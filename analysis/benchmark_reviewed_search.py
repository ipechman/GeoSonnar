"""Frozen reviewed-discovery transfer and source-informed matching comparison.

Software gates are independent of the geographic outcome. The inherited fine
grid/pair is a conditional comparison, not automatic refinement or blind truth.
Evaluation baseline results are read only after both new predictions complete.
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
import time

from benchmark_faint import wgs84_distance_m

ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / 'benchmarks' / 'reviewed_search_contract.json'
PRESERVED_FIELDS = ('schema_version', 'evidence_kind', 'tracks', 'observers',
                    'pair_config', 'coarse_bounds', 'measured_separation_s',
                    'separation_tolerance_s')


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def receipt(path):
    path = Path(path).resolve(strict=True)
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def load_json(path):
    def invalid_number(value):
        raise ValueError(f'Nonfinite JSON number: {value}')
    return json.loads(Path(path).read_text(encoding='utf-8-sig'), parse_constant=invalid_number)


def write_json(path, value):
    Path(path).write_text(json.dumps(value, allow_nan=False, indent=2) + '\n', encoding='utf-8')


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def source_snapshot():
    paths = sorted(set((ROOT / 'src').glob('*.rs')) | set((ROOT / 'src').glob('*.html')))
    paths += [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock', Path(__file__),
              ROOT / 'analysis' / 'benchmark_faint.py']
    return {str(p.relative_to(ROOT)): digest(p) for p in paths}


def make_review(discovery, discovery_sha256, contract):
    selected = contract['review']['selected']
    keep = {item['harmonic_group_id'] for item in selected}
    require(len(keep) == 2, 'The pair needs two distinct reviewed groups')
    require(discovery.get('location_verified') is False, 'Discovery must remain unverified')
    review = {
        'schema_version': 2,
        'discovery_sha256': discovery_sha256,
        'source_sha256': discovery['source_sha256'],
        'selected_channel_index': discovery['source_metadata']['selected_channel_index'],
        'review_actor': 'agent',
        'location_verified': False,
        'scope': contract['review']['basis'],
        'groups': [{
            'harmonic_group_id': group['harmonic_group_id'],
            'review': 'keep_for_matching' if group['harmonic_group_id'] in keep else 'unreviewed',
            'event_ids': [event['event_id'] for event in discovery['events']
                          if event['harmonic_group_id'] == group['harmonic_group_id']],
        } for group in discovery['groups']],
    }
    actual = {item['harmonic_group_id'] for item in review['groups'] if item['review'] == 'keep_for_matching'}
    require(actual == keep, 'A frozen selected group is absent from the discovery')
    return review


def validate_transfer(native_input, template, discovery, selected):
    """Independent equality audit: no interpolation, filtering or retuning."""
    for field in PRESERVED_FIELDS:
        require(native_input.get(field) == template.get(field), f'Changed inherited search field: {field}')
    require(native_input['evidence_kind'] == 'real_recording', 'Source recording must remain real evidence')
    require(len(native_input.get('events', [])) == 2, 'Expected exactly two transferred events')
    events = {event['event_id']: event for event in discovery['events']}
    require(len(events) == len(discovery['events']), 'Discovery event identities are not unique')
    details = []
    for index, (event, choice) in enumerate(zip(native_input['events'], selected, strict=True)):
        source = events[choice['event_id']]
        require(source['harmonic_group_id'] == choice['harmonic_group_id'], 'Ridge belongs to another group')
        points = [{'t_rec_s': p['time_s'], 'frequency_hz': p['frequency_hz']} for p in source['ridge']]
        require(event['points'] == points, f'Changed, filled, reordered or clipped measured event {index + 1}')
        require(event['f0_hz'] == choice['f0_hz'] == template['events'][index]['f0_hz'],
                'Emitted-tone grid drifted from the declared manual baseline')
        gaps = [b['t_rec_s'] - a['t_rec_s'] for a, b in zip(points, points[1:])]
        details.append({
            'event_id': choice['event_id'], 'harmonic_group_id': choice['harmonic_group_id'],
            'sample_count': len(points), 'time_range_s': [points[0]['t_rec_s'], points[-1]['t_rec_s']],
            'max_measured_gap_s': max(gaps), 'source_quality': source['quality'],
            'samples_preserved_exactly': True, 'frequency_grid_unchanged': True,
            'weight_policy': 'Matcher uses equal sample weights; original contrast weights and quality remain in the discovery and bridge receipt.',
        })
    require(native_input.get('provenance'), 'Reviewed search provenance is missing')
    return {'passed': True, 'preserved_fields': list(PRESERVED_FIELDS), 'events': details}


def validate_bridge_receipt(bridge, discovery, selected, discovery_hash, review_hash, search_hash):
    require(bridge.get('status') == 'prepared_reviewed_search', 'Bridge did not publish a completed receipt')
    require(bridge.get('review_actor') == 'agent' and bridge.get('location_verified') is False,
            'Bridge changed the declared reviewer or verified a location')
    require(bridge.get('aircraft_identity_confirmed') is False, 'Review cannot confirm an aircraft identity')
    require(bridge['provenance']['discovery']['sha256'] == discovery_hash, 'Receipt discovery binding changed')
    require(bridge['provenance']['review_selection']['sha256'] == review_hash, 'Receipt review binding changed')
    require(bridge['provenance']['source']['sha256'] == discovery['source_sha256'], 'Receipt source binding changed')
    require(bridge['search_input']['sha256'] == search_hash, 'Receipt search binding changed')
    require(len(bridge['selected_ridges']) == len(selected) == 2, 'Receipt lost selected ridge evidence')
    by_id = {event['event_id']: event for event in discovery['events']}
    for evidence, choice in zip(bridge['selected_ridges'], selected, strict=True):
        event = by_id[choice['event_id']]
        for field in ('event_id', 'harmonic_group_id', 'f0_hz'):
            require(evidence[field] == choice[field], f'Receipt selected {field} changed')
        require(evidence['point_count'] == len(event['ridge']), 'Receipt sample count changed')
        require(evidence['quality'] == event['quality'], 'Receipt lost the original uncertainty/quality')
        require(evidence['contrast_weights'] == [p['weight'] for p in event['ridge']], 'Receipt lost original contrast weights')
        others = [e['event_id'] for e in discovery['events']
                  if e['harmonic_group_id'] == choice['harmonic_group_id'] and e['event_id'] != choice['event_id']]
        require(evidence['other_group_ridges_not_scored'] == others, 'Receipt hides untested harmonics')
        gaps = [{'after_time_s': a['time_s'], 'before_time_s': b['time_s'], 'duration_s': b['time_s'] - a['time_s']}
                for a, b in zip(event['ridge'], event['ridge'][1:])
                if b['time_s'] - a['time_s'] > discovery['settings']['max_gap_s']]
        require(evidence['preserved_gaps'] == gaps, 'Receipt lost measured gap locations')
    return {'passed': True, 'source_and_review_bound': True, 'quality_weights_and_gaps_preserved': True,
            'untested_harmonics_explicit': True}


def validate_result(result, native_input, returncode):
    require(result.get('location_verified') is False, 'Ranking must not claim a verified location')
    require(result.get('evidence_kind') == 'real_recording', 'Result evidence kind changed')
    candidates = result.get('candidates')
    require(isinstance(candidates, list), 'Missing candidate rows')
    require(result.get('ranked_count') == len(candidates), 'Candidate count disagrees with rows')
    require(result.get('observer_count') == len(native_input['observers']), 'Observer count changed')
    require(result.get('track_count') == len(native_input['tracks']), 'Track count changed')
    if not candidates:
        require(returncode == 3 and result.get('status') == 'no_candidate', 'No candidate must remain an explicit exit-3 outcome')
        require(result.get('best_per_flight_pair') == [], 'Empty search has an invented best pair')
        return {'scientific_status': 'no_candidate', 'candidate_count': 0, 'location_verified': False}
    require(returncode == 0 and result.get('status') == 'hypotheses_ranked', 'Ranked output has a contradictory status/exit')
    ids = {track['flight_id'] for track in native_input['tracks']}
    observers = {observer['id']: observer for observer in native_input['observers']}
    previous = -math.inf
    for row in candidates:
        observer, score = row['observer'], row['score']
        require(observer == observers.get(observer['id']), 'Candidate observer was not in the frozen grid')
        require(len(score['flight_ids']) == 2 and all(flight in ids for flight in score['flight_ids']),
                'Candidate aircraft was not in the inherited track set')
        values = [score['combined_rmse_hz'], score['clock_offset_s'], score['shared_shift_s'],
                  *score['rmse_hz'], *score['fitted_f0_hz']]
        require(all(finite(v) for v in values), 'Candidate contains a nonfinite numeric score')
        require(len(score['rmse_hz']) == 2 and len(score['fitted_f0_hz']) == 2, 'Candidate needs two event scores')
        require(score['combined_rmse_hz'] >= 0 and score['combined_rmse_hz'] >= previous,
                'Candidate scores are negative or not sorted')
        require(all(value >= 0 for value in score['rmse_hz']), 'Negative per-event RMSE')
        require(score['shared_shift_s'] in native_input['pair_config']['shared_shifts_s'], 'Undeclared clock shift')
        previous = score['combined_rmse_hz']
    return {'scientific_status': 'hypotheses_ranked', 'candidate_count': len(candidates), 'location_verified': False}


def coordinates(observer):
    return observer['latitude_deg'], observer['longitude_deg']


def compare_winners(result, baseline, native_input, locations):
    require(baseline.get('location_verified') is False and baseline.get('candidates'), 'Invalid frozen baseline result')
    manual = baseline['candidates'][0]
    comparison = {
        'location_verified': False, 'scientific_status': result['status'],
        'manual_winner': manual,
        'score_comparability': 'Different fixed sample sets; lower RMSE is not proof of a better location or measurement.',
        'geographic_acceptance_gate': None,
    }
    if not result['candidates']:
        comparison.update({'automatic_winner': None, 'same_flight_pair': None,
                           'distance_from_manual_winner_m': None,
                           'interpretation': 'Reviewed transfer completed but these settings support no candidate.'})
        return comparison
    winner = result['candidates'][0]
    comparison['automatic_winner'] = winner
    comparison['same_flight_pair'] = winner['score']['flight_ids'] == manual['score']['flight_ids']
    comparison['distance_from_manual_winner_m'] = wgs84_distance_m(coordinates(winner['observer']), coordinates(manual['observer']))
    comparison['clock_difference_s'] = winner['score']['clock_offset_s'] - manual['score']['clock_offset_s']
    comparison['combined_rmse_difference_hz_diagnostic_only'] = winner['score']['combined_rmse_hz'] - manual['score']['combined_rmse_hz']
    comparison['reference_distances_m'] = {
        name: {'automatic': wgs84_distance_m(coordinates(winner['observer']), tuple(reference['coordinate'])),
               'manual': wgs84_distance_m(coordinates(manual['observer']), tuple(reference['coordinate'])),
               'reference': reference}
        for name, reference in locations.items()
    }
    shifts = native_input['pair_config']['shared_shifts_s']
    comparison['at_shared_shift_boundary'] = winner['score']['shared_shift_s'] in (min(shifts), max(shifts))
    comparison['at_observer_grid_boundary'] = any(
        abs(winner['observer'][field] - edge) <= 1e-12
        for field in ('latitude_deg', 'longitude_deg')
        for edge in (min(o[field] for o in native_input['observers']), max(o[field] for o in native_input['observers'])))
    comparison['interpretation'] = 'Unverified source-informed hypothesis; no settings were changed in response to this result.'
    return comparison


def command(arguments, log_prefix, timeout):
    began = time.perf_counter()
    execution = {'command': list(map(str, arguments)), 'timeout_s': timeout}
    with Path(str(log_prefix) + '.stdout').open('wb') as stdout, Path(str(log_prefix) + '.stderr').open('wb') as stderr:
        try:
            completed = subprocess.run(arguments, cwd=ROOT, stdout=stdout, stderr=stderr, timeout=timeout, check=False)
            execution['returncode'] = completed.returncode
        except subprocess.TimeoutExpired:
            execution.update({'returncode': None, 'timed_out': True})
    execution['elapsed_s'] = time.perf_counter() - began
    write_json(Path(str(log_prefix) + '.json'), execution)
    return execution


def run(exe, output, contract_path=CONTRACT):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / 'logs').mkdir()
    began = time.perf_counter()
    report = {'schema_version': 1, 'status': 'running', 'passed': False,
              'started_utc': datetime.now(timezone.utc).isoformat(), 'location_verified': False,
              'independent_location_accuracy_established': False, 'cases': [],
              'evaluation_results_read_after_all_predictions': False}
    frozen_files = {}
    try:
        contract = load_json(contract_path)
        require(contract['schema_version'] == 1 and len(contract['cases']) == 2, 'Expected frozen two-case contract')
        frozen_files[str(Path(contract_path).resolve())] = digest(contract_path)
        paths = {key: (ROOT / value['path']).resolve(strict=True) for key, value in contract['files'].items()}
        for key, path in paths.items():
            require(digest(path) == contract['files'][key]['sha256'], f'Frozen file hash mismatch: {key}')
            frozen_files[str(path)] = digest(path)
        discovery = load_json(paths['discovery'])
        source = Path(discovery['source']).resolve(strict=True)
        require(digest(source) == discovery['source_sha256'], 'Recording hash differs from the discovery')
        frozen_files[str(source)] = digest(source)
        binary = output / 'geosonnar.exe'
        shutil.copyfile(Path(exe).resolve(strict=True), binary)
        frozen_files[str(Path(exe).resolve(strict=True))] = digest(exe)
        frozen_files[str(binary)] = digest(binary)
        source_hashes = source_snapshot()
        shutil.copyfile(Path(__file__), output / 'runner-snapshot.py')
        shutil.copyfile(contract_path, output / 'contract-snapshot.json')
        report.update({
            'scope': contract['scope'], 'frozen_contract': receipt(contract_path),
            'provenance': {'executable': receipt(exe), 'snapshot': receipt(binary), 'source_sha256': source_hashes,
                           'inputs_sha256': frozen_files.copy(), 'platform': platform.platform(),
                           'python': platform.python_version()},
            'selection_basis': contract['review'],
            'interpretation': contract['scientific_outcomes'],
            'evaluation_isolation': contract['evaluation_isolation'],
        })
        review_path = output / 'review-selection.json'
        write_json(review_path, make_review(discovery, digest(paths['discovery']), contract))
        frozen_files[str(review_path)] = digest(review_path)
        completed_predictions = []
        for case in contract['cases']:
            entry = {'case_id': case['case_id'], 'scope': case['scope'], 'status': 'running'}
            report['cases'].append(entry)
            write_json(output / 'report.json', report)
            case_dir = output / case['case_id']
            config_path = output / (case['case_id'] + '-bridge.json')
            config = {'schema_version': 1, 'discovery': str(paths['discovery']),
                      'review_selection': str(review_path), 'template_search_input': str(paths[case['template']]),
                      'selected_ridges': contract['review']['selected'],
                      'selection_basis': contract['review']['basis']}
            write_json(config_path, config)
            frozen_files[str(config_path)] = digest(config_path)
            prepare_execution = command([str(binary), 'prepare-reviewed-search', '--config', str(config_path), '--out', str(case_dir)],
                                        output / 'logs' / (case['case_id'] + '-prepare'), contract['timeout_s_per_command'])
            entry['prepare_execution'] = prepare_execution
            require(prepare_execution.get('returncode') == 0, f"{case['case_id']} bridge execution failed or timed out")
            input_path = case_dir / 'search_input.json'
            native_input, template = load_json(input_path), load_json(paths[case['template']])
            entry['transfer'] = validate_transfer(native_input, template, discovery, contract['review']['selected'])
            entry['input'] = receipt(input_path)
            entry['bridge_receipt'] = receipt(case_dir / 'receipt.json')
            entry['receipt_audit'] = validate_bridge_receipt(load_json(case_dir / 'receipt.json'), discovery,
                                                            contract['review']['selected'], digest(paths['discovery']),
                                                            digest(review_path), digest(input_path))
            frozen_files[str(input_path)] = digest(input_path)
            result_path = case_dir / 'results.json'
            execution = command([str(binary), 'rank', '--input', str(input_path), '--out', str(result_path)],
                                output / 'logs' / (case['case_id'] + '-rank'), contract['timeout_s_per_command'])
            entry['rank_execution'] = execution
            require(execution.get('returncode') in (0, 3), f"{case['case_id']} search execution failed or timed out")
            result = load_json(result_path)
            entry['outcome'] = validate_result(result, native_input, execution['returncode'])
            entry['results'] = receipt(result_path)
            frozen_files[str(result_path)] = digest(result_path)
            entry['status'] = 'prediction_complete'
            completed_predictions.append((entry, case, input_path, result_path))
            write_json(output / 'report.json', report)
            del native_input, template, result
        require(len(completed_predictions) == 2, 'Both frozen searches must complete before evaluation')
        require(source_snapshot() == source_hashes, 'Source changed while the benchmark ran')
        for path, expected in frozen_files.items():
            require(digest(path) == expected, f'Input or output changed during execution: {path}')
        # Evaluation boundary: both new native predictions are finished and immutable.
        locations = load_json(paths['evaluation_locations'])
        for entry, case, input_path, result_path in completed_predictions:
            entry['comparison'] = compare_winners(load_json(result_path), load_json(paths[case['baseline']]),
                                                 load_json(input_path), locations)
            entry['status'] = 'passed_software_integrity'
        report.update({'status': 'passed_software_integrity', 'passed': True,
                       'evaluation_results_read_after_all_predictions': True,
                       'immutability': {'passed': True, 'files_sha256': frozen_files},
                       'scientific_location_pass': None})
    except KeyboardInterrupt:
        report.update({'status': 'interrupted', 'error': 'Interrupted; no complete comparison claimed.'})
    except Exception as error:
        report.update({'status': 'failed', 'error': f'{type(error).__name__}: {error}'})
    finally:
        report['elapsed_s'] = time.perf_counter() - began
        report['finished_utc'] = datetime.now(timezone.utc).isoformat()
        write_json(output / 'report.json', report)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', type=Path, default=ROOT / 'target' / 'release' / 'geosonnar.exe')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = run(args.exe, args.output)
    print(json.dumps({'status': result['status'], 'report': str(args.output / 'report.json'),
                      'elapsed_s': result['elapsed_s'], 'error': result.get('error')}, indent=2))
    raise SystemExit(0 if result['passed'] else 1)


if __name__ == '__main__':
    main()
