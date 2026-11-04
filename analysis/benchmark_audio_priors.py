"""Audit audio-derived priors and refinement seeded only by new candidates.

Location outcomes are diagnostic. Software integrity is not location accuracy,
and an inconclusive acoustic fit must never fall back to manual timing priors.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time

from benchmark_faint import wgs84_distance_m
from benchmark_reviewed_search import make_review, validate_result

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / 'benchmarks' / 'audio_priors_contract.json'


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def receipt(path):
    path = Path(path).resolve(strict=True)
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def load_json(path):
    def invalid(value):
        raise ValueError(f'Nonfinite JSON: {value}')
    return json.loads(Path(path).read_text(encoding='utf-8-sig'), parse_constant=invalid)


def write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=2, allow_nan=False) + '\n', encoding='utf-8')


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def close(a, b, tolerance=1e-8):
    return finite(a) and finite(b) and abs(a-b) <= tolerance


def source_snapshot():
    paths = set((ROOT / 'src').glob('*.rs')) | set((ROOT / 'src').glob('*.html'))
    paths.update(ROOT / name for name in ('Cargo.toml', 'Cargo.lock', 'analysis/benchmark_audio_priors.py',
                                         'analysis/refine_automatic.py', 'analysis/import_reference.py',
                                         'analysis/benchmark_faint.py', 'analysis/benchmark_reviewed_search.py'))
    return {str(p.relative_to(ROOT)): digest(p) for p in sorted(paths)}


def emission_domain_frequency(time_s, parameters, sound_speed):
    """Independent monotone arrival-time inversion, not the fitted closed form."""
    speed, distance = parameters['speed_mps'], parameters['slant_distance_m']
    f0, center = parameters['emitted_frequency_hz'], parameters['emission_cpa_recording_s']
    require(all(finite(v) for v in (time_s, speed, distance, f0, center, sound_speed)), 'Nonfinite physical fit')
    require(0 < speed < sound_speed and distance > 0 and f0 > 0, 'Invalid physical fit')
    arrival = time_s - center
    bound = (abs(arrival) + distance / sound_speed + 1.) / (1. - speed / sound_speed)
    low, high = -bound, bound
    for _ in range(85):
        emitted = (low + high) / 2
        received = emitted + math.hypot(distance, speed * emitted) / sound_speed
        if received < arrival:
            low = emitted
        else:
            high = emitted
    emitted = (low + high) / 2
    radial_velocity = speed * speed * emitted / math.hypot(distance, speed * emitted)
    return f0 / (1. + radial_velocity / sound_speed)


def selected_events(discovery, selected):
    by_id = {e['event_id']: e for e in discovery['events']}
    require(len(by_id) == len(discovery['events']), 'Duplicate discovery identities')
    events = []
    for choice in selected:
        event = by_id[choice['event_id']]
        require(event['harmonic_group_id'] == choice['harmonic_group_id'], 'Selected event/group mismatch')
        events.append(event)
    require(len(events) == 2 and len({c['harmonic_group_id'] for c in selected}) == 2, 'Two independent groups required')
    return events


def validate_fit_report(priors, discovery, contract, returncode):
    require(priors.get('location_verified') is False, 'Acoustic fitting cannot verify a location')
    require(priors.get('utc_clock_inferred') is False, 'Doppler shape cannot infer absolute UTC')
    fits = priors.get('fits')
    require(isinstance(fits, list) and len(fits) == 2, 'Two acoustic fit reports required')
    ready = priors.get('usable_for_search') is True
    require(priors.get('status') == ('audio_priors_ready' if ready else 'inconclusive_audio_priors'), 'Contradictory prior status')
    require(returncode == (0 if ready else 3), 'Prior exit code contradicts usability')
    events = selected_events(discovery, contract['review']['selected'])
    speed_of_sound = contract['regional_geometry']['sound_speed_mps']
    checks = []
    for index, (report, source) in enumerate(zip(fits, events, strict=True)):
        require(isinstance(report.get('usable_for_search'), bool), 'Missing fit usability diagnostic')
        require(report.get('diagnostics') is not None and report.get('limitations'), 'Fit must preserve diagnostics and limitations')
        diagnostics = report['diagnostics']
        require(diagnostics.get('sensitivity_is_confidence_interval') is False, 'Empirical sensitivity cannot be labelled a confidence interval')
        require(diagnostics.get('sample_count') == len(source['ridge']), 'Fit sample count differs from selected measurements')
        if ready:
            require(report['usable_for_search'] is True and report.get('fit') and report.get('envelope'), 'Unsafe fit became ready search priors')
            require(not diagnostics.get('reasons') and diagnostics.get('leave_time_block_out_failures') == 0, 'Unresolved fit diagnostics became ready search priors')
            require(all(diagnostics.get(key) is False for key in ('best_on_physical_boundary', 'best_on_numerical_boundary', 'profile_touches_physical_boundary', 'profile_touches_numerical_boundary')), 'Boundary fit cannot claim closed usable priors')
        parameters = report.get('fit')
        check = {'event_id': source['event_id'], 'usable_for_search': report['usable_for_search']}
        if parameters:
            prediction = [emission_domain_frequency(p['time_s'], parameters, speed_of_sound) for p in source['ridge']]
            rmse = math.sqrt(sum((f-p['frequency_hz'])**2 for f, p in zip(prediction, source['ridge'], strict=True)) / len(prediction))
            require(close(rmse, parameters['rmse_hz'], 1e-7), 'Published acoustic RMSE disagrees with independent emission-domain physics')
            heard = parameters['emission_cpa_recording_s'] + parameters['slant_distance_m'] / speed_of_sound
            require(close(heard, parameters['heard_cpa_recording_s']), 'Heard CPA confuses emission and reception time')
            before = sum(p['time_s'] < heard for p in source['ridge'])
            after = sum(p['time_s'] > heard for p in source['ridge'])
            require(diagnostics['samples_before_heard_cpa'] == before and diagnostics['samples_after_heard_cpa'] == after, 'Fit support diagnostics disagree with measured samples')
            check.update({'independent_rmse_hz': rmse, 'sample_count': len(prediction), 'heard_cpa_checked': True})
        checks.append(check)
    if not ready:
        require(not all(f['usable_for_search'] for f in fits) or priors.get('inconclusive_reason'), 'Inconclusive result needs a reason')
        require(not priors.get('derived') and all(not e.get('f0_hz') for e in priors.get('events', [])), 'Inconclusive fitting must not publish usable manual fallback priors')
    return {'passed': True, 'ready': ready, 'fits': checks, 'independent_model': 'Emission-domain arrival inversion by bisection'}


def expected_grid(bounds, center, step):
    low, high = bounds
    require(all(finite(v) for v in (low, high, center, step)) and low <= center <= high and step > 0, 'Invalid fitted grid')
    first, last = math.ceil(low / step), math.floor(high / step)
    require(last - first <= 9995, 'Fitted grid exceeds declared numerical budget')
    return sorted({low, high, center, *(i * step for i in range(first, last + 1))})


def validate_derived_priors(priors, contract):
    policy = contract['search_padding_policy']
    names = {'frequency_margin_hz': 'emitted_frequency_margin_hz', 'timing_margin_s': 'heard_cpa_margin_s',
             'speed_fraction': 'speed_fraction', 'slant_fraction': 'slant_fraction',
             'slant_min_margin_m': 'slant_minimum_margin_m', 'frequency_step_hz': 'frequency_step_hz',
             'shift_step_s': 'shared_shift_step_s'}
    padding = priors['search_padding']; settings = priors['settings']
    require(padding == {name: policy[key] for name, key in names.items()}, 'Native search padding changed from the preregistered policy')
    require(settings['sound_speed_mps'] == contract['regional_geometry']['sound_speed_mps'], 'Acoustic sound-speed assumption changed')
    ranges = []
    for report in priors['fits']:
        fit, envelope = report['fit'], report['envelope']
        for field in ('emitted_frequency_hz', 'heard_cpa_recording_s', 'speed_mps', 'slant_distance_m'):
            interval = envelope[field]
            require(len(interval) == 2 and all(finite(v) for v in interval) and interval[0] <= fit[field] <= interval[1], 'Fit sensitivity envelope does not contain its fitted value')
        speed_pad = padding['speed_fraction'] * fit['speed_mps']
        slant_pad = max(padding['slant_fraction'] * fit['slant_distance_m'], padding['slant_min_margin_m'])
        speed_raw = [envelope['speed_mps'][0] - speed_pad, envelope['speed_mps'][1] + speed_pad]
        slant_raw = [envelope['slant_distance_m'][0] - slant_pad, envelope['slant_distance_m'][1] + slant_pad]
        speed = [max(speed_raw[0], settings['speed_bounds_mps'][0]), min(speed_raw[1], settings['speed_bounds_mps'][1])]
        slant = [max(slant_raw[0], settings['slant_bounds_m'][0]), min(slant_raw[1], settings['slant_bounds_m'][1])]
        frequency = [max(sys.float_info.min, envelope['emitted_frequency_hz'][0] - padding['frequency_margin_hz']), envelope['emitted_frequency_hz'][1] + padding['frequency_margin_hz']]
        tau = [envelope['heard_cpa_recording_s'][0] - padding['timing_margin_s'], envelope['heard_cpa_recording_s'][1] + padding['timing_margin_s']]
        ranges.append({'emitted_frequency_hz': frequency, 'f0_hz': expected_grid(frequency, fit['emitted_frequency_hz'], padding['frequency_step_hz']),
                       'heard_cpa_recording_s': tau, 'speed_mps': speed, 'slant_distance_m': slant,
                       'physical_domain_clipping': {'speed': speed != speed_raw, 'slant': slant != slant_raw}})
    derived = priors['derived']
    require(derived['event_ranges'] == ranges, 'Generated priors disagree with independently padded acoustic envelopes')
    first, second = [f['fit']['heard_cpa_recording_s'] for f in priors['fits']]
    one, two = [r['heard_cpa_recording_s'] for r in ranges]
    separation = second - first
    bounds = [two[0] - one[1], two[1] - one[0]]
    require(bounds[0] > 0 and separation > 0, 'Ambiguous event order cannot become pair timing')
    require(derived['event1_cpa_recording_s'] == first and derived['measured_separation_s'] == separation, 'Manual CPA or separation replaced the acoustic fit')
    require(derived['separation_bounds_s'] == bounds and derived['separation_tolerance_s'] == max(separation - bounds[0], bounds[1] - separation), 'Pair separation does not cover both fitted timing intervals')
    require(derived['shared_shifts_s'] == expected_grid([one[0] - first, one[1] - first], 0., padding['shift_step_s']), 'Shared clock grid failed to cover the first acoustic timing interval')
    require(derived['coarse_bounds'] == [{'speed_mps': r['speed_mps'], 'slant_distance_m': r['slant_distance_m']} for r in ranges], 'Manual coarse bounds replaced acoustic estimates')
    require([event['f0_hz'] for event in priors['events']] == [r['f0_hz'] for r in ranges], 'Emitted-frequency grids differ from acoustic derivation')
    return {'passed': True, 'interval_padding_recomputed_independently': True, 'full_grid_endpoints_preserved': True, 'calibrated_confidence_claimed': False}


def validate_source_binding(priors, discovery, contract, discovery_hash, review_hash):
    evidence = priors['provenance']['reviewed_audio']
    require(evidence['discovery']['sha256'] == discovery_hash and evidence['review_selection']['sha256'] == review_hash, 'Audio fit no longer binds reviewed detection bytes')
    require(evidence['source']['sha256'] == discovery['source_sha256'] and evidence['review_actor'] == 'agent', 'Acoustic source or declared reviewer changed')
    require(priors['selected_ridges'] == contract['review']['selected'], 'Audio stage substituted selected ridges')
    selected = selected_events(discovery, contract['review']['selected'])
    require(priors['frequency_sigma_hz'] == [e['quality']['frequency_sigma_hz'] for e in selected], 'Nominal frequency measurement scale changed')
    require(len(evidence['selected_ridges']) == 2, 'Selected quality evidence missing')
    for record, source in zip(evidence['selected_ridges'], selected, strict=True):
        require(record['quality'] == source['quality'] and record['contrast_weights'] == [p['weight'] for p in source['ridge']], 'Original acoustic quality or contrast evidence changed')
        omitted = [e['event_id'] for e in discovery['events'] if e['harmonic_group_id'] == source['harmonic_group_id'] and e['event_id'] != source['event_id']]
        require(record['other_group_ridges_not_scored'] == omitted, 'Other harmonics were implicitly accepted or hidden')
    return {'passed': True, 'source_and_review_bound': True, 'measurement_quality_preserved': True, 'unscored_harmonics_explicit': True}


def validate_generated_search(search, priors, discovery, contract, normalized, observers):
    require(search['tracks'] == [p['track'] for p in normalized['passes']], 'Candidate tracks changed from normalized source')
    require(search['observers'] == observers, 'Regional observer grid changed')
    require(search['schema_version'] == 1 and search['evidence_kind'] == 'real_recording', 'Search evidence schema changed')
    geometry = contract['regional_geometry']
    require(search['pair_config']['clock_offset_bounds_s'] == geometry['clock_offset_bounds_s'], 'Explicit day/clock bound changed')
    require(search['pair_config']['model'] == {'sound_speed_mps': geometry['sound_speed_mps'], 'max_gap_s': geometry['max_track_gap_s']}, 'Propagation model changed')
    derived = priors['derived']
    for field in ('event1_cpa_recording_s', 'shared_shifts_s'):
        require(search['pair_config'][field] == derived[field], f'Search ignored audio-derived {field}')
    for field in ('coarse_bounds', 'measured_separation_s', 'separation_tolerance_s'):
        require(search[field] == derived[field], f'Search ignored audio-derived {field}')
    require(search['events'] == priors['events'], 'Search changed generated event samples/frequency grids')
    require(len(search['events']) == 2, 'Search lost an event')
    selected = selected_events(discovery, contract['review']['selected'])
    details = []
    for event, source in zip(search['events'], selected, strict=True):
        expected = [{'t_rec_s': p['time_s'], 'frequency_hz': p['frequency_hz']} for p in source['ridge']]
        require(event['points'] == expected, 'Audio-derived stage clipped, resampled, rescaled or filled measured points')
        require(event['f0_hz'] and all(finite(f) and f > 0 for f in event['f0_hz']), 'Invalid derived frequency grid')
        require(all(a < b for a, b in zip(event['f0_hz'], event['f0_hz'][1:])), 'Frequency grid must be strictly increasing')
        details.append({'event_id': source['event_id'], 'samples': len(expected), 'max_gap_s': max(b['t_rec_s'] - a['t_rec_s'] for a, b in zip(expected, expected[1:])), 'samples_preserved_exactly': True})
    fits = [f['fit'] for f in priors['fits']]
    require(close(derived['event1_cpa_recording_s'], fits[0]['heard_cpa_recording_s']), 'Clock anchor did not come from audio fit')
    require(close(derived['measured_separation_s'], fits[1]['heard_cpa_recording_s'] - fits[0]['heard_cpa_recording_s']), 'Separation did not come from both acoustic fits')
    return {'passed': True, 'events': details, 'tracks_preserved': len(search['tracks']), 'observers_preserved': len(observers), 'manual_search_priors_not_used': True}


def validate_refinement(report, regional):
    require(report.get('execution_passed') is True and report.get('location_verified') is False, 'Refinement failed or verified a location')
    require(report.get('conditional_on_retained_pairs') is True, 'Finite refinement cannot claim global completeness')
    expected, seen = [], set()
    for row in regional['best_per_flight_pair']:
        pair = tuple(row['score']['flight_ids'])
        if pair not in seen:
            expected.append(pair); seen.add(pair)
        if len(expected) == 3:
            break
    pairs = report.get('pairs', [])
    require([tuple(p['flight_ids']) for p in pairs] == expected, 'Refinement was not seeded by the new top distinct regional pairs')
    for pair in pairs:
        require(pair.get('local') is not None, 'Retained pair has no local search')
        if pair['local'].get('best') is not None:
            require(pair.get('fine') is not None, 'Retained local winner did not receive its own fine grid')
    require(report.get('status') in ('completed', 'coverage_incomplete', 'no_candidate'), 'Unknown refinement status')
    return {'passed': True, 'new_candidate_pairs_only': True, 'retained_pairs': len(pairs), 'scientific_status': report['status']}


def command(arguments, prefix, timeout):
    began = time.perf_counter()
    execution = {'command': list(map(str, arguments)), 'timeout_s': timeout}
    with Path(str(prefix) + '.stdout').open('wb') as out, Path(str(prefix) + '.stderr').open('wb') as err:
        try:
            completed = subprocess.run(arguments, cwd=ROOT, stdout=out, stderr=err, timeout=timeout, check=False)
            execution['returncode'] = completed.returncode
        except subprocess.TimeoutExpired:
            execution.update({'returncode': None, 'timed_out': True})
    execution['elapsed_s'] = time.perf_counter() - began
    write_json(Path(str(prefix) + '.json'), execution)
    return execution


def compare(winner, baseline, locations):
    result = {'location_verified': False, 'geographic_acceptance_gate': None,
              'score_comparability': 'RMSE values use partial automatic ridges; manual traces differ. Lower error or reference distance is not calibrated accuracy.'}
    if winner is None:
        return dict(result, scientific_status='no_candidate', winner=None)
    manual = baseline['candidates'][0]
    point = lambda row: (row['observer']['latitude_deg'], row['observer']['longitude_deg'])
    result.update({'winner': winner, 'manual_winner': manual,
                   'same_flight_pair_as_manual': winner['score']['flight_ids'] == manual['score']['flight_ids'],
                   'distance_from_manual_m': wgs84_distance_m(point(winner), point(manual)),
                   'clock_difference_s': winner['score']['clock_offset_s'] - manual['score']['clock_offset_s'],
                   'reference_distances_m': {k: {'new': wgs84_distance_m(point(winner), v['coordinate']), 'manual': wgs84_distance_m(point(manual), v['coordinate']), 'reference': v} for k, v in locations.items()}})
    return result


def run(exe, output, contract_path=CONTRACT):
    output = Path(output).resolve(); output.mkdir(parents=True, exist_ok=False); (output / 'logs').mkdir()
    started = time.perf_counter()
    report = {'schema_version': 1, 'status': 'running', 'passed': False, 'location_verified': False,
              'independent_location_accuracy_established': False, 'started_utc': datetime.now(timezone.utc).isoformat(), 'stages': [], 'evaluation_read_after_predictions': False}
    frozen = {}
    try:
        contract = load_json(contract_path)
        require(contract['schema_version'] == 1, 'Unsupported frozen contract')
        paths = {k: (ROOT / v['path']).resolve(strict=True) for k, v in contract['files'].items()}
        for name, path in paths.items():
            require(digest(path) == contract['files'][name]['sha256'], f'Frozen input hash mismatch: {name}')
            frozen[str(path)] = digest(path)
        frozen[str(Path(contract_path).resolve())] = digest(contract_path)
        discovery = load_json(paths['discovery'])
        source = Path(discovery['source']).resolve(strict=True)
        require(digest(source) == discovery['source_sha256'], 'Recording no longer matches discovery')
        frozen[str(source)] = digest(source)
        binary = output / 'geosonnar.exe'; shutil.copyfile(Path(exe).resolve(strict=True), binary)
        frozen[str(Path(exe).resolve())] = digest(exe); frozen[str(binary)] = digest(binary)
        code_hashes = source_snapshot()
        shutil.copyfile(__file__, output / 'runner-snapshot.py'); shutil.copyfile(contract_path, output / 'contract-snapshot.json')
        report.update({'scope': contract['scope'], 'provenance': {'executable': receipt(exe), 'snapshot': receipt(binary), 'source_sha256': code_hashes,
                       'inputs_sha256': frozen.copy(), 'contract': receipt(contract_path), 'python': platform.python_version()},
                       'scientific_outcomes': contract['scientific_outcomes'], 'selection_basis': contract['review']})
        write_json(output / 'report.json', report)
        review_path = output / 'review-selection.json'; write_json(review_path, make_review(discovery, digest(paths['discovery']), contract))
        audio_config_path = output / 'audio-config.json'
        write_json(audio_config_path, {'schema_version': 1, 'discovery': str(paths['discovery']), 'review_selection': str(review_path),
                                       'selected_ridges': contract['review']['selected'], 'selection_basis': contract['review']['basis']})
        for path in (review_path, audio_config_path): frozen[str(path)] = digest(path)
        audio_dir = output / 'audio-priors'
        execution = command([str(binary), 'derive-audio-priors', '--config', str(audio_config_path), '--out', str(audio_dir)], output / 'logs' / 'derive', contract['timeout_s_per_command'])
        report['stages'].append({'stage': 'derive_audio_priors', 'execution': execution})
        require(execution.get('returncode') in (0, 3), 'Native acoustic derivation failed/timed out')
        priors_path = audio_dir / 'priors.json'; priors = load_json(priors_path)
        report['fit_audit'] = validate_fit_report(priors, discovery, contract, execution['returncode'])
        report['source_binding_audit'] = validate_source_binding(priors, discovery, contract, digest(paths['discovery']), digest(review_path))
        report['priors'] = receipt(priors_path); frozen[str(priors_path)] = digest(priors_path)
        if report['fit_audit']['ready']:
            report['derived_priors_audit'] = validate_derived_priors(priors, contract)
            write_json(output / 'report.json', report)
            geo = contract['regional_geometry']
            geometry = {k: geo[k] for k in ('observer_bounds', 'grid_spacing_m', 'observer_ellipsoid_height_m', 'clock_offset_bounds_s')}
            geometry.update({'schema_version': 1, 'normalized_tracks': str(paths['normalized_tracks']), 'observers_file': str(paths['observers']),
                             'model': {'sound_speed_mps': geo['sound_speed_mps'], 'max_gap_s': geo['max_track_gap_s']},
                             'provenance': ['Source-informed regional candidate coverage/date and terrain; no manual acoustic or fine-grid priors.', contract['scope']]})
            geometry_path = output / 'geometry.json'; write_json(geometry_path, geometry); frozen[str(geometry_path)] = digest(geometry_path)
            search_dir = output / 'regional'
            execution = command([str(binary), 'prepare-audio-search', '--priors', str(priors_path), '--geometry', str(geometry_path), '--out', str(search_dir)], output / 'logs' / 'prepare', contract['timeout_s_per_command'])
            report['stages'].append({'stage': 'prepare_audio_search', 'execution': execution}); require(execution.get('returncode') == 0, 'Preparing audio-derived search failed')
            search_path = search_dir / 'search_input.json'; search = load_json(search_path)
            report['transfer_audit'] = validate_generated_search(search, priors, discovery, contract, load_json(paths['normalized_tracks']), load_json(paths['observers']))
            report['regional_input'] = receipt(search_path); frozen[str(search_path)] = digest(search_path)
            result_path = search_dir / 'results.json'
            execution = command([str(binary), 'rank', '--input', str(search_path), '--out', str(result_path)], output / 'logs' / 'regional-rank', contract['timeout_s_per_command'])
            report['stages'].append({'stage': 'regional_search', 'execution': execution}); require(execution.get('returncode') in (0, 3), 'Regional search failed/timed out')
            result = load_json(result_path); report['regional_audit'] = validate_result(result, search, execution['returncode'])
            report['regional_result'] = receipt(result_path); frozen[str(result_path)] = digest(result_path)
            write_json(output / 'report.json', report)
            refined = None
            if result['candidates']:
                policy = contract['refinement']; refine_dir = output / 'refinement'
                arguments = [sys.executable, str(ROOT / 'analysis' / 'refine_automatic.py'), '--input', str(search_path), '--coarse-results', str(result_path), '--exe', str(binary), '--out-dir', str(refine_dir), '--dem', str(paths['dem']), '--geoid', str(paths['geoid']), '--microphone-height-m', str(geo['microphone_height_m'])]
                for name in ('top_pairs', 'radius_m', 'spacing_m', 'fine_radius_m', 'fine_spacing_m'):
                    arguments += ['--' + name.replace('_', '-'), str(policy[name])]
                arguments += ['--timeout-s', str(contract['timeout_s_per_command'])]
                execution = command(arguments, output / 'logs' / 'refine', contract['timeout_s_per_command'] * 8)
                report['stages'].append({'stage': 'refinement', 'execution': execution}); require(execution.get('returncode') == 0, 'Automatic refinement failed/timed out')
                refine_path = refine_dir / 'report.json'; refined = load_json(refine_path)
                report['refinement_audit'] = validate_refinement(refined, result)
                report['refinement_report'] = receipt(refine_path); frozen[str(refine_path)] = digest(refine_path)
            # Evaluation boundary: all new predictions and own-candidate refinements are complete.
            locations = load_json(paths['evaluation_locations'])
            report['evaluation'] = {'regional': compare(result['candidates'][0] if result['candidates'] else None, load_json(paths['regional_manual_baseline']), locations),
                                    'refined': compare(refined['best'] if refined else None, load_json(paths['fine_manual_baseline']), locations),
                                    'scope': 'Diagnostic comparison only; no known point or old fine grid seeded this run.'}
            report['evaluation_read_after_predictions'] = True
            report['scientific_status'] = refined['status'] if refined else 'no_regional_candidate'
        else:
            report['scientific_status'] = 'inconclusive_audio_priors'
            report['matching_not_started'] = True
        require(source_snapshot() == code_hashes, 'Source code changed during the run')
        for path, expected in frozen.items(): require(digest(path) == expected, f'Input/output changed during the run: {path}')
        report.update({'status': 'passed_software_integrity', 'passed': True, 'inputs_and_code_unchanged': True, 'final_inputs_and_outputs_sha256': frozen})
    except KeyboardInterrupt:
        report.update({'status': 'interrupted', 'error': 'Interrupted; unfinished stages are not passed.'})
    except Exception as error:
        report.update({'status': 'failed', 'error': f'{type(error).__name__}: {error}'})
    report['elapsed_s'] = time.perf_counter() - started
    report['finished_utc'] = datetime.now(timezone.utc).isoformat()
    write_json(output / 'report.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=ROOT / 'target' / 'release' / 'geosonnar.exe')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args(); result = run(args.exe, args.output)
    print(json.dumps({k: result.get(k) for k in ('status', 'scientific_status', 'elapsed_s', 'error')}, indent=2))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
