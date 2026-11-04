"""Predeclared bounded frequency/observer widening of a regional experiment.

No truth coordinates are inputs. Every rung reranks all supplied tracks, keeps
the newly leading pairs and previous retained pairs, and cumulatively retains
their sampled observers. Numerical stability is not location accuracy.
"""
from __future__ import annotations

import argparse
from copy import deepcopy
from datetime import datetime, timezone
import math
from pathlib import Path
import shutil
import sys
import time

from benchmark_faint import wgs84_distance_m
from refine_automatic import (GeographicRaster, assert_replayed_seed,
    boundary_diagnostics, digest, finite, grid_around, load_json,
    ordered_pair_winner, pair_search, receipt, require, run_rank,
    validate_result, write_json)

ROOT = Path(__file__).resolve().parents[1]


def widen_frequencies(grid, extra_hz, step):
    require(grid and all(finite(x) and x > 0 for x in grid), 'Invalid emitted-frequency grid')
    require(all(a < b for a, b in zip(grid, grid[1:])), 'Frequency grid must be strictly increasing')
    require(finite(extra_hz) and extra_hz >= 0 and finite(step) and step > 0, 'Invalid frequency widening')
    low, high = min(grid) - extra_hz, max(grid) + extra_hz
    require(low > 0 and math.ceil((high-low)/step) < 10000, 'Frequency expansion exceeds positive/budget domain')
    return sorted({*grid, low, high, *(i*step for i in range(math.ceil(low/step), math.floor(high/step)+1))})


def expanded_search(base, observers, extra_hz, step, previous_frequency_grids=None):
    result = deepcopy(base)
    result['observers'] = deepcopy(observers)
    for index, event in enumerate(result['events']):
        event['f0_hz'] = widen_frequencies(event['f0_hz'], extra_hz, step)
        if previous_frequency_grids is not None:
            old = previous_frequency_grids[index]
            require(all(finite(x) and min(event['f0_hz']) <= x <= max(event['f0_hz']) for x in old),
                    'Previous frequency values exceed current bounded domain')
            event['f0_hz'] = sorted(set(event['f0_hz']) | set(old))
    result['provenance'].append(f'Predeclared symmetric frequency widening by {extra_hz} Hz from original bounds; all original measurements, tracks, clock and coarse filters unchanged.')
    return result


def lattice(observers):
    require(observers, 'Empty original observer lattice')
    latitudes = sorted({o['latitude_deg'] for o in observers})
    longitudes = sorted({o['longitude_deg'] for o in observers})
    require(len(latitudes) >= 2 and len(longitudes) >= 2, 'Observer lattice needs two axes')
    require(all(finite(x) for x in latitudes + longitudes), 'Nonfinite observer coordinate')
    require(len(observers) == len(latitudes)*len(longitudes)
            and len({(o['latitude_deg'], o['longitude_deg']) for o in observers}) == len(observers),
            'Original observer lattice has missing or duplicate cells')
    steps = []
    for values in (latitudes, longitudes):
        step = (values[-1]-values[0])/(len(values)-1)
        require(step > 0 and all(abs(value-(values[0]+i*step)) < 1e-10 for i, value in enumerate(values)),
                'Original observer lattice is irregular')
        steps.append(step)
    return latitudes, longitudes, steps


def extend_observers(base_observers, rings, dem, geoid, microphone_height_m):
    require(type(rings) is int and 0 <= rings <= 100, 'Regional rings must be a bounded nonnegative integer')
    require(finite(microphone_height_m) and microphone_height_m >= 0, 'Invalid microphone height')
    latitudes, longitudes, steps = lattice(base_observers)
    require((len(latitudes)+2*rings)*(len(longitudes)+2*rings) <= 100000, 'Regional observer budget exceeded')
    result = deepcopy(base_observers)
    identifiers = {o['id'] for o in result}
    for north in range(-rings, len(latitudes)+rings):
        for east in range(-rings, len(longitudes)+rings):
            if 0 <= north < len(latitudes) and 0 <= east < len(longitudes):
                continue
            lat, lon = latitudes[0]+north*steps[0], longitudes[0]+east*steps[1]
            require(-85 < lat < 85 and -180 <= lon <= 180, 'Expanded grid exceeds supported latitude/date-line domain')
            height = dem.sample(lat, lon)+geoid.sample(lat, lon)+microphone_height_m
            require(finite(height), 'Terrain returned nonfinite height')
            name = f'expanded-r{north}-c{east}'
            require(name not in identifiers, 'Expanded observer identity collision')
            identifiers.add(name)
            result.append({'id': name, 'latitude_deg': lat, 'longitude_deg': lon, 'ellipsoid_height_m': height})
    return result


def merge_observers(*collections):
    # Input IDs are local to a grid; key the physical samples and assign new
    # deterministic IDs. Different heights at one horizontal point stay distinct.
    result, seen = [], set()
    for collection in collections:
        for observer in collection:
            key = tuple(observer[name] for name in ('latitude_deg', 'longitude_deg', 'ellipsoid_height_m'))
            require(all(finite(x) for x in key), 'Nonfinite refinement observer')
            if key in seen:
                continue
            seen.add(key)
            result.append(dict(observer, id=f'observer-{len(result):06d}'))
    return result


def footprint(observers):
    # Only call this on one complete generated lattice, never a cumulative union.
    lattice(observers)
    return {'south': min(o['latitude_deg'] for o in observers),
            'north': max(o['latitude_deg'] for o in observers),
            'west': min(o['longitude_deg'] for o in observers),
            'east': max(o['longitude_deg'] for o in observers)}


def spatial_interior(observer, footprints):
    lat, lon = observer['latitude_deg'], observer['longitude_deg']
    return any(p['south']+1e-11 < lat < p['north']-1e-11
               and p['west']+1e-11 < lon < p['east']-1e-11 for p in footprints)


def union_diagnostics(winner, search, footprints, required_spacing_m=None, newest_footprint=None):
    if winner is None:
        return {'status': 'no_candidate', 'coverage_incomplete': True}
    surrogate = deepcopy(winner)
    surrogate['observer']['id'] = 'n0-e0'
    result = boundary_diagnostics(surrogate, search, 1)
    support_edge = not spatial_interior(winner['observer'], footprints)
    resolution_footprints = footprints if required_spacing_m is None else [p for p in footprints if p.get('spacing_m', math.inf) <= required_spacing_m]
    edge = not spatial_interior(winner['observer'], resolution_footprints)
    result['at_observer_grid_boundary'] = edge
    result['at_sampled_support_boundary'] = support_edge
    result['required_spatial_spacing_m'] = required_spacing_m
    result['at_required_resolution_boundary'] = edge
    result['at_new_grid_boundary'] = not spatial_interior(winner['observer'], [newest_footprint]) if newest_footprint else None
    result['spatial_boundary_policy'] = 'Strictly interior to at least one complete footprint at the required spacing or finer; coarse support cannot close a fine-resolution edge. Union gaps and edges remain incomplete.'
    incomplete = edge or result['at_shared_shift_boundary'] or result['at_absolute_clock_boundary'] or any(result['at_emitted_frequency_boundary']) or any(result['at_coarse_filter_boundary'])
    result.update(coverage_incomplete=incomplete, status='coverage_incomplete' if incomplete else 'interior_sampled_optimum')
    return result


def compare_stages(previous, current, policy):
    one, two = previous.get('best'), current.get('best')
    result = {'previous': previous.get('name'), 'current': current.get('name'), 'stable': False}
    if not one or not two:
        result['reason'] = 'A stage has no candidate'
        return result
    a, b = one['score'], two['score']
    coordinates = [(row['observer']['latitude_deg'], row['observer']['longitude_deg']) for row in (one, two)]
    numbers = [*coordinates[0], *coordinates[1], *a['fitted_f0_hz'], *b['fitted_f0_hz'],
               a['clock_offset_s'], b['clock_offset_s'], a['shared_shift_s'], b['shared_shift_s'],
               a['combined_rmse_hz'], b['combined_rmse_hz']]
    if not all(finite(value) for value in numbers):
        result['reason'] = 'Nonfinite candidate cannot establish stability'
        return result
    location = wgs84_distance_m(*coordinates)
    result.update(same_ordered_pair=a['flight_ids'] == b['flight_ids'], location_change_m=location,
                  frequency_changes_hz=[abs(x-y) for x,y in zip(a['fitted_f0_hz'], b['fitted_f0_hz'], strict=True)],
                  clock_change_s=abs(a['clock_offset_s']-b['clock_offset_s']),
                  shared_shift_change_s=abs(a['shared_shift_s']-b['shared_shift_s']),
                  rmse_change_hz=abs(a['combined_rmse_hz']-b['combined_rmse_hz']),
                  previous_winner_boundary=previous.get('best_diagnostics', {}).get('coverage_incomplete', True),
                  current_winner_boundary=current.get('best_diagnostics', {}).get('coverage_incomplete', True))
    result['stable'] = (result['same_ordered_pair'] and location <= policy['location_m']
                        and max(result['frequency_changes_hz']) <= policy['frequency_hz']
                        and result['clock_change_s'] <= policy['clock_s']
                        and result['shared_shift_change_s'] <= policy['shared_shift_s']
                        and result['rmse_change_hz'] <= policy['rmse_hz']
                        and not result['previous_winner_boundary'] and not result['current_winner_boundary'])
    return result


def stability_summary(stages, policy):
    transitions = [compare_stages(a, b, policy) for a, b in zip(stages, stages[1:])]
    required = policy['required_transitions']
    require(type(required) is int and required >= 1, 'Stability requires a positive transition count')
    stable = len(transitions) >= required and all(t['stable'] for t in transitions[-required:])
    return {'stable': stable, 'transitions': transitions, 'required_transitions': required,
            'any_retained_pair_coverage_incomplete': any(s.get('any_retained_pair_coverage_incomplete', True) for s in stages[-1:]),
            'location_verified': False, 'global_uniqueness_established': False, 'calibrated_uncertainty': False}


def audit_frozen(hashes):
    for path, expected in hashes.items():
        require(digest(path) == expected, f'Frozen input or output changed: {path}')
    return True


def validate_contract(contract):
    require(contract.get('schema_version') == 1, 'Unsupported sensitivity contract')
    require(type(contract['top_pairs']) is int and 1 <= contract['top_pairs'] <= 10, 'Invalid retained pair count')
    require(finite(contract['timeout_s']) and contract['timeout_s'] > 0, 'Invalid command timeout')
    prior = (0., 0, 0., 0.)
    names = set()
    require(len(contract['stages']) >= 3, 'At least three expanded stages are required')
    for stage in contract['stages']:
        require(stage['name'] not in names and stage['name'].replace('-', '').isalnum(), 'Invalid or duplicate stage name')
        names.add(stage['name'])
        values = tuple(stage[k] for k in ('frequency_extra_hz', 'regional_rings', 'local_radius_m', 'fine_radius_m'))
        require(all(finite(v) and v > p for v,p in zip(values, prior, strict=True)), 'Expansion schedule must increase monotonically')
        require(type(stage['regional_rings']) is int and stage['regional_rings'] <= 100, 'Invalid ring schedule')
        for radius, spacing in ((values[2], contract['grid']['local_spacing_m']), (values[3], contract['grid']['fine_spacing_m'])):
            require(finite(spacing) and 0 < spacing <= radius and (2*math.ceil(radius/spacing)+1)**2 <= 100000, 'Refinement grid budget exceeded')
        prior = values
    require(all(finite(contract['stability'][key]) and contract['stability'][key] >= 0 for key in ('location_m','frequency_hz','clock_s','shared_shift_s','rmse_hz')), 'Invalid stability tolerance')


def run(contract_path, output):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    began = time.perf_counter()
    report = {'schema_version': 1, 'status': 'running', 'execution_passed': False,
              'started_utc': datetime.now(timezone.utc).isoformat(), 'stages': [],
              'location_verified': False, 'no_evaluation_reference_read': True,
              'interpretation': 'Bounded numerical sensitivity, conditional on supplied tracks, date, retained pairs and unchanged model; not calibrated accuracy or global uniqueness.'}
    frozen = {}
    def freeze(path):
        item = receipt(path)
        frozen[item['path']] = item['sha256']
        return item
    def save():
        write_json(output/'report.json', report, replace=True)
    try:
        contract_path = Path(contract_path).resolve(strict=True)
        contract = load_json(contract_path)
        validate_contract(contract)
        freeze(contract_path)
        files = {}
        for name, item in contract['files'].items():
            path = (ROOT/item['path']).resolve(strict=True)
            require(digest(path) == item['sha256'], f'Contract hash mismatch: {name}')
            files[name] = path
            freeze(path)
        for path in [*sorted((ROOT/'src').glob('*.rs')), *sorted((ROOT/'src').glob('*.html')),
                     ROOT/'Cargo.toml', ROOT/'Cargo.lock', Path(__file__).resolve(),
                     ROOT/'analysis/refine_automatic.py', ROOT/'analysis/import_reference.py', ROOT/'analysis/benchmark_faint.py']:
            freeze(path)
        binary = output/'geosonnar.exe'
        shutil.copyfile(files['executable'], binary)
        require(digest(binary) == contract['files']['executable']['sha256'], 'Program copy changed')
        freeze(binary)
        source, original_result = load_json(files['baseline_search']), load_json(files['baseline_regional_results'])
        baseline = load_json(files['baseline_refinement'])
        validate_result(original_result, source)
        require(baseline['execution_passed'] and baseline['location_verified'] is False, 'Invalid frozen refinement')
        baseline_best = baseline['best']
        original_best_diag = baseline['pairs'][baseline['best_pair_index']]['fine']['diagnostics'] if baseline_best else {'coverage_incomplete': True}
        report['baseline'] = {'name': 'baseline', 'best': baseline_best, 'best_diagnostics': original_best_diag,
                              'any_retained_pair_coverage_incomplete': baseline['any_retained_pair_coverage_incomplete']}
        retained, sampled, footprints, previous_best = [], {}, {}, {}
        for pair in baseline['pairs']:
            key = tuple(pair['flight_ids'])
            require(key not in retained, 'Duplicate baseline retained pair')
            retained.append(key); sampled[key] = []; footprints[key] = []
            for level in ('local', 'fine'):
                old = pair[level]
                if old and old.get('receipt'):
                    info = old['receipt']['input']
                    path = Path(info['path'])
                    require(digest(path) == info['sha256'], 'Baseline refinement input changed')
                    freeze(path)
                    search = load_json(path)
                    require(search['events'] == source['events'] and search['pair_config'] == source['pair_config'] and search['coarse_bounds'] == source['coarse_bounds'], 'Baseline refinement changed fixed assumptions')
                    require(all(search[k] == source[k] for k in ('measured_separation_s','separation_tolerance_s')), 'Baseline refinement changed separation assumptions')
                    source_tracks = {track['flight_id']: track for track in source['tracks']}
                    require(search['tracks'] == [source_tracks[identifier] for identifier in dict.fromkeys(key)], 'Baseline refinement changed selected trajectories')
                    sampled[key] = merge_observers(sampled[key], search['observers'])
                    footprints[key].append(dict(footprint(search['observers']), spacing_m=baseline['policy'][f'{level}_spacing_m' if level == 'fine' else 'spacing_m']))
            previous_best[key] = pair['fine'].get('best') if pair['fine'] else None
        plan = {'schema_version': 1, 'frozen_utc': datetime.now(timezone.utc).isoformat(),
                'contract': contract, 'input_hashes': deepcopy(frozen), 'baseline_retained_pairs': [list(k) for k in retained],
                'no_evaluation_reference_input': True, 'all_stages_required': True,
                'cumulative_refinement_observers': True, 'prior_retained_pairs_never_dropped': True,
                'track_coverage_expanded': False}
        write_json(output/'frozen-plan.json', plan)
        freeze(output/'frozen-plan.json')
        report['plan'] = receipt(output/'frozen-plan.json')
        save()
        dem, geoid = GeographicRaster(files['dem']), GeographicRaster(files['geoid'])
        grid, timeout = contract['grid'], contract['timeout_s']
        previous_regional_observers, previous_frequency_grids = source['observers'], [e['f0_hz'] for e in source['events']]
        previous_regional_best = original_result['candidates'][0]['score']['combined_rmse_hz'] if original_result['candidates'] else None
        for spec in contract['stages']:
            directory = output/spec['name']; directory.mkdir()
            observers = extend_observers(source['observers'], spec['regional_rings'], dem, geoid, grid['microphone_height_m'])
            require(all(o in observers for o in previous_regional_observers), 'Expanded regional observers are not nested')
            search = expanded_search(source, observers, spec['frequency_extra_hz'], grid['frequency_step_hz'], previous_frequency_grids)
            require(all(set(old).issubset(event['f0_hz']) for old,event in zip(previous_frequency_grids, search['events'], strict=True)), 'Expanded frequency samples are not nested')
            require(search['tracks'] == source['tracks'] and all(e['points'] == old['points'] for e,old in zip(search['events'], source['events'], strict=True)), 'Expansion altered tracks or observations')
            for key in ('pair_config', 'coarse_bounds', 'measured_separation_s', 'separation_tolerance_s'):
                require(search[key] == source[key], f'Expansion altered fixed {key}')
            stage = {'name': spec['name'], 'policy': spec, 'regional_observer_count': len(observers),
                     'frequency_bounds_hz': [[min(e['f0_hz']),max(e['f0_hz'])] for e in search['events']],
                     'frequency_grid_counts': [len(e['f0_hz']) for e in search['events']],
                     'invariants': {'tracks_unchanged': True, 'measured_points_and_gaps_unchanged': True,
                                    'clock_and_coarse_filters_unchanged': True, 'regional_observers_nested': True,
                                    'frequency_samples_nested': True},
                     'pairs': [], 'best': None, 'best_diagnostics': {'coverage_incomplete': True}}
            report['stages'].append(stage); save()
            print(f'{spec["name"]}: regional rank on {len(observers)} observers and {len(search["tracks"])} tracks', flush=True)
            regional, execution = run_rank(binary, search, directory/'regional', timeout)
            for name in ('input', 'results'):
                frozen[execution[name]['path']] = execution[name]['sha256']
            seeds = validate_result(regional, search)
            if previous_regional_best is not None:
                require(seeds and seeds[0]['score']['combined_rmse_hz'] <= previous_regional_best+1e-8, 'Nested regional search worsened previous best score')
            previous_regional_best = seeds[0]['score']['combined_rmse_hz'] if seeds else None
            stage['invariants']['regional_best_score_nondegrading'] = True
            stage.update(regional_execution=execution, regional_ranked_count=regional['ranked_count'],
                         regional_ordered_pair_count=len(seeds), newly_selected_pairs=[s['score']['flight_ids'] for s in seeds[:contract['top_pairs']]])
            by_pair = {tuple(s['score']['flight_ids']): s for s in seeds}
            for seed in seeds[:contract['top_pairs']]:
                key = tuple(seed['score']['flight_ids'])
                if key not in retained:
                    retained.append(key); sampled[key] = []; footprints[key] = []; previous_best[key] = None
            stage['retained_pairs'] = [list(k) for k in retained]
            stage['unrefined_regional_pair_count'] = len(set(by_pair)-set(retained))
            for index, key in enumerate(retained):
                require(key in by_pair, 'Previously retained pair disappeared from nested regional search')
                seed = by_pair[key]
                check = pair_search(search, key, [seed['observer']], 'Replay current regional seed before cumulative refinement')
                replay, checked = run_rank(binary, check, directory/f'pair-{index+1}'/'seed', timeout)
                assert_replayed_seed(seed, ordered_pair_winner(replay, key))
                for name in ('input', 'results'):
                    frozen[checked[name]['path']] = checked[name]['sha256']
                item = {'flight_ids': list(key), 'seed': seed, 'seed_validation': checked, 'local': None, 'fine': None}
                stage['pairs'].append(item)
                center = seed['observer']
                for level in ('local', 'fine'):
                    radius = spec[f'{level}_radius_m']; spacing = grid[f'{level}_spacing_m']
                    additions, count = grid_around(center, radius, spacing, dem, geoid, grid['microphone_height_m'])
                    newest_footprint = dict(footprint(additions), spacing_m=spacing)
                    footprints[key].append(newest_footprint)
                    before = len(sampled[key]); sampled[key] = merge_observers(sampled[key], additions)
                    refined = pair_search(search, key, sampled[key], f'Cumulative {level} footprint; new radius {radius} m, spacing {spacing} m; all previous sampled observers retained')
                    result, refinement_execution = run_rank(binary, refined, directory/f'pair-{index+1}'/level, timeout)
                    winner = ordered_pair_winner(result, key)
                    require(winner is not None, 'Nested refinement lost a previously available pair')
                    diagnostics = union_diagnostics(winner, refined, footprints[key], spacing, newest_footprint)
                    item[level] = {'best': winner, 'diagnostics': diagnostics, 'grid_center': center,
                                   'new_grid_count': count, 'previous_observer_count': before,
                                   'observer_count': len(sampled[key]), 'footprints': deepcopy(footprints[key]), 'receipt': refinement_execution}
                    for name in ('input', 'results'):
                        frozen[refinement_execution[name]['path']] = refinement_execution[name]['sha256']
                    center = winner['observer']; save()
                winner = item['fine']['best']
                if previous_best[key]:
                    require(winner['score']['combined_rmse_hz'] <= previous_best[key]['score']['combined_rmse_hz']+1e-8, 'Nested per-pair search worsened prior best score')
                previous_best[key] = winner
                print(f'{spec["name"]}: pair {index+1}/{len(retained)} RMSE {winner["score"]["combined_rmse_hz"]:.6f} Hz; boundary={item["fine"]["diagnostics"]["coverage_incomplete"]}', flush=True)
            winners = sorted((p for p in stage['pairs'] if p['fine'] and p['fine']['best']), key=lambda p:p['fine']['best']['score']['combined_rmse_hz'])
            if winners:
                stage['best'] = winners[0]['fine']['best']; stage['best_diagnostics'] = winners[0]['fine']['diagnostics']
                stage['runner_up'] = winners[1]['fine']['best'] if len(winners)>1 else None
                stage['score_gap_hz'] = winners[1]['fine']['best']['score']['combined_rmse_hz']-stage['best']['score']['combined_rmse_hz'] if len(winners)>1 else None
            stage['any_retained_pair_coverage_incomplete'] = any(p[level]['diagnostics']['coverage_incomplete'] for p in stage['pairs'] for level in ('local','fine'))
            stage['status'] = 'coverage_incomplete' if stage['any_retained_pair_coverage_incomplete'] else 'completed'
            previous_regional_observers = observers
            previous_frequency_grids = [e['f0_hz'] for e in search['events']]
            report['stability'] = stability_summary(report['stages'], contract['stability']); save()
            report['baseline_to_first_expansion'] = compare_stages(report['baseline'], report['stages'][0], contract['stability'])
        audit_frozen(frozen)
        report.update(execution_passed=True, status='stable_within_declared_limits' if report['stability']['stable'] else 'not_stabilized_within_declared_limits',
                      immutability={'passed': True, 'sha256': frozen}, all_declared_stages_completed=True)
    except BaseException as error:
        report.update(status='interrupted' if isinstance(error, KeyboardInterrupt) else 'failed', execution_passed=False,
                      error=f'{type(error).__name__}: {error}')
    report.update(elapsed_s=time.perf_counter()-began, finished_utc=datetime.now(timezone.utc).isoformat())
    save()
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--contract', type=Path, default=ROOT/'benchmarks/search_sensitivity_contract.json')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    report = run(args.contract, args.output)
    print({'status': report['status'], 'execution_passed': report['execution_passed'], 'elapsed_s': report['elapsed_s'], 'error': report.get('error')}, flush=True)
    return 0 if report['execution_passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
