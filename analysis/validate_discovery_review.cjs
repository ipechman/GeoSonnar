#!/usr/bin/env node
'use strict';
// Code-level test of our generated review page. No browser, server, network,
// screenshot, or visual-QA claim. The DOM below implements only the used APIs.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const vm = require('node:vm');

function hash(file) { return crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex'); }
function normalize(value) { return JSON.parse(JSON.stringify(value)); }
function scripts(html) {
  const matches = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)];
  assert.equal(matches.length, 2, 'Exactly one JSON payload and one inline UI script are required');
  assert.match(matches[0][1], /type\s*=\s*["']application\/json["']/i);
  assert.match(matches[0][1], /id\s*=\s*["']report["']/i);
  assert.equal(matches[1][1].trim(), '', 'UI script must be inline without remote attributes');
  assert(!/[<>]/.test(matches[0][2]), 'Embedded JSON must escape HTML delimiters');
  return { jsonText: matches[0][2], behavior: matches[1][2], jsonBlock: matches[0][0] };
}

function checkOffline(html, code) {
  assert(!/<script\b[^>]*\bsrc\s*=/i.test(html), 'External scripts are forbidden');
  assert(!/<(?:link|img|iframe|audio|video|source|object|embed)\b/i.test(html), 'Review must not need external media/assets');
  const styles = [...html.matchAll(/<style\b[^>]*>([\s\S]*?)<\/style\s*>/gi)].map(match => match[1]).join('\n');
  assert(!/@import\b|url\s*\(/i.test(styles), 'Styles must be self-contained');
  assert(!/\b(?:fetch|XMLHttpRequest|WebSocket|EventSource|sendBeacon|Worker)\s*\(/.test(code), 'UI must not perform network or worker requests');
  assert(!/\bimport\s*\(|\brequire\s*\(/.test(code), 'UI must not load executable dependencies');
  assert(!/\.(?:innerHTML|outerHTML)\s*=|insertAdjacentHTML\s*\(/.test(code), 'Report strings must use textContent, not HTML insertion');
}

function makeDom(jsonText) {
  const clicks = [], blobs = new Map(), revoked = [], roots = new Map();
  let serial = 0;
  class Element {
    constructor(tag, namespace = null) { this.tag = tag; this.namespace = namespace; this.children = []; this.attributes = {}; this._text = ''; this._value = null; this.hidden = false; }
    set textContent(value) { this._text = String(value); this.children = []; }
    get textContent() { return this._text + this.children.map(child => child.textContent).join(''); }
    set innerHTML(_) { throw new Error('HTML injection is forbidden in the review fixture'); }
    set outerHTML(_) { throw new Error('HTML injection is forbidden in the review fixture'); }
    setAttribute(key, value) { this.attributes[key] = String(value); }
    append(...children) { assert(children.every(child => child instanceof Element)); this.children.push(...children); }
    set value(value) {
      if (this.tag === 'select') assert(this.children.some(option => option.value === value), 'Select only declared review options');
      this._value = String(value);
    }
    get value() { return this._value ?? (this.tag === 'select' ? this.children[0]?.value ?? '' : ''); }
    click() { clicks.push({ href: this.href, download: this.download }); }
  }
  for (const id of ['report', 'source', 'stats', 'empty', 'method', 'overview', 'rows', 'details', 'export']) roots.set(id, new Element(id === 'export' ? 'button' : 'div'));
  roots.get('report').textContent = jsonText;
  const document = {
    getElementById(id) { assert(roots.has(id), `Unexpected DOM ID ${id}`); return roots.get(id); },
    createElement(tag) { return new Element(tag); },
    createElementNS(namespace, tag) { assert.equal(namespace, 'http://www.w3.org/2000/svg'); return new Element(tag, namespace); },
  };
  class MemoryBlob { constructor(parts, options) { this.text = parts.map(String).join(''); this.type = options.type; } }
  const memoryUrl = {
    createObjectURL(blob) { const url = `blob:review-test-${++serial}`; blobs.set(url, blob); return url; },
    revokeObjectURL(url) { assert(blobs.has(url)); revoked.push(url); },
  };
  const forbidden = () => { throw new Error('Network/dependency access is forbidden by this code-level test'); };
  const context = vm.createContext({ document, Blob: MemoryBlob, URL: memoryUrl,
    setTimeout(callback) { callback(); return 0; }, fetch: forbidden, XMLHttpRequest: forbidden,
    WebSocket: forbidden, EventSource: forbidden, Worker: forbidden, navigator: { sendBeacon: forbidden },
  }, { codeGeneration: { strings: false, wasm: false } });
  return { roots, context, clicks, blobs, revoked, Element };
}

function children(element, tag) { return element.children.filter(child => child.tag === tag); }
function descendants(element) { return element.children.flatMap(child => [child, ...descendants(child)]); }
function measuredSegments(event, maxGap) {
  const segments = []; let current = [];
  for (const point of event.ridge) {
    if (current.length && point.time_s - current.at(-1).time_s > maxGap) { segments.push(current); current = []; }
    current.push(point);
  }
  if (current.length) segments.push(current);
  return segments;
}

function inspectPlot(svg, events, limits, settings) {
  assert.equal(svg.tag, 'svg');
  assert.equal(svg.attributes.viewBox, '0 0 1100 330');
  for (const element of [svg, ...descendants(svg)]) {
    for (const [name, value] of Object.entries(element.attributes)) {
      assert(!/NaN|Infinity|undefined/.test(value), `Invalid SVG ${name}=${value}`);
      if (['x', 'y', 'x1', 'x2', 'y1', 'y2', 'stroke-width', 'font-size'].includes(name)) assert(Number.isFinite(Number(value)), `SVG coordinate ${name} must be finite`);
    }
  }
  const lines = children(svg, 'polyline');
  const expected = events.flatMap(event => measuredSegments(event, settings.max_gap_s));
  assert.equal(lines.length, expected.length, 'Every missing span must break the SVG polyline');
  let measuredCount = 0;
  for (let i = 0; i < expected.length; i++) {
    const rendered = lines[i].attributes.points.trim().split(/\s+/).map(pair => pair.split(',').map(Number));
    assert.equal(rendered.length, expected[i].length, 'Chart must retain exactly the measured samples');
    for (let j = 0; j < rendered.length; j++) {
      assert(rendered[j].length === 2 && rendered[j].every(Number.isFinite));
      const point = expected[i][j];
      const x = 65 + (point.time_s - limits.lo) / Math.max(.001, limits.hi - limits.lo) * 1010;
      const y = 280 - (point.frequency_hz - limits.low) / Math.max(1, limits.high - limits.low) * 260;
      assert(Math.abs(rendered[j][0] - x) < 1e-8 && Math.abs(rendered[j][1] - y) < 1e-8, 'Chart coordinate must map the original observation');
      measuredCount++;
    }
  }
  return { segments: lines.length, measured_points: measuredCount };
}

function runUi(html, report, discoverySha256) {
  assert.match(discoverySha256, /^[a-f0-9]{64}$/, 'Review binding must be the exact discovery-file SHA-256');
  const extracted = scripts(html);
  assert.deepEqual(JSON.parse(extracted.jsonText), report, 'Embedded report differs from discovery.json');
  checkOffline(html, extracted.behavior);
  const dom = makeDom(extracted.jsonText);
  new vm.Script(extracted.behavior, { filename: 'generated-review-ui.js' }).runInContext(dom.context, { timeout: 10000 });
  assert.equal(dom.roots.get('source').textContent, report.source);
  assert.equal(dom.roots.get('empty').hidden, report.group_count !== 0);
  assert.deepEqual(JSON.parse(dom.roots.get('method').textContent), {
    discovery_sha256: discoverySha256,
    source_sha256: report.source_sha256, source_metadata: report.source_metadata,
    settings: report.settings, limitations: report.limitations,
  });
  const selectors = descendants(dom.roots.get('rows')).filter(node => node.tag === 'select');
  assert.equal(selectors.length, report.groups.length);
  assert(selectors.every(select => select.value === 'unreviewed'), 'Review must not default to accepting proposals');
  for (const select of selectors) assert.deepEqual(select.children.map(option => option.value), ['unreviewed', 'keep_for_matching', 'reject', 'uncertain']);
  assert.equal(report.groups.length, report.group_count);
  assert(report.groups.every(group => group.review_status === 'unreviewed' && group.aircraft_identity_confirmed === false));
  const overview = children(dom.roots.get('overview'), 'svg');
  assert.equal(overview.length, 1);
  const plots = [inspectPlot(overview[0], report.events, { lo: 0, hi: report.source_metadata.duration_s,
    low: report.settings.min_frequency_hz, high: report.settings.max_frequency_hz }, report.settings)];
  const details = children(dom.roots.get('details'), 'details');
  assert.equal(details.length, report.groups.length);
  for (let i = 0; i < details.length; i++) {
    const group = report.groups[i], events = report.events.filter(event => event.harmonic_group_id === group.harmonic_group_id);
    assert(events.length > 0, 'A displayed group must have measured ridges');
    const svgs = children(details[i], 'svg'); assert.equal(svgs.length, 1);
    plots.push(inspectPlot(svgs[0], events, { lo: group.start_s, hi: group.end_s,
      low: Math.max(0, Math.min(...events.map(event => event.band_hz[0])) - 5),
      high: Math.max(...events.map(event => event.band_hz[1])) + 5 }, report.settings));
    const links = children(details[i], 'a'); assert.equal(links.length, events.length);
    for (let j = 0; j < links.length; j++) {
      assert.match(links[j].href, /^[A-Za-z0-9_.-]+\.csv$/);
      assert.equal(links[j].href, events[j].event_id + '.csv');
      assert.equal(links[j].download, links[j].href);
      let prevented = false;
      links[j].onclick({ preventDefault() { prevented = true; } });
      assert(prevented, 'CSV download must not navigate relative to the regenerated review directory');
      const csvClick = dom.clicks.at(-1), csvBlob = dom.blobs.get(csvClick.href);
      assert.equal(csvClick.download, events[j].event_id + '.csv');
      assert.equal(csvBlob.type, 'text/csv');
      assert.equal(csvBlob.text, 'time_s,frequency_hz,weight\n' + events[j].ridge.map(p => `${p.time_s},${p.frequency_hz},${p.weight}`).join('\n') + '\n');
      assert(dom.revoked.includes(csvClick.href));
    }
  }
  function exportAndCheck(choices) {
    dom.roots.get('export').onclick();
    const click = dom.clicks.at(-1); assert.equal(click.download, 'review-selection.json');
    const blob = dom.blobs.get(click.href); assert.equal(blob.type, 'application/json');
    const output = JSON.parse(blob.text);
    assert.equal(output.schema_version, 2);
    assert.equal(output.discovery_sha256, discoverySha256, 'Review must bind the exact discovery file, not just its audio source');
    assert.equal(output.review_actor, 'human', 'Browser selections must declare the human review actor');
    assert.equal(output.source_sha256, report.source_sha256);
    assert.equal(output.selected_channel_index, report.source_metadata.selected_channel_index);
    assert.equal(output.location_verified, false);
    assert.deepEqual(output.groups, report.groups.map((group, i) => ({ harmonic_group_id: group.harmonic_group_id,
      review: choices[i], event_ids: report.events.filter(event => event.harmonic_group_id === group.harmonic_group_id).map(event => event.event_id) })));
    assert(dom.revoked.includes(click.href), 'Temporary download URL must be revoked');
    return output;
  }
  exportAndCheck(selectors.map(() => 'unreviewed'));
  const choices = selectors.map((select, i) => { const value = ['keep_for_matching', 'reject', 'uncertain', 'unreviewed'][i % 4]; select.value = value; return value; });
  exportAndCheck(choices);
  assert.deepEqual(JSON.parse(vm.runInContext('JSON.stringify(report)', dom.context)), report, 'Export must not mutate original detection evidence');
  assert.equal(vm.runInContext('globalThis.__reviewInjection', dom.context), undefined);
  return { selector_count: selectors.length, plots, exported_choices: choices,
    embedded_report_matches: true, finite_coordinates: true, gap_segments_preserved: true,
    all_initially_unreviewed: true, exports_preserve_provenance: true, exact_discovery_file_bound: true,
    review_actor_explicit: true, original_report_unchanged: true };
}

function withSyntheticReport(html, report) {
  const serialized = JSON.stringify(report);
  const digest = crypto.createHash('sha256').update(serialized).digest('hex');
  const escaped = serialized.replaceAll('&', '\\u0026').replaceAll('<', '\\u003c').replaceAll('>', '\\u003e');
  const extracted = scripts(html);
  const replaced = html.replace(extracted.jsonBlock, `<script type="application/json" id="report">${escaped}</script>`)
    .replace(/const discoverySha256='[a-f0-9]{64}';/, `const discoverySha256='${digest}';`);
  return { html: replaced, digest };
}

function injectionControl(html, report, rustSource) {
  // Verify the actual native renderer has these replacements before applying
  // that same declared transformation to a synthetic report. This is a static
  // renderer/JS control, not a claim that an OS filename contained '<script>'.
  for (const [character, escape] of [['&', '\\u0026'], ['<', '\\u003c'], ['>', '\\u003e']]) {
    assert(rustSource.includes(`.replace('${character}', "${escape.replaceAll('\\', '\\\\')}")`), 'Native renderer escape contract changed');
  }
  const altered = normalize(report);
  const payload = 'Unicode Ω 中文 🦉 & <script>globalThis.__reviewInjection=true</script> "quoted" __DISCOVERY_SHA256__ __REPORT_JSON__ \u2028 \u2029';
  altered.source = payload;
  altered.limitations = [...altered.limitations, payload];
  altered.source_metadata.time_reference = payload;
  const injected = withSyntheticReport(html, altered);
  assert(injected.html.includes('\\u003cscript\\u003e'));
  runUi(injected.html, altered, injected.digest);
  return { passed: true, payload_preserved_as_text: true, script_not_executed: true,
    method: 'Static verification of native escape replacements, then synthetic JSON/textContent fixture in a minimal DOM; not native filename injection or browser security testing.' };
}

function validate(directory) {
  const reportPath = path.join(directory, 'discovery.json'), htmlPath = path.join(directory, 'review.html');
  const rustPath = path.resolve(__dirname, '../src/discovery.rs');
  const report = JSON.parse(fs.readFileSync(reportPath, 'utf8'));
  const html = fs.readFileSync(htmlPath, 'utf8');
  const actual = runUi(html, report, hash(reportPath));
  const injection = injectionControl(html, report, fs.readFileSync(rustPath, 'utf8'));
  // An explicit gapped fixture exercises line breaks even for an input without
  // gaps. Its values are only UI controls and are never a discovery result.
  const gapped = normalize(report);
  gapped.group_count = 1; gapped.ridge_count = 1;
  const secondStart = 1.5 + report.settings.max_gap_s + 5;
  gapped.source_metadata.duration_s = secondStart + 2;
  gapped.groups = [{ harmonic_group_id: 'gap-control', start_s: 0, end_s: secondStart + 2, ridge_count: 1,
    review_status: 'unreviewed', aircraft_identity_confirmed: false }];
  gapped.events = [{ event_id: 'gap-control', harmonic_group_id: 'gap-control', band_hz: [90, 130],
    ridge: [{ time_s: 1, frequency_hz: 125, weight: 5 }, { time_s: 1.5, frequency_hz: 124, weight: 5 },
      { time_s: secondStart, frequency_hz: 105, weight: 5 }, { time_s: secondStart + .5, frequency_hz: 104, weight: 5 }],
    quality: { support_fraction: .2, fragment_count: 2 } }];
  const gapFixture = withSyntheticReport(html, gapped), gapHtml = gapFixture.html;
  const gap = runUi(gapHtml, gapped, gapFixture.digest);
  assert(gap.plots.every(plot => plot.segments === 2 && plot.measured_points === 4));
  const negativeControls = [];
  for (const [name, before, after] of [
    ['false_location_acceptance_is_caught', 'location_verified:false', 'location_verified:true'],
    ['automatic_review_acceptance_is_caught', 'reviews.set(g.harmonic_group_id,select);', 'select.value="keep_for_matching";reviews.set(g.harmonic_group_id,select);'],
    ['bridged_missing_measurements_are_caught', 'if(last!==null&&p.time_s-last>report.settings.max_gap_s)line();', ''],
    ['network_dependency_is_caught', "'use strict';", "'use strict'; fetch('https://invalid.example/no-request-allowed');"],
    ['wrong_discovery_binding_is_caught', 'schema_version:2,discovery_sha256:discoverySha256', `schema_version:2,discovery_sha256:'${'0'.repeat(64)}'`],
    ['missing_review_actor_is_caught', "review_actor:'human',", ''],
  ]) {
    const mutant = gapHtml.replace(before, after);
    assert.notEqual(mutant, gapHtml, `Failure control ${name} no longer matches the generated script`);
    assert.throws(() => runUi(mutant, gapped, gapFixture.digest), undefined, `Failure control ${name} must fail`);
    negativeControls.push({ name, expected_failure_observed: true });
  }
  return { schema_version: 1, passed: true, scope: 'Static generated-page and Node vm/minimal-DOM tests only',
    visual_browser_qa: false, browser_opened: false, server_started: false, network_used: false,
    input_directory: directory, source_sha256: report.source_sha256, selected_channel_index: report.source_metadata.selected_channel_index,
    artifacts: { discovery_json_sha256: hash(reportPath), review_html_sha256: hash(htmlPath),
      native_renderer_source_sha256: hash(rustPath), validator_sha256: hash(__filename) },
    node: process.version, actual_report: actual, unicode_and_script_text_control: injection,
    synthetic_gap_control: gap, validator_failure_controls: negativeControls,
    limitations: ['No layout, font, interaction accessibility, or browser rendering was visually inspected.',
      'The minimal DOM models only the APIs used by this generated page. This is not a general browser/security sandbox.'] };
}

function main() {
  const args = process.argv.slice(2), options = {};
  for (let i = 0; i < args.length; i += 2) {
    assert(['--input', '--out'].includes(args[i]) && args[i + 1], 'Usage: node validate_discovery_review.cjs --input DIR --out RECEIPT.json');
    assert(!options[args[i]], 'Duplicate option'); options[args[i]] = args[i + 1];
  }
  assert(options['--input'] && options['--out'], 'Both --input and --out are required');
  const directory = path.resolve(options['--input']), destination = path.resolve(options['--out']);
  assert(!fs.existsSync(destination), 'Existing validation receipts are never overwritten');
  let receipt;
  try { receipt = validate(directory); }
  catch (error) { receipt = { schema_version: 1, passed: false, visual_browser_qa: false,
    scope: 'Static generated-page and Node vm/minimal-DOM tests only', input_directory: directory,
    error: `${error.name}: ${error.message}`, stack: error.stack }; }
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx' });
  console.log(JSON.stringify({ receipt: destination, passed: receipt.passed, visual_browser_qa: false, error: receipt.error }));
  process.exitCode = receipt.passed ? 0 : 1;
}

if (require.main === module) main();
module.exports = { validate, runUi, scripts };
