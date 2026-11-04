"""Independent failure controls for the reviewed-discovery comparison audit."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'analysis'))
import benchmark_reviewed_search as audit


class ReviewedSearchAuditControls(unittest.TestCase):
    def setUp(self):
        self.selected = [
            {'harmonic_group_id': 'a', 'event_id': 'one', 'f0_hz': [100.]},
            {'harmonic_group_id': 'b', 'event_id': 'two', 'f0_hz': [150.]},
        ]
        self.discovery = {
            'source_sha256': 'a' * 64, 'source_metadata': {'selected_channel_index': 1},
            'location_verified': False, 'settings': {'max_gap_s': 2.},
            'groups': [{'harmonic_group_id': x} for x in ('a', 'b', 'c')],
            'events': [
                {'event_id': name, 'harmonic_group_id': group, 'quality': {'support_fraction': .5},
                 'ridge': [{'time_s': t + offset, 'frequency_hz': f, 'weight': 2}
                           for t, f in [(1., 110.), (2., 105.), (9., 100.)]]}
                for name, group, offset in [('one', 'a', 0), ('two', 'b', 20), ('three', 'c', 40)]],
        }
        observers = [{'id': 'origin', 'latitude_deg': 40., 'longitude_deg': -73., 'ellipsoid_height_m': 3.},
                     {'id': 'other', 'latitude_deg': 40.01, 'longitude_deg': -72.99, 'ellipsoid_height_m': 3.}]
        events = [{'points': [{'t_rec_s': point['time_s'], 'frequency_hz': point['frequency_hz']} for point in event['ridge']],
                   'f0_hz': choice['f0_hz']}
                  for event, choice in zip(self.discovery['events'][:2], self.selected, strict=True)]
        self.native = {'schema_version': 1, 'evidence_kind': 'real_recording', 'provenance': ['reviewed acoustic points'],
                       'tracks': [{'flight_id': 'first', 'points': []}, {'flight_id': 'second', 'points': []}],
                       'observers': observers, 'events': events,
                       'pair_config': {'shared_shifts_s': [-1., 0., 1.]},
                       'coarse_bounds': [{'speed_mps': [100., 200.]}] * 2,
                       'measured_separation_s': 20., 'separation_tolerance_s': 1.}
        self.template = deepcopy(self.native)
        self.template['events'][0]['points'] = [{'t_rec_s': 1., 'frequency_hz': 99.}] * 3
        self.template['events'][1]['points'] = [{'t_rec_s': 21., 'frequency_hz': 149.}] * 3
        self.row = {'observer': observers[0], 'score': {
            'flight_ids': ['first', 'second'], 'combined_rmse_hz': 2., 'clock_offset_s': 500.,
            'shared_shift_s': 0., 'rmse_hz': [2., 2.], 'fitted_f0_hz': [100., 150.]}}
        self.result = {'location_verified': False, 'evidence_kind': 'real_recording',
                       'observer_count': 2, 'track_count': 2, 'ranked_count': 1,
                       'status': 'hypotheses_ranked', 'candidates': [self.row], 'best_per_flight_pair': [self.row]}

    def test_review_is_explicitly_agent_and_preserves_all_memberships(self):
        contract = {'review': {'selected': self.selected, 'basis': 'source-informed acoustic review'}}
        review = audit.make_review(self.discovery, 'b' * 64, contract)
        self.assertEqual(review['review_actor'], 'agent')
        self.assertEqual(review['discovery_sha256'], 'b' * 64)
        self.assertEqual([x['review'] for x in review['groups']], ['keep_for_matching', 'keep_for_matching', 'unreviewed'])
        self.assertEqual([x['event_ids'] for x in review['groups']], [['one'], ['two'], ['three']])
        self.assertFalse(review['location_verified'])

    def test_exact_transfer_preserves_real_gap_and_rejects_invented_measurement(self):
        checked = audit.validate_transfer(self.native, self.template, self.discovery, self.selected)
        self.assertEqual(checked['events'][0]['max_measured_gap_s'], 7.)
        changed = deepcopy(self.native)
        changed['events'][0]['points'].insert(2, {'t_rec_s': 5., 'frequency_hz': 102.})
        with self.assertRaisesRegex(AssertionError, 'Changed, filled'):
            audit.validate_transfer(changed, self.template, self.discovery, self.selected)

    def test_dropped_or_rescaled_points_are_not_silently_accepted(self):
        for mutation in ['drop', 'rescale', 'reverse']:
            with self.subTest(mutation=mutation):
                changed = deepcopy(self.native)
                if mutation == 'drop': changed['events'][0]['points'].pop()
                if mutation == 'rescale': changed['events'][0]['points'][0]['frequency_hz'] /= 2
                if mutation == 'reverse': changed['events'][0]['points'].reverse()
                with self.assertRaises(AssertionError):
                    audit.validate_transfer(changed, self.template, self.discovery, self.selected)

    def test_search_space_or_tone_grid_retuning_fails(self):
        for field in ['tracks', 'observers', 'pair_config', 'coarse_bounds', 'measured_separation_s', 'separation_tolerance_s']:
            with self.subTest(field=field):
                changed = deepcopy(self.native)
                changed[field] = None
                with self.assertRaisesRegex(AssertionError, 'Changed inherited'):
                    audit.validate_transfer(changed, self.template, self.discovery, self.selected)
        changed = deepcopy(self.native)
        changed['events'][0]['f0_hz'] = [101.]
        with self.assertRaisesRegex(AssertionError, 'grid drifted'):
            audit.validate_transfer(changed, self.template, self.discovery, self.selected)

    def test_receipt_must_preserve_uncertainty_and_unscored_harmonics(self):
        extra = deepcopy(self.discovery['events'][0])
        extra['event_id'] = 'one-harmonic'
        self.discovery['events'].append(extra)
        selected = []
        for choice, event in zip(self.selected, self.discovery['events'][:2], strict=True):
            selected.append({**choice, 'point_count': 3, 'quality': event['quality'], 'contrast_weights': [2, 2, 2],
                             'preserved_gaps': [{'after_time_s': event['ridge'][1]['time_s'],
                                                 'before_time_s': event['ridge'][2]['time_s'], 'duration_s': 7.}],
                             'other_group_ridges_not_scored': ['one-harmonic'] if choice['event_id'] == 'one' else []})
        bridge = {'status': 'prepared_reviewed_search', 'review_actor': 'agent', 'location_verified': False,
                  'aircraft_identity_confirmed': False, 'selected_ridges': selected,
                  'provenance': {'discovery': {'sha256': 'discovery'}, 'review_selection': {'sha256': 'review'},
                                 'source': {'sha256': self.discovery['source_sha256']}},
                  'search_input': {'sha256': 'search'}}
        self.assertTrue(audit.validate_bridge_receipt(bridge, self.discovery, self.selected, 'discovery', 'review', 'search')['passed'])
        for field, value in [('quality', {}), ('contrast_weights', []), ('preserved_gaps', []),
                             ('other_group_ridges_not_scored', [])]:
            with self.subTest(field=field):
                changed = deepcopy(bridge)
                changed['selected_ridges'][0][field] = value
                with self.assertRaises(AssertionError):
                    audit.validate_bridge_receipt(changed, self.discovery, self.selected, 'discovery', 'review', 'search')

    def test_no_candidate_is_retained_as_inconclusive(self):
        result = deepcopy(self.result)
        result.update({'status': 'no_candidate', 'ranked_count': 0, 'candidates': [], 'best_per_flight_pair': []})
        self.assertEqual(audit.validate_result(result, self.native, 3)['scientific_status'], 'no_candidate')
        comparison = audit.compare_winners(result, self.result, self.native, {})
        self.assertIsNone(comparison['automatic_winner'])
        self.assertIsNone(comparison['distance_from_manual_winner_m'])
        self.assertIsNone(comparison['geographic_acceptance_gate'])
        with self.assertRaises(AssertionError): audit.validate_result(result, self.native, 0)

    def test_corrupted_or_verified_output_fails(self):
        for key, value in [('ranked_count', 99), ('location_verified', True), ('track_count', 999), ('status', 'no_candidate')]:
            with self.subTest(key=key):
                changed = deepcopy(self.result)
                changed[key] = value
                with self.assertRaises(AssertionError): audit.validate_result(changed, self.native, 0)
        for value in [float('nan'), float('inf'), -1.]:
            changed = deepcopy(self.result)
            changed['candidates'][0]['score']['combined_rmse_hz'] = value
            with self.assertRaises(AssertionError): audit.validate_result(changed, self.native, 0)

    def test_foreign_observer_or_aircraft_and_retuned_clock_fail(self):
        for area, key, value in [('observer', 'latitude_deg', 39.), ('score', 'flight_ids', ['other', 'second']),
                                 ('score', 'shared_shift_s', 2.)]:
            changed = deepcopy(self.result)
            changed['candidates'][0][area][key] = value
            with self.assertRaises(AssertionError): audit.validate_result(changed, self.native, 0)

    def test_comparison_reports_drift_without_geographic_pass(self):
        automatic = deepcopy(self.result)
        automatic['candidates'][0]['observer'] = self.native['observers'][1]
        automatic['candidates'][0]['score']['clock_offset_s'] += 3
        automatic['candidates'][0]['score']['shared_shift_s'] = 1.
        evaluation = audit.compare_winners(automatic, self.result, self.native,
                                          {'example': {'coordinate': [40., -73.], 'source': 'control only'}})
        self.assertGreater(evaluation['distance_from_manual_winner_m'], 1000.)
        self.assertEqual(evaluation['clock_difference_s'], 3.)
        self.assertTrue(evaluation['same_flight_pair'])
        self.assertTrue(evaluation['at_observer_grid_boundary'])
        self.assertTrue(evaluation['at_shared_shift_boundary'])
        self.assertIsNone(evaluation['geographic_acceptance_gate'])
        self.assertFalse(evaluation['location_verified'])

    def test_nonfinite_json_is_rejected_before_evaluation(self):
        with patch.object(Path, 'read_text', return_value='{"score":NaN}'):
            with self.assertRaisesRegex(ValueError, 'Nonfinite JSON'): audit.load_json('bad.json')


if __name__ == '__main__':
    unittest.main()
