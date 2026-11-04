"""Reject malformed or misleading discovery evaluations independently of Rust."""
from copy import deepcopy
from pathlib import Path
import sys
import unittest
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'analysis'))
import benchmark_discovery as audit


class DiscoveryEvaluationControls(unittest.TestCase):
    def setUp(self):
        self.contract = audit.load_json(audit.CONTRACT)

    @staticmethod
    def event(name='one', group='source', times=(10., 20., 30.), frequencies=(150., 130., 110.)):
        return {'event_id': name, 'harmonic_group_id': group, 'start_s': min(times), 'end_s': max(times),
                'band_hz': [min(frequencies) - 1, max(frequencies) + 1],
                'ridge': [{'time_s': t, 'frequency_hz': f, 'weight': 1.} for t, f in zip(times, frequencies, strict=True)],
                'quality': {'harmonic_count': 1, 'fragment_count': 1}}

    @staticmethod
    def prediction(events):
        return {'location_verified': False, 'events': events, 'ridge_count': len(events),
                'group_count': len({e['harmonic_group_id'] for e in events})}

    def test_one_proposal_cannot_match_two_overlapping_sources(self):
        truth = [{'start_s': 0., 'end_s': 20.}, {'start_s': 10., 'end_s': 30.}]
        matches = audit.match_intervals(truth, {'one': [{'start_s': 0., 'end_s': 30.}]}, .3)
        self.assertEqual(len(matches), 1)

    def test_matching_maximizes_cardinality_instead_of_greedy_first_match(self):
        truth = [{'start_s': 0., 'end_s': 10.}, {'start_s': 6., 'end_s': 16.}]
        groups = {'broad': [{'start_s': 0., 'end_s': 16.}], 'early': [{'start_s': 0., 'end_s': 8.}]}
        self.assertEqual(len(audit.match_intervals(truth, groups, .3)), 2)

    def test_overlapping_sources_match_best_iou_without_arbitrary_identity_swap(self):
        truth = [{'start_s': 0., 'end_s': 60.}, {'start_s': 25., 'end_s': 85.}]
        groups = {'early': [{'start_s': 5., 'end_s': 55.}], 'late': [{'start_s': 30., 'end_s': 80.}]}
        matched = {row['truth_index']: row['group_id'] for row in audit.match_intervals(truth, groups, .3)}
        self.assertEqual(matched, {0: 'early', 1: 'late'})

    def test_closed_form_frequency_agrees_with_emission_domain_derivative(self):
        source = {'center_emission_s': 35., 'speed_mps': 80., 'slant_m': 1700., 'source_frequency_hz': 126.}
        emitted = np.array([-28., -4., 0., 9., 25.])
        distance = np.sqrt(source['slant_m']**2 + (source['speed_mps'] * emitted)**2)
        arrival = source['center_emission_s'] + emitted + distance / 343
        epsilon = 1e-4
        def arrival_time(t):
            return source['center_emission_s'] + t + np.sqrt(source['slant_m']**2 + (source['speed_mps'] * t)**2) / 343
        derivative = (arrival_time(emitted + epsilon) - arrival_time(emitted - epsilon)) / (2 * epsilon)
        np.testing.assert_allclose(audit.source_frequency_at_arrival(arrival, source), source['source_frequency_hz'] / derivative, rtol=1e-9)

    def test_correct_interval_with_wrong_frequency_fails(self):
        source = {'start_s': 10., 'end_s': 80., 'center_emission_s': 35., 'speed_mps': 80., 'slant_m': 1700., 'source_frequency_hz': 126., 'harmonics': [1, 2]}
        times = [15., 25., 35., 45., 55., 65., 75.]
        frequencies = audit.source_frequency_at_arrival(times, source)
        good = self.prediction([self.event(times=times, frequencies=frequencies)])
        label = {'kind': 'harmonic', 'events': [source], 'expected_groups': 1}
        self.assertEqual(audit.evaluate_synthetic(good, label, 100, self.contract)['status'], 'passed')
        bad = self.prediction([self.event(times=times, frequencies=frequencies + 30)])
        self.assertEqual(audit.evaluate_synthetic(bad, label, 100, self.contract)['status'], 'failed')

    def test_zero_events_does_not_pass_a_positive_label(self):
        label = {'kind': 'harmonic', 'events': [{'start_s': 10., 'end_s': 80.}], 'expected_groups': 1}
        result = audit.evaluate_synthetic(self.prediction([]), label, 100, self.contract)
        self.assertEqual(result['status'], 'failed')
        self.assertEqual(result['missed_events'], 1)

    def test_filled_silent_gap_fails_even_when_group_count_is_correct(self):
        event = self.event(times=[50., 55., 64., 72., 80.], frequencies=[150., 140., 130., 120., 110.])
        event['quality']['fragment_count'] = 2
        result = audit.evaluate_synthetic(self.prediction([event]), {'kind': 'interrupted', 'expected_groups': 1}, 140, self.contract)
        self.assertFalse(result['checks']['no_invented_gap_observations'])
        self.assertEqual(result['status'], 'failed')

    def test_invalid_contracts_cannot_be_reported_as_technical_success(self):
        valid = self.prediction([self.event()])
        mutations = []
        for key, value in [('location_verified', True), ('events', None), ('ridge_count', 0), ('ridge_count', True), ('group_count', 2)]:
            bad = deepcopy(valid); bad[key] = value; mutations.append(bad)
        for key, value in [('time_s', float('nan')), ('time_s', 9.), ('frequency_hz', float('inf')), ('weight', -1.)]:
            bad = deepcopy(valid); bad['events'][0]['ridge'][1][key] = value; mutations.append(bad)
        bad = deepcopy(valid); bad['events'].append(deepcopy(bad['events'][0])); bad['ridge_count'] = 2; mutations.append(bad)
        for bad in mutations:
            with self.assertRaises(AssertionError):
                audit.validate_prediction(bad, 100)


if __name__ == '__main__':
    unittest.main()
