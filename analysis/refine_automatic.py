"""Refine retained pairs from a new native search on their own terrain grids.

No evaluation coordinate, manual trace, previous pair or previous fine grid is
accepted. All retained ordered pairs receive local and fine searches. This
finite refinement remains conditional on regional coverage and pair retention.
Terrain sampling currently uses the existing Python GeoTIFF adapter; scoring
and propagation stay in the portable Rust executable.
"""
from __future__ import annotations

import argparse
from copy import deepcopy
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

from import_reference import GeographicRaster

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def receipt(path):
    path = Path(path).resolve(strict=True)
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}


def load_json(path):
    def invalid(value):
        raise ValueError(f'Nonfinite JSON number: {value}')
    return json.loads(Path(path).read_text(encoding='utf-8-sig'), parse_constant=invalid)


def write_json(path, value, *, replace=False):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = 'w' if replace else 'x'
    with path.open(mode, encoding='utf-8') as stream:
        json.dump(value, stream, indent=2, allow_nan=False)
        stream.write('\n')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def validate_policy(policy):
    require(isinstance(policy['top_pairs'], int) and not isinstance(policy['top_pairs'], bool)
            and 1 <= policy['top_pairs'] <= 10, 'Retain 1..10 distinct ordered flight pairs')
    for name in ('radius_m', 'spacing_m', 'fine_radius_m', 'fine_spacing_m', 'timeout_s'):
        require(finite(policy[name]) and policy[name] > 0, f'{name} must be finite and positive')
    require(finite(policy['microphone_height_m']) and policy['microphone_height_m'] >= 0,
            'Microphone height must be finite and nonnegative')
    for radius, spacing in [('radius_m', 'spacing_m'), ('fine_radius_m', 'fine_spacing_m')]:
        require(policy[spacing] <= policy[radius], 'Grid spacing must not exceed its declared radius')
        require((2 * math.ceil(policy[radius] / policy[spacing]) + 1) ** 2 <= 100_000,
                'Refinement grid exceeds 100,000 observer points')


def validate_result(result, search):
    """Check membership and consistency before accepting any retained seed."""
    require(result.get('location_verified') is False, 'A rank result must remain unverified')
    require(result.get('evidence_kind') == search.get('evidence_kind'), 'Result evidence kind differs from input')
    require(search.get('schema_version') == 1 and search.get('evidence_kind') in ('real_recording', 'synthetic'),
            'Unsupported search input schema or evidence kind')
    tracks = {track['flight_id']: track for track in search['tracks']}
    observers = {observer['id']: observer for observer in search['observers']}
    require(len(tracks) == len(search['tracks']) and tracks, 'Search track identities must be unique and nonempty')
    require(len(observers) == len(search['observers']) and observers, 'Search observer identities must be unique and nonempty')
    require(result.get('track_count') == len(tracks) and result.get('observer_count') == len(observers),
            'Result track/observer count differs from search input')
    rows = result.get('candidates')
    require(isinstance(rows, list) and result.get('ranked_count') == len(rows), 'Ranked row count is inconsistent')
    require(result.get('status') == ('hypotheses_ranked' if rows else 'no_candidate'), 'Search status contradicts candidate rows')
    first_per_pair, previous = {}, -math.inf
    for row in rows:
        score, observer = row['score'], row['observer']
        require(observer == observers.get(observer['id']), 'Candidate observer does not belong to supplied input')
        ids = score.get('flight_ids')
        require(isinstance(ids, list) and len(ids) == 2 and all(x in tracks for x in ids),
                'Candidate ordered pair does not belong to supplied input')
        for name in ('rmse_hz', 'fitted_f0_hz', 'cpa'):
            require(isinstance(score.get(name), list) and len(score[name]) == 2, f'Candidate requires two {name} values')
        numbers = [score.get('combined_rmse_hz'), score.get('clock_offset_s'), score.get('shared_shift_s'),
                   score.get('predicted_event2_cpa_recording_s'), *score['rmse_hz'], *score['fitted_f0_hz']]
        require(all(finite(x) for x in numbers), 'Candidate has a nonfinite or invalid score')
        require(all(x >= 0 for x in score['rmse_hz']) and score['combined_rmse_hz'] >= previous,
                'Candidate errors must be nonnegative and sorted')
        require(math.isclose(score['combined_rmse_hz'], math.hypot(*score['rmse_hz']) / math.sqrt(2), rel_tol=1e-10, abs_tol=1e-10),
                'Combined score does not match equal-event RMSE')
        require(score['shared_shift_s'] in search['pair_config']['shared_shifts_s'], 'Candidate uses an undeclared clock shift')
        clock_bounds = search['pair_config'].get('clock_offset_bounds_s')
        if clock_bounds is not None:
            require(clock_bounds[0] <= score['clock_offset_s'] <= clock_bounds[1], 'Candidate clock lies outside the declared time interval')
        for i, cpa in enumerate(score['cpa']):
            require(cpa.get('at_track_boundary') is False, 'A truncated pass cannot seed a refinement')
            require(all(finite(cpa.get(k)) for k in ('arrival_time_s', 'emission_time_s', 'slant_distance_m', 'speed_mps')),
                    'Candidate CPA contains invalid numbers')
            require(score['fitted_f0_hz'][i] in search['events'][i]['f0_hz'], 'Candidate emitted tone lies outside the explicit grid')
            bounds = search['coarse_bounds'][i]
            require(bounds['speed_mps'][0] <= cpa['speed_mps'] <= bounds['speed_mps'][1]
                    and bounds['slant_distance_m'][0] <= cpa['slant_distance_m'] <= bounds['slant_distance_m'][1],
                    'Candidate CPA violates source coarse filters')
        require(score['cpa'][0]['arrival_time_s'] < score['cpa'][1]['arrival_time_s'], 'Candidate passes are not time ordered')
        separation = score['cpa'][1]['arrival_time_s'] - score['cpa'][0]['arrival_time_s']
        require(abs(separation - search['measured_separation_s']) <= search['separation_tolerance_s'] + 1e-8,
                'Candidate pass separation differs from declared selection')
        previous = score['combined_rmse_hz']
        first_per_pair.setdefault(tuple(ids), row)
    require(result.get('best_per_flight_pair') == list(first_per_pair.values()),
            'Best-per-pair list is not the sorted first row of each ordered pair')
    return list(first_per_pair.values())


def retained_seeds(result, search, top_pairs):
    return validate_result(result, search)[:top_pairs]


def pair_search(source, flight_ids, observers, note):
    tracks = {track['flight_id']: track for track in source['tracks']}
    # The same aircraft may make two separate passes. It remains one track.
    identities = list(dict.fromkeys(flight_ids))
    require(len(flight_ids) == 2 and all(identity in tracks for identity in identities), 'Missing seed trajectory')
    search = deepcopy({key: value for key, value in source.items() if key not in ('tracks', 'observers')})
    search['tracks'] = [deepcopy(tracks[identity]) for identity in identities]
    search['observers'] = deepcopy(observers)
    search['provenance'].append(note)
    return search


def ordered_pair_winner(result, flight_ids):
    return next((row for row in result['candidates'] if row['score']['flight_ids'] == list(flight_ids)), None)


def assert_replayed_seed(seed, replay):
    require(replay is not None, 'Selected regional seed could not be reproduced by the native matcher')
    require(replay['observer'] == seed['observer'] and replay['score']['flight_ids'] == seed['score']['flight_ids'],
            'Native replay changed the selected observer or ordered pair')
    # Same executable and input floats should be deterministic. A tiny absolute
    # tolerance permits JSON decimal round trips, not a different prediction.
    def compare(left, right):
        if isinstance(left, dict):
            return isinstance(right, dict) and left.keys() == right.keys() and all(compare(v, right[k]) for k, v in left.items())
        if isinstance(left, list):
            return isinstance(right, list) and len(left) == len(right) and all(compare(a, b) for a, b in zip(left, right, strict=True))
        if finite(left):
            return finite(right) and math.isclose(left, right, rel_tol=1e-12, abs_tol=1e-9)
        return type(left) is type(right) and left == right
    require(compare(seed['score'], replay['score']), 'Native seed replay disagrees with supplied regional score/clock/CPA')


def grid_around(center, radius_m, spacing_m, dem, geoid, microphone_height_m):
    lat0, lon0 = center['latitude_deg'], center['longitude_deg']
    require(finite(lat0) and finite(lon0) and -85 < lat0 < 85 and -180 <= lon0 <= 180,
            'Use a local WGS84 seed within +/-85 latitude and valid longitude')
    latitude = math.radians(lat0)
    e2 = 6.6943799901413165e-3
    denominator = 1 - e2 * math.sin(latitude) ** 2
    lat_m = 6378137 * (1-e2) / denominator ** 1.5 * math.pi / 180
    lon_m = 6378137 / math.sqrt(denominator) * math.cos(latitude) * math.pi / 180
    count = math.ceil(radius_m / spacing_m)
    require((2 * count + 1) ** 2 <= 100_000, 'Grid exceeds observer budget')
    observers = []
    for north in range(-count, count + 1):
        for east in range(-count, count + 1):
            lat = lat0 + north * spacing_m / lat_m
            lon = lon0 + east * spacing_m / lon_m
            require(-85 < lat < 85 and -180 <= lon <= 180,
                    'Refinement crosses a supported latitude/date-line limit; split the region explicitly')
            height = dem.sample(lat, lon) + geoid.sample(lat, lon) + microphone_height_m
            require(finite(height), 'Terrain returned a nonfinite observer height')
            observers.append({'id': f'n{north}-e{east}', 'latitude_deg': lat,
                              'longitude_deg': lon, 'ellipsoid_height_m': height})
    return observers, count


def boundary_diagnostics(winner, search, grid_count):
    if winner is None:
        return {'status': 'no_candidate', 'coverage_incomplete': True}
    north, east = winner['observer']['id'][1:].split('-e')
    grid_edge = abs(int(north)) == grid_count or abs(int(east)) == grid_count
    score = winner['score']
    shifts = search['pair_config']['shared_shifts_s']
    fixed_clock = len(set(shifts)) == 1
    shift_edge = not fixed_clock and score['shared_shift_s'] in (min(shifts), max(shifts))
    tone_edges, fixed_tones = [], []
    for event, fitted in zip(search['events'], score['fitted_f0_hz'], strict=True):
        grid = event['f0_hz']
        fixed = len(set(grid)) == 1
        fixed_tones.append(fixed)
        tone_edges.append(not fixed and fitted in (min(grid), max(grid)))
    clock = search['pair_config'].get('clock_offset_bounds_s')
    absolute_clock_edge = clock is not None and (abs(score['clock_offset_s']-clock[0]) < 1e-7 or abs(score['clock_offset_s']-clock[1]) < 1e-7)
    coarse_edges = []
    for cpa, bounds in zip(score['cpa'], search['coarse_bounds'], strict=True):
        coarse_edges.append(any(math.isclose(cpa[key], edge, rel_tol=1e-8, abs_tol=1e-7)
                                for key in ('speed_mps', 'slant_distance_m') for edge in bounds[key]))
    incomplete = grid_edge or shift_edge or any(tone_edges) or absolute_clock_edge or any(coarse_edges)
    return {'status': 'coverage_incomplete' if incomplete else 'interior_sampled_optimum',
            'coverage_incomplete': incomplete, 'at_observer_grid_boundary': grid_edge,
            'at_shared_shift_boundary': shift_edge, 'at_absolute_clock_boundary': absolute_clock_edge,
            'at_emitted_frequency_boundary': tone_edges, 'at_coarse_filter_boundary': coarse_edges,
            'shared_shift_fixed': fixed_clock, 'emitted_frequencies_fixed': fixed_tones,
            'calibrated_uncertainty': False}


def run_rank(exe, search, directory, timeout_s):
    directory.mkdir(parents=True, exist_ok=False)
    input_path, result_path = directory/'input.json', directory/'results.json'
    write_json(input_path, search)
    arguments = [str(exe), 'rank', '--input', str(input_path), '--out', str(result_path)]
    execution = {'command': arguments, 'timeout_s': timeout_s, 'input': receipt(input_path)}
    began = time.perf_counter()
    try:
        with (directory/'stdout.txt').open('wb') as stdout, (directory/'stderr.txt').open('wb') as stderr:
            result = subprocess.run(arguments, cwd=ROOT, stdout=stdout, stderr=stderr, timeout=timeout_s,
                                    creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0), check=False)
        execution['returncode'] = result.returncode
        require(result.returncode in (0, 3) and result_path.is_file(), 'Native rank failed; inspect this run\'s stdout/stderr')
        result = load_json(result_path)
        validate_result(result, search)
        require((execution['returncode'] == 3) == (result['status'] == 'no_candidate'), 'Native rank exit/status contradiction')
        require(digest(input_path) == execution['input']['sha256'], 'Search input changed while rank ran')
        execution.update({'status': 'completed', 'results': receipt(result_path)})
    except BaseException as error:
        execution.update({'status': 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed',
                          'error': f'{type(error).__name__}: {error}'})
        raise
    finally:
        execution['elapsed_s'] = time.perf_counter() - began
        write_json(directory/'execution.json', execution)
    return result, execution


def refine(args):
    output = Path(args.out_dir).resolve()
    output.mkdir(parents=True, exist_ok=False)
    began = time.perf_counter()
    report = {'schema_version': 1, 'status': 'running', 'execution_passed': False,
              'started_utc': datetime.now(timezone.utc).isoformat(), 'location_verified': False,
              'conditional_on_retained_pairs': True, 'seed_validation': [], 'pairs': [], 'best': None,
              'best_pair_index': None, 'independent_location_accuracy_established': False}
    frozen = {}
    try:
        policy = {key: getattr(args, key) for key in ('top_pairs', 'radius_m', 'spacing_m', 'fine_radius_m',
                  'fine_spacing_m', 'timeout_s', 'microphone_height_m')}
        validate_policy(policy)
        paths = {key: Path(getattr(args, key)).resolve(strict=True) for key in ('input', 'coarse_results', 'exe', 'dem', 'geoid')}
        paths['runner'] = Path(__file__).resolve()
        paths['terrain_adapter'] = ROOT/'analysis'/'import_reference.py'
        inputs = {key: receipt(path) for key, path in paths.items()}
        frozen.update({str(path): inputs[key]['sha256'] for key, path in paths.items()})
        source, coarse = load_json(paths['input']), load_json(paths['coarse_results'])
        all_seeds = validate_result(coarse, source)
        seeds = all_seeds[:policy['top_pairs']]
        binary = output/'geosonnar.exe'
        shutil.copyfile(paths['exe'], binary)
        frozen[str(binary)] = digest(binary)
        require(frozen[str(binary)] == inputs['exe']['sha256'], 'Executable changed while snapshotting')
        plan = {'schema_version': 1, 'policy': policy, 'inputs': inputs, 'executable_snapshot': receipt(binary),
                'environment': {'python': platform.python_version(), 'platform': platform.platform()},
                'selected_seeds': seeds, 'available_ordered_pairs': len(all_seeds),
                'omitted_ordered_pairs': len(all_seeds)-len(seeds),
                'selection': 'First distinct ordered pairs in the new regional ranking; no external pair or coordinate',
                'seed_validation': 'Native re-evaluation at exact seed observer and deduplicated seed tracks, before local grids',
                'no_evaluation_reference_input': True, 'automatic_boundary_expansion': False,
                'scope': 'Conditional local and fine refinement of every retained pair, not exhaustive regional/global uniqueness'}
        write_json(output/'plan.json', plan)
        frozen[str(output/'plan.json')] = digest(output/'plan.json')
        report['plan'] = receipt(output/'plan.json')
        report['policy'] = policy
        report['limitations'] = [
            'The search region, date, sound speed, source tracks and terrain remain supplied assumptions.',
            'Only retained ordered pairs and their sampled local footprints are refined; omitted pairs or distant locations may win.',
            'Seed replay checks compatibility with the declared input/executable, not completeness of the original regional search or flight archive.',
            'Native rank may enumerate different local passes of the same retained tracks; all such ordered-pair alternatives within the declared clock/filter bounds remain eligible.',
            'A grid, clock, emitted-frequency or coarse-filter edge is incomplete coverage; no automatic parameter widening is performed.',
            'An interior sampled optimum does not establish continuous uniqueness or calibrated uncertainty.',
            'Partial acoustic measurements, unscored harmonics and missed events remain limitations.',
            'GeoTIFF terrain import still uses Python; numeric scoring and propagation run in Rust.']
        write_json(output/'report.json', report)
        for index, seed in enumerate(seeds):
            ids = seed['score']['flight_ids']
            check = pair_search(source, ids, [seed['observer']], 'Native replay of a retained regional seed before refinement; no external reference')
            replay, execution = run_rank(binary, check, output/f'seed-{index+1}', policy['timeout_s'])
            winner = ordered_pair_winner(replay, ids)
            assert_replayed_seed(seed, winner)
            report['seed_validation'].append({'pair_index': index, 'flight_ids': ids, 'passed': True, 'receipt': execution})
            frozen[execution['input']['path']] = execution['input']['sha256']
            frozen[execution['results']['path']] = execution['results']['sha256']
            write_json(output/'report.json', report, replace=True)
        if seeds:
            dem, geoid = GeographicRaster(paths['dem']), GeographicRaster(paths['geoid'])
            report['terrain'] = {'dem': dem.provenance(), 'geoid': geoid.provenance(), 'microphone_height_m': policy['microphone_height_m']}
        for index, seed in enumerate(seeds):
            ids = seed['score']['flight_ids']
            item = {'pair_index': index, 'flight_ids': ids, 'seed': seed, 'local': None, 'fine': None}
            report['pairs'].append(item)
            local_center = seed['observer']
            for stage in ('local', 'fine'):
                if stage == 'fine' and item['local']['best'] is None:
                    item['fine'] = {'status': 'not_run_no_local_candidate', 'best': None,
                                    'diagnostics': {'status': 'no_candidate', 'coverage_incomplete': True}}
                    break
                radius, spacing = ((policy['radius_m'], policy['spacing_m']) if stage == 'local'
                                   else (policy['fine_radius_m'], policy['fine_spacing_m']))
                observers, count = grid_around(local_center, radius, spacing, dem, geoid, policy['microphone_height_m'])
                search = pair_search(source, ids, observers,
                    f'Automatic {stage} grid around retained pair {index+1} own {"regional" if stage == "local" else "local"} winner; radius={radius}m, spacing={spacing}m; no reference coordinate')
                result, execution = run_rank(binary, search, output/f'pair-{index+1}'/stage, policy['timeout_s'])
                winner = ordered_pair_winner(result, ids)
                diagnostics = boundary_diagnostics(winner, search, count)
                item[stage] = {'status': result['status'], 'best': winner, 'receipt': execution,
                               'grid_center': local_center, 'grid_count': count, 'observer_count': len(observers),
                               'ranked_count': result['ranked_count'],
                               'ordered_pair_candidate_count': sum(row['score']['flight_ids'] == ids for row in result['candidates']),
                               'diagnostics': diagnostics}
                frozen[execution['input']['path']] = execution['input']['sha256']
                frozen[execution['results']['path']] = execution['results']['sha256']
                if winner:
                    local_center = winner['observer']
                write_json(output/'report.json', report, replace=True)
                print(json.dumps({'pair_index': index, 'stage': stage, 'status': result['status'],
                                  'best_rmse_hz': winner['score']['combined_rmse_hz'] if winner else None,
                                  'coverage_incomplete': diagnostics['coverage_incomplete']}), flush=True)
        winners = [(item['fine']['best']['score']['combined_rmse_hz'], item['pair_index'], item['fine']['best'])
                   for item in report['pairs'] if item['fine'] and item['fine']['best']]
        if winners:
            _, report['best_pair_index'], report['best'] = min(winners, key=lambda row: (row[0], row[1]))
            incomplete = any(item[stage]['diagnostics']['coverage_incomplete']
                             for item in report['pairs'] for stage in ('local', 'fine'))
            report['status'] = 'coverage_incomplete' if incomplete else 'completed'
            report['any_retained_pair_coverage_incomplete'] = incomplete
        else:
            report['status'] = 'no_candidate'
            report['any_retained_pair_coverage_incomplete'] = True
        for path, expected in frozen.items():
            require(digest(path) == expected, f'Frozen input/output changed during refinement: {path}')
        report['immutability'] = {'passed': True, 'sha256': frozen}
        report['execution_passed'] = True
    except BaseException as error:
        report.update({'status': 'interrupted' if isinstance(error, KeyboardInterrupt) else 'failed',
                       'execution_passed': False, 'error': f'{type(error).__name__}: {error}'})
    report['elapsed_s'] = time.perf_counter()-began
    report['finished_utc'] = datetime.now(timezone.utc).isoformat()
    write_json(output/'report.json', report, replace=True)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('input', 'coarse-results', 'exe', 'out-dir', 'dem', 'geoid'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--microphone-height-m', type=float, default=3)
    parser.add_argument('--top-pairs', type=int, default=3)
    parser.add_argument('--radius-m', type=float, default=600)
    parser.add_argument('--spacing-m', type=float, default=100)
    parser.add_argument('--fine-radius-m', type=float, default=150)
    parser.add_argument('--fine-spacing-m', type=float, default=25)
    parser.add_argument('--timeout-s', type=float, default=300)
    args = parser.parse_args()
    try:
        result = refine(args)
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(json.dumps({'status': result['status'], 'execution_passed': result['execution_passed'],
                      'report': str(args.out_dir/'report.json'), 'elapsed_s': result['elapsed_s'],
                      'error': result.get('error')}, indent=2), flush=True)
    return 0 if result['execution_passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
