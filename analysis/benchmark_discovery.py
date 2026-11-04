"""Fresh native discovery controls and all-40 real audio proposal audit.

The primary signal generator is adapted from DOPPLERGEIST's pinned
scripts/make_benchmark_controls.py (70626ad44fb9c7f4e2f5b225956c1f9281f91c13).
Extension signals follow its tests/test_discovery.py. Copyright (c) 2026
Cinna Davis, MIT; see licenses/DOPPLERGEIST-MIT.txt. Production discovery
is exclusively the Rust executable. Python generates and evaluates fixtures.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import sys
import time

import numpy as np
import soundfile as sf
from scipy.optimize import linear_sum_assignment

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'benchmarks'))
from run_validation import WindowsChildJob

CONTRACT = ROOT / 'benchmarks/discovery_contract.json'
PIN = '70626ad44fb9c7f4e2f5b225956c1f9281f91c13'


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + '\n', encoding='utf-8')


def load_json(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def receipt(path):
    path = Path(path).resolve()
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def native_source_snapshot():
    # include_str! embeds the review document into the executable, so it is
    # part of native provenance even though its file extension is not .rs.
    paths = set((ROOT / 'src').glob('*.rs'))
    paths.add(ROOT / 'src/discovery_review.html')
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(paths)}


def command(arguments, prefix, timeout=240):
    """Use the tested owned-job cleanup contract, including FFmpeg descendants."""
    prefix = Path(prefix)
    argv = list(map(str, arguments))
    row = {'command': argv, 'timeout_s': timeout}
    started = time.monotonic()
    try:
        with prefix.with_suffix('.stdout').open('wb') as stdout, prefix.with_suffix('.stderr').open('wb') as stderr:
            job = WindowsChildJob() if os.name == 'nt' else None
            options = {'creationflags': subprocess.CREATE_NO_WINDOW | 0x4} if job else {'start_new_session': True}
            try:
                with subprocess.Popen(argv, cwd=ROOT, stdout=stdout, stderr=stderr, **options) as child:
                    try:
                        if job:
                            job.assign_and_resume(child)
                        row['returncode'] = child.wait(timeout=timeout)
                    except BaseException:
                        if job:
                            job.close()
                        else:
                            try:
                                os.killpg(child.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        if child.poll() is None:
                            child.kill()
                        child.wait(timeout=15)
                        raise
            finally:
                if job:
                    job.close()
        if row['returncode'] != 0:
            raise RuntimeError(f'Exit {row["returncode"]}; see {prefix.with_suffix(".stderr")}')
    except BaseException as error:
        row['execution_error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        row['elapsed_s'] = time.monotonic() - started
        write_json(prefix.with_suffix('.command.json'), row)
    return row


def generate_controls(directory):
    """Exact 14 upstream emission-domain controls plus four named extensions."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=False)
    inputs, truth = directory / 'inputs', directory / 'truth'
    inputs.mkdir(); truth.mkdir()
    cases, labels = [], {}
    sr = 8000
    for split, seed in [('development', 892741), ('public_held_out', 219305)]:
        rng = np.random.default_rng(seed)
        for kind in ['noise', 'stationary', 'non_aircraft_sweep', 'harmonic', 'overlap', 'short', 'mp3']:
            key = f'control-{len(cases) + 1:03d}'
            duration = 8 if kind == 'short' else 100
            t = np.arange(duration * sr) / sr
            audio = rng.normal(0, .025, len(t))
            events = []
            if kind == 'stationary':
                audio += .18 * np.sin(2 * np.pi * 173 * t)
            elif kind == 'non_aircraft_sweep':
                audio += .2 * np.sin(2 * np.pi * (110 * t + .6 * t**2))
            elif kind != 'noise':
                centers = [32, 59] if kind == 'overlap' else [45]
                if kind == 'short':
                    centers = [0]
                for index, center in enumerate(centers):
                    speed = 73 + rng.uniform(-8, 8)
                    slant = 1400 + rng.uniform(-120, 120)
                    frequency = 126 + index * 59 + rng.uniform(-4, 4)
                    emission = np.arange(-32 * sr, 32 * sr) / sr
                    distance = np.sqrt(slant**2 + (speed * emission)**2)
                    arrival = center + emission + distance / 343
                    envelope = np.hanning(len(emission)) * (slant / distance)
                    source = .22 * envelope * (np.sin(2 * np.pi * frequency * emission) +
                             .6 * np.sin(2 * np.pi * 2 * frequency * emission + .3))
                    audio += np.interp(t, arrival, source, left=0, right=0)
                    if kind != 'short':
                        events.append({'start_s': max(0, float(arrival[0])), 'end_s': min(duration, float(arrival[-1])),
                                       'center_emission_s': center, 'speed_mps': float(speed), 'slant_m': float(slant),
                                       'source_frequency_hz': float(frequency), 'harmonics': [1, 2]})
            audio_path = inputs / f'{key}.wav'
            sf.write(audio_path, audio, sr, subtype='PCM_16')
            if kind == 'mp3':
                target = audio_path.with_suffix('.mp3')
                command(['ffmpeg', '-v', 'error', '-nostdin', '-i', audio_path, '-map_metadata', '-1',
                         '-codec:a', 'libmp3lame', '-b:a', '64k', target], directory / f'{key}-encode')
                # Keep the source fixture as evidence; only MP3 is passed to Rust.
                audio_path = target
            cases.append({'case_id': key, 'split': split, 'lane': 'primary', 'audio_path': str(audio_path.resolve()),
                          'duration_s': duration, 'audio_sha256': digest(audio_path)})
            labels[key] = {'kind': kind, 'events': events, 'expected_groups': len(events)}
    for kind in ['missing_fundamental', 'interrupted', 'switching_stationary', 'weak_partial']:
        key = f'control-{len(cases) + 1:03d}'
        sr, duration = 4000, 140
        t = np.arange(sr * duration) / sr
        rng = np.random.default_rng(811 if kind == 'switching_stationary' else 9133)
        audio = rng.normal(0, .04 if kind == 'switching_stationary' else .08, len(t))
        if kind == 'switching_stationary':
            audio += .2 / (1 + np.exp((t - 60) / 2)) * np.sin(2 * np.pi * 74 * t)
            audio += .2 / (1 + np.exp(-(t - 60) / 2)) * np.sin(2 * np.pi * 65 * t)
        else:
            audio += .12 * np.sin(2 * np.pi * 400 * t)
            relative = t - 65
            frequency = 120 * (1 - .18 * relative / np.sqrt(relative**2 + 8**2))
            phase = 2 * np.pi * np.cumsum(frequency) / sr
            amplitude = .16 * np.exp(-(relative / 24)**2) * (abs(relative) < 44)
            harmonics = [2, 3] if kind == 'missing_fundamental' else [1, 2]
            if kind == 'interrupted':
                amplitude *= (t < 58) | (t > 69)
            if kind == 'weak_partial':
                amplitude *= .15 * ((t >= 66) & (t <= 79))
            for harmonic in harmonics:
                audio += amplitude / harmonic * np.sin(harmonic * phase)
        audio_path = inputs / f'{key}.wav'
        sf.write(audio_path, audio, sr, subtype='FLOAT')
        cases.append({'case_id': key, 'split': 'semantic_extension', 'lane': 'extension',
                      'audio_path': str(audio_path.resolve()), 'duration_s': duration, 'audio_sha256': digest(audio_path)})
        labels[key] = {'kind': kind, 'expected_groups': 0 if kind == 'switching_stationary' else (None if kind == 'weak_partial' else 1)}
    write_json(inputs / 'manifest.json', {'schema_version': 1, 'cases': cases})
    write_json(truth / 'labels.json', labels)
    write_json(directory / 'provenance.json', {'generator': receipt(__file__), 'contract': receipt(CONTRACT),
                'upstream_commit': PIN, 'license': 'MIT; analysis/licenses/DOPPLERGEIST-MIT.txt',
                'scope': 'Public deterministic development controls; no location labels or flight corpus.'})
    return inputs / 'manifest.json'


def finite(value):
    return not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value)


def validate_prediction(prediction, duration):
    if prediction.get('location_verified') is not False:
        raise AssertionError('Discovery must explicitly report location_verified=false')
    events = prediction.get('events')
    if not isinstance(events, list):
        raise AssertionError('Missing events array')
    identifiers, groups = set(), {}
    for event in events:
        identifier, group = event.get('event_id'), event.get('harmonic_group_id')
        if not isinstance(identifier, str) or not identifier or identifier in identifiers or not isinstance(group, str) or not group:
            raise AssertionError('Event identifiers must be unique and harmonic groups nonempty')
        identifiers.add(identifier)
        start, end = event.get('start_s'), event.get('end_s')
        if not finite(start) or not finite(end) or not 0 <= start < end <= duration + .1:
            raise AssertionError('Invalid event interval or timestamps beyond input')
        band = event.get('band_hz')
        if not isinstance(band, list) or len(band) != 2 or not all(finite(x) for x in band) or not 0 < band[0] < band[1]:
            raise AssertionError('Invalid frequency band')
        ridge = event.get('ridge')
        if not isinstance(ridge, list) or len(ridge) < 2:
            raise AssertionError('A discovered ridge must contain observations')
        previous = -1
        for point in ridge:
            t, f, weight = point.get('time_s'), point.get('frequency_hz'), point.get('weight')
            if not all(finite(x) for x in [t, f, weight]) or not start - 1e-6 <= t <= end + 1e-6 or t <= previous or f <= 0 or weight < 0:
                raise AssertionError('Ridge observation must be finite, chronological, within interval, and nonnegative weight')
            if not band[0] - 1e-6 <= f <= band[1] + 1e-6:
                raise AssertionError('Ridge observation is outside its declared band')
            previous = t
        if not isinstance(event.get('quality'), dict):
            raise AssertionError('Missing quality diagnostics')
        groups.setdefault(group, []).append(event)
    if (type(prediction.get('group_count')) is not int or type(prediction.get('ridge_count')) is not int
            or prediction['group_count'] != len(groups) or prediction['ridge_count'] != len(events)):
        raise AssertionError('Group/ridge counts disagree with returned events')
    return groups


def interval_iou(a, b):
    intersection = max(0., min(a[1], b[1]) - max(a[0], b[0]))
    return intersection / (max(a[1], b[1]) - min(a[0], b[0]))


def match_intervals(truth, groups, threshold):
    """Maximize cardinality, then total IoU; never reuse a group for two events."""
    candidates = {name: [min(e['start_s'] for e in rows), max(e['end_s'] for e in rows)] for name, rows in groups.items()}
    names = list(candidates)
    if not truth or not names:
        return []
    # Dummy columns permit an unmatched true event. One match is worth more
    # than all IoU tie-break contributions together; ineligible pairs score0.
    scores = np.zeros((len(truth), len(names) + len(truth)))
    ious = np.zeros((len(truth), len(names)))
    priority = min(len(truth), len(names)) + 1
    for index, label in enumerate(truth):
        for column, name in enumerate(names):
            iou = interval_iou([label['start_s'], label['end_s']], candidates[name])
            ious[index, column] = iou
            if iou >= threshold:
                scores[index, column] = priority + iou
    rows, columns = linear_sum_assignment(scores, maximize=True)
    return [{'truth_index': int(index), 'group_id': names[column], 'iou': float(ious[index, column])}
            for index, column in zip(rows, columns, strict=True) if column < len(names) and scores[index, column] > 0]


def source_frequency_at_arrival(t, source):
    """Independent closed-form retarded time for uniform rectilinear subsonic motion."""
    tau = np.asarray(t) - source['center_emission_s']
    speed, slant, c = source['speed_mps'], source['slant_m'], 343.
    emission = (c*c*tau - np.sqrt(c*c*speed*speed*tau*tau + (c*c-speed*speed)*slant*slant)) / (c*c-speed*speed)
    distance = np.sqrt(slant*slant + speed*speed*emission*emission)
    return source['source_frequency_hz'] / (1 + speed*speed*emission / (c*distance))


def evaluate_synthetic(prediction, label, duration, contract):
    groups = validate_prediction(prediction, duration)
    checks = {'group_count': label['expected_groups'] is None or len(groups) == label['expected_groups']}
    result = {'kind': label['kind'], 'observed_groups': len(groups), 'expected_groups': label['expected_groups'], 'checks': checks}
    if 'events' in label:
        truth = label['events']
        matched = match_intervals(truth, groups, contract['primary_synthetic']['group_interval_iou_minimum'])
        result.update({'truth_events': len(truth), 'matched': matched, 'missed_events': len(truth) - len(matched), 'unmatched_groups': len(groups) - len(matched)})
        checks['no_missed_events'] = len(matched) == len(truth)
        checks['no_unmatched_groups'] = len(matched) == len(groups)
        for pair in matched:
            source = truth[pair['truth_index']]
            points = [point for event in groups[pair['group_id']] for point in event['ridge']
                      if source['start_s'] <= point['time_s'] <= source['end_s']]
            if not points:
                pair['median_absolute_ridge_error_hz'] = None
                checks['ridge_frequency_agrees'] = False
                continue
            expected = source_frequency_at_arrival([p['time_s'] for p in points], source)
            observed = np.asarray([p['frequency_hz'] for p in points])
            errors = np.min(np.abs(observed[:, None] - expected[:, None] * np.asarray(source['harmonics'])[None, :]), axis=1)
            pair['median_absolute_ridge_error_hz'] = float(np.median(errors))
            pair['evaluated_points'] = len(points)
            checks['ridge_frequency_agrees'] = checks.get('ridge_frequency_agrees', True) and pair['median_absolute_ridge_error_hz'] <= contract['primary_synthetic']['maximum_matched_ridge_median_absolute_error_hz']
    if label['kind'] == 'missing_fundamental':
        checks['retained_two_harmonics'] = len(prediction['events']) >= 2 and any(e['quality'].get('harmonic_count', 0) >= 2 for e in prediction['events'])
    if label['kind'] == 'interrupted':
        checks['fragments_recorded'] = any(e['quality'].get('fragment_count', 0) >= 2 for e in prediction['events'])
        checks['no_invented_gap_observations'] = not any(60 < p['time_s'] < 67 for e in prediction['events'] for p in e['ridge'])
    result['status'] = 'passed' if all(checks.values()) else 'failed'
    if label['kind'] == 'weak_partial':
        result['sensitivity_result'] = 'diagnostic_only'
    return result


def run_native(executable, audio, output, logs, key, timeout, expected_duration):
    execution = command([executable, 'discover', '--input', audio, '--out', output, '--channel', '0'], logs / key, timeout)
    result_file = output / 'discovery.json'
    if not result_file.is_file():
        raise AssertionError('Native discovery exited without discovery.json')
    prediction = load_json(result_file)
    if prediction.get('source_sha256') != digest(audio):
        raise AssertionError('Native output source hash does not match the actual submitted audio')
    metadata = prediction.get('source_metadata', {})
    if metadata.get('selected_channel_index') != 0 or metadata.get('channels_averaged') is not False:
        raise AssertionError('Native discovery did not preserve the selected channel')
    decoded_duration = metadata.get('duration_s')
    if not finite(decoded_duration) or abs(decoded_duration - expected_duration) > .1:
        raise AssertionError('Decoded recording duration differs from the independent source duration by more than0.1s')
    return prediction, execution


def run_discovery_benchmark(executable, output, corpus_manifest=None, timeout=240):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / 'logs').mkdir()
    started = time.monotonic()
    contract = load_json(CONTRACT)
    executable = Path(executable).resolve()
    snapshot = output / executable.name
    shutil.copy2(executable, snapshot)
    shutil.copy2(CONTRACT, output / 'frozen-contract.json')
    shutil.copy2(__file__, output / 'runner-snapshot.py')
    source_hashes = native_source_snapshot()
    report = {'schema_version': 1, 'status': 'running', 'started_utc': datetime.now(timezone.utc).isoformat(),
              'provenance': {'executable': receipt(executable), 'snapshot': receipt(snapshot), 'contract': receipt(CONTRACT),
                             'source_sha256': source_hashes, 'python': platform.python_version(), 'numpy': np.__version__,
                             'soundfile': sf.__version__, 'runner': receipt(__file__),
                             'upstream_source_receipt': receipt(ROOT / 'benchmarks/upstream' / PIN / 'discovery_source/receipt.json')},
              'synthetic': [], 'real': [], 'scope': 'Native acoustic discovery only; no geolocation validation.'}
    def save():
        report['elapsed_s'] = time.monotonic() - started
        write_json(output / 'report.json', report)
    save()
    try:
        for tool in ['ffmpeg', 'ffprobe']:
            location = shutil.which(tool)
            if not location:
                raise FileNotFoundError(f'Required media tool not found: {tool}')
            report['provenance'][tool] = receipt(location)
            command([location, '-version'], output / 'logs' / f'{tool}-version', timeout)
            report['provenance'][tool]['version'] = (output / 'logs' / f'{tool}-version.stdout').read_text(encoding='utf-8', errors='replace').splitlines()[0]
        manifest = generate_controls(output / 'controls')
        cases = load_json(manifest)['cases']
        if len(cases) != 18 or len({c['case_id'] for c in cases}) != 18:
            raise AssertionError('Expected all14 primary and four extension controls')
        produced = []
        for case in cases:
            key = case['case_id']
            row = {'case_id': key, 'lane': case['lane'], 'split': case['split'], 'input': receipt(case['audio_path'])}
            try:
                prediction, execution = run_native(snapshot, case['audio_path'], output / key, output / 'logs', key, timeout, case['duration_s'])
                row.update({'status': 'produced', 'prediction': str(output / key / 'discovery.json'), 'execution': execution})
            except Exception as error:
                row.update({'status': 'failed', 'error': f'{type(error).__name__}: {error}'})
            produced.append(row)
            report['synthetic'] = produced
            save()
            print(json.dumps({'case': key, 'stage': 'synthetic_production', 'status': row['status']}), flush=True)
        # Truth is parsed only after every primary/extension prediction has been produced.
        labels = load_json(output / 'controls/truth/labels.json')
        for case, row in zip(cases, produced, strict=True):
            if row['status'] != 'produced':
                continue
            try:
                row['evaluation'] = evaluate_synthetic(load_json(row['prediction']), labels[case['case_id']], case['duration_s'], contract)
                row['status'] = row['evaluation']['status']
            except Exception as error:
                row.update({'status': 'failed', 'error': f'{type(error).__name__}: {error}'})
        report['synthetic_summary'] = {'cases': len(produced), 'primary_cases': sum(r['lane'] == 'primary' for r in produced),
                 'passed': sum(r['status'] == 'passed' for r in produced), 'failed': sum(r['status'] == 'failed' for r in produced),
                 'missed_events': sum(r.get('evaluation', {}).get('missed_events', 0) for r in produced),
                 'unmatched_groups': sum(r.get('evaluation', {}).get('unmatched_groups', 0) for r in produced),
                 'weak_partial_is_diagnostic': True}
        save()
        if corpus_manifest:
            corpus_manifest = Path(corpus_manifest).resolve()
            report['provenance']['corpus_manifest'] = receipt(corpus_manifest)
            if report['provenance']['corpus_manifest']['sha256'] != contract['real_corpus']['manifest_sha256']:
                raise AssertionError('Real40 manifest differs from the source identity frozen before evaluation')
            corpus = load_json(corpus_manifest)
            recordings = corpus.get('recordings')
            if not isinstance(recordings, list) or len(recordings) != 40 or len({r['case_id'] for r in recordings}) != 40:
                raise AssertionError('Real corpus requires exactly40 unique declared recordings; no subset is a pass')
            inputs = output / 'real-inputs'; inputs.mkdir()
            for index, recording in enumerate(recordings):
                key = f'clip-{index:03d}'
                source = Path(recording['audio_path'])
                if not source.is_absolute():
                    source = corpus_manifest.parent / source
                row = {'case_id': key, 'source_case': recording['case_id']}
                try:
                    row['source'] = receipt(source)
                    if row['source']['sha256'] != recording['sha256']:
                        raise AssertionError('Real input content differs from corpus receipt')
                    audio = inputs / f'{key}.mp3'
                    command(['ffmpeg', '-v', 'error', '-nostdin', '-i', source, '-map', '0:a:0', '-vn', '-map_metadata', '-1',
                             '-c:a', 'libmp3lame', '-b:a', '192k', audio], output / 'logs' / f'{key}-encode', timeout)
                    row['encoded_input'] = receipt(audio)
                    prediction, execution = run_native(snapshot, audio, output / key, output / 'logs', key, timeout, recording['duration_s'])
                    groups = validate_prediction(prediction, recording['duration_s'])
                    row.update({'status': 'passed', 'proposal_groups': len(groups), 'ridge_records': len(prediction['events']),
                                'execution': execution, 'prediction': str(output / key / 'discovery.json')})
                except Exception as error:
                    row.update({'status': 'failed', 'error': f'{type(error).__name__}: {error}'})
                report['real'].append(row)
                save()
                print(json.dumps({'case': key, 'stage': 'real_production', 'status': row['status'], 'groups': row.get('proposal_groups')}), flush=True)
            rows = report['real']
            report['real_summary'] = {'recordings': len(rows), 'technical_passed': sum(r['status'] == 'passed' for r in rows),
                  'failed': sum(r['status'] == 'failed' for r in rows), 'recordings_with_proposals': sum(r.get('proposal_groups', 0) > 0 for r in rows),
                  'recordings_without_proposals': sum(r.get('proposal_groups') == 0 for r in rows),
                  'proposal_groups': sum(r.get('proposal_groups', 0) for r in rows), 'ridge_records': sum(r.get('ridge_records', 0) for r in rows),
                  'event_precision': None, 'event_recall': None, 'limitation': contract['real_corpus']['limitation']}
        else:
            report['real_summary'] = {'status': 'not_requested', 'reason': 'No corpus manifest supplied; synthetic-only audit cannot claim real40 validation.'}
        unchanged = source_hashes == native_source_snapshot()
        report['provenance']['native_source_unchanged'] = unchanged
        report['provenance']['executable_unchanged'] = digest(executable) == report['provenance']['executable']['sha256']
        report['provenance']['contract_unchanged'] = digest(CONTRACT) == report['provenance']['contract']['sha256']
        failures = sum(r['status'] != 'passed' for r in report['synthetic'] + report['real'])
        report['status'] = 'passed' if failures == 0 and unchanged and report['provenance']['executable_unchanged'] and report['provenance']['contract_unchanged'] else 'failed'
    except KeyboardInterrupt:
        report['status'] = 'interrupted'
        report['interruption'] = 'Cancelled before completing the declared case set; this is not a validation pass.'
    except Exception as error:
        report['status'] = 'failed'
        report['fatal_error'] = f'{type(error).__name__}: {error}'
    save()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--corpus-manifest', type=Path)
    parser.add_argument('--timeout', type=float, default=240)
    parser.add_argument('--generate-only', action='store_true', help='Freeze fixtures without executing the detector')
    args = parser.parse_args()
    if args.generate_only:
        print(generate_controls(args.output))
        return 0
    if not args.exe:
        parser.error('--exe is required unless --generate-only')
    report = run_discovery_benchmark(args.exe, args.output, args.corpus_manifest, args.timeout)
    print(json.dumps({key: report.get(key) for key in ['status', 'synthetic_summary', 'real_summary', 'elapsed_s', 'fatal_error']}, indent=2))
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    sys.exit(main())
