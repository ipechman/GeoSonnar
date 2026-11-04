"""Independent failure controls for automatic, conditional pair refinement."""
from copy import deepcopy
from pathlib import Path
import math
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'analysis'))
import refine_automatic as audit


class ConstantRaster:
    def __init__(self, value): self.value = value
    def sample(self, lat, lon): return self.value


class AutomaticRefinementControls(unittest.TestCase):
    def setUp(self):
        self.policy = {'top_pairs': 3, 'radius_m': 600., 'spacing_m': 100.,
                       'fine_radius_m': 150., 'fine_spacing_m': 25.,
                       'timeout_s': 300., 'microphone_height_m': 3.}
        observers = [{'id': 'one', 'latitude_deg': 40., 'longitude_deg': -73., 'ellipsoid_height_m': 13.},
                     {'id': 'two', 'latitude_deg': 40.01, 'longitude_deg': -73.01, 'ellipsoid_height_m': 15.}]
        self.source = {'schema_version': 1, 'evidence_kind': 'synthetic', 'provenance': ['Control only'],
                       'tracks': [{'flight_id': key, 'points': [{'t_s': 0, 'xyz_m': [1, 2, 3]}]} for key in ('a', 'b', 'c')],
                       'observers': observers, 'events': [{'points': [{'t_rec_s': 1, 'frequency_hz': 100}], 'f0_hz': [100., 101., 102.]},
                                                          {'points': [{'t_rec_s': 11, 'frequency_hz': 150}], 'f0_hz': [150., 151., 152.]}],
                       'pair_config': {'shared_shifts_s': [-1., 0., 1.], 'clock_offset_bounds_s': [0., 1000.],
                                       'event1_cpa_recording_s': 100., 'model': {'sound_speed_mps': 343., 'max_gap_s': 30.}},
                       'coarse_bounds': [{'speed_mps': [40., 200.], 'slant_distance_m': [100., 5000.]}] * 2,
                       'measured_separation_s': 50., 'separation_tolerance_s': 1.}
        def candidate(pair, observer, rmse):
            return {'observer': deepcopy(observer), 'score': {'flight_ids': pair, 'rmse_hz': [rmse, rmse],
                    'combined_rmse_hz': rmse, 'clock_offset_s': 500., 'shared_shift_s': 0.,
                    'fitted_f0_hz': [101., 151.], 'predicted_event2_cpa_recording_s': 151.,
                    'observer_xyz_m': [1., 2., 3.], 'cpa': [
                        {'arrival_time_s': time, 'emission_time_s': time-1000./343.,
                         'speed_mps': 100., 'slant_distance_m': 1000., 'at_track_boundary': False, 'segment_index': 0}
                        for time in (600., 650.)]}}
        rows = [candidate(['a', 'b'], observers[0], 1.), candidate(['a', 'b'], observers[1], 1.1),
                candidate(['b', 'a'], observers[1], 2.), candidate(['c', 'c'], observers[0], 3.)]
        self.result = {'status': 'hypotheses_ranked', 'location_verified': False, 'evidence_kind': 'synthetic',
                       'track_count': 3, 'observer_count': 2, 'ranked_count': len(rows), 'candidates': rows,
                       'best_per_flight_pair': [rows[0], rows[2], rows[3]]}

    def test_retention_uses_distinct_ordered_pairs_from_new_results(self):
        retained = audit.retained_seeds(self.result, self.source, 3)
        self.assertEqual([x['score']['flight_ids'] for x in retained], [['a', 'b'], ['b', 'a'], ['c', 'c']])
        self.assertEqual(audit.retained_seeds(self.result, self.source, 1), [self.result['candidates'][0]])
        self.assertEqual(len(audit.retained_seeds(self.result, self.source, 10)), 3)

    def test_stale_membership_counts_scores_and_pass_filters_are_rejected(self):
        mutations = [lambda r: r.update(track_count=2), lambda r: r.update(ranked_count=1),
                     lambda r: r.update(location_verified=True), lambda r: r.update(status='no_candidate'),
                     lambda r: r['candidates'][0]['observer'].update(ellipsoid_height_m=99),
                     lambda r: r['candidates'][0]['score'].update(flight_ids=['a', 'missing']),
                     lambda r: r['candidates'][0]['score'].update(combined_rmse_hz=-1),
                     lambda r: r['candidates'][0]['score'].update(combined_rmse_hz=float('nan')),
                     lambda r: r['candidates'][0]['score'].update(shared_shift_s=2),
                     lambda r: r['candidates'][0]['score'].update(clock_offset_s=1001),
                     lambda r: r['candidates'][0]['score'].update(fitted_f0_hz=[105, 151]),
                     lambda r: r['candidates'][0]['score']['cpa'][0].update(speed_mps=300),
                     lambda r: r['candidates'][0]['score']['cpa'][0].update(at_track_boundary=True),
                     lambda r: r['candidates'][0]['score']['cpa'][1].update(arrival_time_s=652)]
        for mutation in mutations:
            changed = deepcopy(self.result)
            mutation(changed)
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                audit.validate_result(changed, self.source)

    def test_omitted_or_retuned_best_pair_list_is_not_trusted(self):
        for rows in [self.result['best_per_flight_pair'][:1], list(reversed(self.result['best_per_flight_pair'])),
                     self.result['candidates'][1:]]:
            changed = deepcopy(self.result)
            changed['best_per_flight_pair'] = deepcopy(rows)
            with self.assertRaisesRegex(ValueError, 'Best-per-pair'):
                audit.validate_result(changed, self.source)

    def test_no_candidate_remains_explicit_with_no_invented_seed(self):
        empty = deepcopy(self.result)
        empty.update(status='no_candidate', ranked_count=0, candidates=[], best_per_flight_pair=[])
        self.assertEqual(audit.retained_seeds(empty, self.source, 3), [])
        empty['best_per_flight_pair'] = self.result['best_per_flight_pair']
        with self.assertRaises(ValueError): audit.validate_result(empty, self.source)

    def test_same_aircraft_two_passes_is_one_track_and_measurements_unchanged(self):
        original = deepcopy(self.source)
        observers = [{'id': 'new', 'latitude_deg': 40.1, 'longitude_deg': -73.1, 'ellipsoid_height_m': 10.}]
        changed = audit.pair_search(self.source, ['c', 'c'], observers, 'new grid')
        self.assertEqual([t['flight_id'] for t in changed['tracks']], ['c'])
        for key in ('events', 'pair_config', 'coarse_bounds', 'measured_separation_s', 'separation_tolerance_s'):
            self.assertEqual(changed[key], self.source[key])
        changed['events'][0]['points'][0]['frequency_hz'] = 0
        self.assertEqual(self.source, original)

    def test_ordered_pair_is_not_swapped_for_a_better_other_pair(self):
        chosen = audit.ordered_pair_winner(self.result, ['b', 'a'])
        self.assertEqual(chosen, self.result['candidates'][2])
        self.assertIsNone(audit.ordered_pair_winner(self.result, ['a', 'c']))

    def test_native_seed_replay_rejects_stale_clock_cpa_and_scores(self):
        seed = self.result['candidates'][0]
        audit.assert_replayed_seed(seed, deepcopy(seed))
        for field in ('clock_offset_s', 'shared_shift_s', 'combined_rmse_hz'):
            changed = deepcopy(seed)
            changed['score'][field] += .01
            with self.assertRaisesRegex(ValueError, 'disagrees'):
                audit.assert_replayed_seed(seed, changed)
        changed = deepcopy(seed)
        changed['score']['cpa'][0]['arrival_time_s'] += .1
        with self.assertRaises(ValueError): audit.assert_replayed_seed(seed, changed)
        with self.assertRaises(ValueError): audit.assert_replayed_seed(seed, None)

    def test_new_grid_is_centered_on_new_seed_and_uses_both_height_datums(self):
        center = self.source['observers'][0]
        observers, count = audit.grid_around(center, 600, 100, ConstantRaster(25), ConstantRaster(-15), 3)
        self.assertEqual((len(observers), count), (169, 6))
        middle = next(x for x in observers if x['id'] == 'n0-e0')
        self.assertEqual(middle['latitude_deg'], center['latitude_deg'])
        self.assertEqual(middle['longitude_deg'], center['longitude_deg'])
        self.assertTrue(all(x['ellipsoid_height_m'] == 13 for x in observers))
        east = next(x for x in observers if x['id'] == 'n0-e1')
        north = next(x for x in observers if x['id'] == 'n1-e0')
        self.assertGreater(east['longitude_deg'], middle['longitude_deg'])
        self.assertEqual(east['latitude_deg'], middle['latitude_deg'])
        self.assertGreater(north['latitude_deg'], middle['latitude_deg'])
        self.assertEqual(north['longitude_deg'], middle['longitude_deg'])

    def test_missing_terrain_is_never_replaced_with_zero_or_clamped(self):
        with self.assertRaisesRegex(ValueError, 'nonfinite'):
            audit.grid_around(self.source['observers'][0], 600, 100, ConstantRaster(float('nan')), ConstantRaster(0), 3)
        missing = ConstantRaster(0)
        with patch.object(missing, 'sample', side_effect=ValueError('outside raster support')):
            with self.assertRaisesRegex(ValueError, 'outside raster'):
                audit.grid_around(self.source['observers'][0], 600, 100, missing, ConstantRaster(0), 3)
        center = dict(self.source['observers'][0], longitude_deg=179.999)
        with self.assertRaisesRegex(ValueError, 'date-line'):
            audit.grid_around(center, 600, 100, ConstantRaster(0), ConstantRaster(0), 3)

    def test_grid_clock_and_frequency_edges_remain_incomplete(self):
        winner = deepcopy(self.result['candidates'][0])
        winner['observer']['id'] = 'n0-e0'
        self.assertFalse(audit.boundary_diagnostics(winner, self.source, 6)['coverage_incomplete'])
        for target in ('grid', 'clock', 'frequency', 'absolute_clock', 'coarse'):
            changed = deepcopy(winner)
            if target == 'grid': changed['observer']['id'] = 'n6-e0'
            elif target == 'clock': changed['score']['shared_shift_s'] = -1.
            elif target == 'frequency': changed['score']['fitted_f0_hz'][0] = 100.
            elif target == 'absolute_clock': changed['score']['clock_offset_s'] = 0.
            else: changed['score']['cpa'][0]['speed_mps'] = 40.
            with self.subTest(target=target):
                self.assertTrue(audit.boundary_diagnostics(changed, self.source, 6)['coverage_incomplete'])
        self.assertTrue(audit.boundary_diagnostics(None, self.source, 6)['coverage_incomplete'])

    def test_fixed_tones_are_declared_as_fixed_not_search_edge(self):
        source = deepcopy(self.source)
        source['pair_config']['shared_shifts_s'] = [0.]
        source['events'][0]['f0_hz'] = [101.]
        source['events'][1]['f0_hz'] = [151.]
        winner = deepcopy(self.result['candidates'][0])
        winner['observer']['id'] = 'n0-e0'
        checked = audit.boundary_diagnostics(winner, source, 6)
        self.assertTrue(checked['shared_shift_fixed'])
        self.assertEqual(checked['emitted_frequencies_fixed'], [True, True])
        self.assertFalse(checked['coverage_incomplete'])

    def test_invalid_policy_and_nonfinite_json_are_rejected(self):
        audit.validate_policy(self.policy)
        for key, value in [('top_pairs', 0), ('top_pairs', True), ('radius_m', float('inf')),
                           ('spacing_m', .001), ('fine_spacing_m', 200.), ('timeout_s', -1.),
                           ('microphone_height_m', float('nan'))]:
            changed = dict(self.policy, **{key: value})
            with self.subTest(key=key, value=value), self.assertRaises(ValueError): audit.validate_policy(changed)
        with patch.object(Path, 'read_text', return_value='{"value":NaN}'):
            with self.assertRaises(ValueError): audit.load_json('bad.json')


if __name__ == '__main__':
    unittest.main()
