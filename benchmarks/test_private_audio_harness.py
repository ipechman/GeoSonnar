"""Independent controls for the private runner, using fabricated data only.

These tests never load a private case or recording. The end-to-end control
simulates native output in an isolated temporary project; no detector or
location result from a user's recording is an expected test outcome.
"""
from copy import deepcopy
from itertools import combinations, permutations
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'analysis'))
sys.path.insert(0, str(ROOT / 'benchmarks'))
import validate_private_audio as audit


DISCOVERY_DEFAULTS = {
    'chunk_s': 120., 'overlap_s': 60., 'min_frequency_hz': 40.,
    'max_frequency_hz': 1600., 'window_s': 2.048, 'hop_s': .5,
    'min_contrast': 2.5, 'min_duration_s': 12., 'min_support': .65,
    'min_drop_hz': 4., 'min_fractional_drop': .03, 'max_gap_s': 2.,
    'max_fragment_gap_s': 20., 'max_peaks_per_frame': 60,
}


def event(name, group=None, start=10., end=30., count=3, contrast=3.):
    times = [start + (end - start) * i / (count - 1) for i in range(count)]
    frequencies = [150. - 40. * i / (count - 1) for i in range(count)]
    return {
        'event_id': name, 'harmonic_group_id': group or name,
        'start_s': start, 'end_s': end, 'band_hz': [109., 151.],
        'ridge': [{'time_s': t, 'frequency_hz': f, 'weight': contrast}
                  for t, f in zip(times, frequencies, strict=True)],
        'quality': {'harmonic_count': 1, 'fragment_count': 1,
                    'median_contrast': contrast, 'frequency_sigma_hz': .5},
    }


def source_receipt(path=None):
    return {'path': str(path or ROOT / 'fabricated-input.bin'),
            'bytes': 100, 'sha256': 'a' * 64}


def prediction(events=(), source=None, channel=0, channels=2, duration=100.):
    source = source or source_receipt()
    groups = {}
    for item in events:
        groups.setdefault(item['harmonic_group_id'], []).append(item)
    return {
        'schema_version': 1, 'source': source['path'],
        'source_sha256': source['sha256'], 'settings': deepcopy(DISCOVERY_DEFAULTS),
        'source_metadata': {
            'selected_audio_stream': '0:a:0', 'selected_channel_index': channel,
            'available_channels': channels, 'channels_averaged': False,
            'sample_rate_hz': 4000, 'decoded_samples': round(duration * 4000),
            'duration_s': duration,
        },
        'status': 'events_proposed' if groups else 'no_events',
        'location_verified': False, 'events': list(events),
        'ridge_count': len(events), 'group_count': len(groups),
        'groups': [{
            'harmonic_group_id': name, 'review_status': 'unreviewed',
            'aircraft_identity_confirmed': False, 'ridge_count': len(members),
            'start_s': min(e['start_s'] for e in members),
            'end_s': max(e['end_s'] for e in members),
        } for name, members in groups.items()],
    }


class RepresentativeAndPairControls(unittest.TestCase):
    def test_representative_prefers_samples_then_contrast_then_lexical_id(self):
        members = [event('few', 'group', count=3, contrast=99.),
                   event('weak', 'group', count=4, contrast=2.),
                   event('z', 'group', count=4, contrast=4.),
                   event('a', 'group', count=4, contrast=4.)]
        before = deepcopy(members)
        for order in permutations(members):
            selected = audit.representatives({'group': list(order)})
            self.assertEqual([e['event_id'] for e in selected], ['a'])
            self.assertIs(selected[0], members[-1])
        self.assertEqual(members, before, 'Selection must not pool, scale, or edit measured points')

    def test_representatives_are_chronological_independent_of_group_order(self):
        early, late = event('early', start=1., end=9.), event('late', start=40., end=60.)
        self.assertEqual(audit.representatives({'late': [late], 'early': [early]}), [early, late])

    def test_odd_number_reuses_a_partner_without_omitting_a_group(self):
        items = [event('a', start=0., end=10.), event('b', start=20., end=30.),
                 event('c', start=40., end=50.)]
        before = deepcopy(items)
        for order in permutations(items):
            pairs, unpairable = audit.pair_plan(list(order))
            self.assertEqual([[e['event_id'] for e in pair] for pair in pairs], [['a', 'b'], ['a', 'c']])
            self.assertEqual(unpairable, [])
        self.assertEqual(items, before)

    def test_pair_compatibility_uses_actual_samples_and_strict_separation(self):
        one = event('one', start=10., end=20.)
        two = event('two', start=30., end=40.)
        # Broad discovery windows overlap; the measured intervals do not.
        one.update(start_s=0., end_s=50.)
        two.update(start_s=0., end_s=50.)
        self.assertEqual(audit.pair_plan([two, one]), ([[one, two]], []))
        touching = event('touching', start=20., end=30.)
        self.assertEqual(audit.pair_plan([one, touching]), ([], [one, touching]))

    def test_every_pairable_group_is_accounted_for_for_varied_overlaps(self):
        intervals = [(0., 10.), (5., 15.), (10., 20.), (16., 25.), (30., 40.), (0., 45.)]
        inventory = [event(str(i), start=start, end=end) for i, (start, end) in enumerate(intervals)]
        for size in range(len(inventory) + 1):
            for subset in combinations(inventory, size):
                with self.subTest(ids=[e['event_id'] for e in subset]):
                    pairs, unpairable = audit.pair_plan(list(subset))
                    covered = {e['event_id'] for pair in pairs for e in pair}
                    # Independent interval predicate, without calling compatible().
                    expected = {a['event_id'] for a in subset if any(
                        a['event_id'] != b['event_id'] and
                        (a['ridge'][-1]['time_s'] < b['ridge'][0]['time_s'] or
                         b['ridge'][-1]['time_s'] < a['ridge'][0]['time_s']) for b in subset)}
                    self.assertEqual(covered, expected)
                    self.assertEqual({e['event_id'] for e in unpairable}, {e['event_id'] for e in subset} - expected)
                    self.assertEqual(audit.pair_plan(list(reversed(subset))), (pairs, unpairable))
                    for left, right in pairs:
                        self.assertNotEqual(left['harmonic_group_id'], right['harmonic_group_id'])
                        self.assertLess(left['ridge'][-1]['time_s'], right['ridge'][0]['time_s'])

    def test_empty_single_and_duplicate_group_controls(self):
        one = event('one')
        self.assertEqual(audit.pair_plan([]), ([], []))
        self.assertEqual(audit.pair_plan([one]), ([], [one]))
        with self.assertRaisesRegex(AssertionError, 'Duplicate group'):
            audit.pair_plan([one, event('two', group='one', start=40., end=60.)])


class PrivateArtifactControls(unittest.TestCase):
    def test_valid_discovery_and_empty_discovery_are_technical_inputs(self):
        sample = event('one')
        self.assertEqual(audit.validate_discovery(prediction([sample]), source_receipt(), 0, 2, 100.), {'one': [sample]})
        self.assertEqual(audit.validate_discovery(prediction(), source_receipt(), 0, 2, 100.), {})

    def test_foreign_hash_path_and_channel_are_rejected(self):
        for key, value in [('source_sha256', 'b' * 64), ('source', str(ROOT / 'different-input.bin'))]:
            bad = prediction()
            bad[key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                audit.validate_discovery(bad, source_receipt(), 0, 2, 100.)
        for key, value in [('selected_audio_stream', '0:a:1'), ('selected_channel_index', 1),
                           ('available_channels', 1), ('channels_averaged', True)]:
            bad = prediction()
            bad['source_metadata'][key] = value
            with self.subTest(key=key), self.assertRaises(AssertionError):
                audit.validate_discovery(bad, source_receipt(), 0, 2, 100.)

    def test_tuned_or_missing_discovery_settings_are_rejected(self):
        self.assertEqual(audit.DISCOVERY_DEFAULTS, DISCOVERY_DEFAULTS)
        for name in DISCOVERY_DEFAULTS:
            for remove in (False, True):
                bad = prediction()
                if remove:
                    del bad['settings'][name]
                else:
                    bad['settings'][name] += 1
                with self.subTest(setting=name, remove=remove), self.assertRaisesRegex(AssertionError, 'defaults changed'):
                    audit.validate_discovery(bad, source_receipt(), 0, 2, 100.)

    def test_bad_duration_nonfinite_quality_and_identity_claims_are_rejected(self):
        mutations = []
        for key, value in [('decoded_samples', 0), ('decoded_samples', True), ('sample_rate_hz', 8000), ('duration_s', 101.)]:
            bad = prediction([event('one')]); bad['source_metadata'][key] = value; mutations.append(bad)
        for key, value in [('status', 'no_events'), ('location_verified', True), ('ridge_count', 0)]:
            bad = prediction([event('one')]); bad[key] = value; mutations.append(bad)
        for key, value in [('aircraft_identity_confirmed', True), ('review_status', 'keep_for_matching'), ('ridge_count', 2)]:
            bad = prediction([event('one')]); bad['groups'][0][key] = value; mutations.append(bad)
        for key, value in [('median_contrast', float('nan')), ('frequency_sigma_hz', 0.)]:
            bad = prediction([event('one')]); bad['events'][0]['quality'][key] = value; mutations.append(bad)
        bad = prediction([event('one')]); bad['events'][0]['ridge'][0]['weight'] = 0.; mutations.append(bad)
        bad = prediction(); bad['unused_nested_diagnostic'] = {'values': [float('inf')]}; mutations.append(bad)
        for index, bad in enumerate(mutations):
            with self.subTest(index=index), self.assertRaises(AssertionError):
                audit.validate_discovery(bad, source_receipt(), 0, 2, 100.)
        with self.assertRaisesRegex(AssertionError, 'duration differs'):
            audit.validate_discovery(prediction(), source_receipt(), 0, 2, 101.)

    def test_receipts_bind_hash_size_and_path(self):
        expected = source_receipt()
        audit.check_receipt(deepcopy(expected), expected, 'input')
        for key, value in [('sha256', 'b' * 64), ('bytes', 101), ('path', str(ROOT / 'other.bin'))]:
            bad = dict(expected, **{key: value})
            with self.subTest(key=key), self.assertRaises(AssertionError):
                audit.check_receipt(bad, expected, 'input')

    @unittest.skipUnless(os.name == 'nt', 'Windows extended path identity')
    def test_windows_extended_paths_identify_the_same_registered_file(self):
        expected = source_receipt()
        extended = dict(expected, path='\\\\?\\' + expected['path'])
        audit.check_receipt(extended, expected, 'input')
        self.assertEqual(audit.path_identity(r'\\?\UNC\server\share\input.bin'),
                         audit.path_identity(r'\\server\share\input.bin'))

    def test_review_is_agent_diagnostic_inclusion_not_human_confirmation(self):
        rows = [event('b', group='one'), event('a', group='one')]
        review = audit.review_selection(prediction(rows), {'sha256': 'c' * 64}, {'one': rows})
        self.assertEqual(review['review_actor'], 'agent')
        self.assertIs(review['location_verified'], False)
        self.assertEqual(review['groups'], [{'harmonic_group_id': 'one', 'review': 'keep_for_matching', 'event_ids': ['a', 'b']}])
        self.assertIn('does not confirm an aircraft', review['scope'])


class NoEventRunControl(unittest.TestCase):
    def test_no_events_can_pass_technical_checks_without_claiming_accuracy(self):
        temporary_parent = ROOT / 'tmp'
        temporary_parent.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='geosonnar-private-control-', dir=temporary_parent) as temporary:
            root = Path(temporary).resolve()
            self.assertTrue(root.is_relative_to(temporary_parent.resolve()))
            case_dir = root / 'data/private-validation/cases'; case_dir.mkdir(parents=True)
            source = case_dir / 'fabricated-input.bin'; source.write_bytes(b'fabricated media placeholder')
            executable = root / 'fabricated-program.exe'; executable.write_bytes(b'fabricated program placeholder')
            case = case_dir / 'case.json'
            audit.write_json(case, {
                'schema_version': 1, 'visibility': 'private_local', 'case_id': 'fabricated',
                'source': dict(audit.receipt(source), path=source.name),
                'frozen_program': audit.receipt(executable), 'media': {'channels': 2},
            })
            output = root / 'output/private-validation/run'
            commands = []

            def simulate_native(arguments, prefix, timeout, allowed=(0,)):
                argv = list(map(str, arguments)); commands.append(argv)
                destination = Path(argv[argv.index('--out') + 1])
                if argv[1] == 'inspect':
                    audit.write_json(destination, {
                        'status': 'input_found', 'location_verified': False,
                        'media': {'bytes': source.stat().st_size, 'channels': 2, 'duration_s': 100.,
                                  'metadata': {'streams': [{'codec_type': 'audio', 'duration': '100.0'}]}},
                    })
                elif argv[1] == 'discover':
                    self.assertEqual(argv[2:], ['--input', str(source), '--out', str(destination), '--channel', argv[-1]])
                    destination.mkdir()
                    audit.write_json(destination / 'discovery.json', prediction(
                        source=audit.receipt(source), channel=int(argv[-1])))
                else:
                    self.fail(f'Empty discovery must not trigger acoustic fitting or other native work: {argv[1]}')
                return {'returncode': 0}

            with patch.object(audit, 'ROOT', root), patch.object(audit, 'native_snapshot', return_value={}), \
                 patch.object(audit, 'harness_snapshot', return_value={}), patch.object(audit, 'run_command', side_effect=simulate_native):
                self.assertEqual(audit.run(case, executable, output, 10), 0)
            report = audit.load_json(output / 'summary.json')
            self.assertEqual([cmd[1] for cmd in commands], ['inspect', 'discover', 'discover'])
            self.assertEqual([cmd[-1] for cmd in commands[1:]], ['0', '1'])
            self.assertTrue(report['technical_validation_passed'])
            self.assertEqual(report['status'], 'technical_checks_passed')
            self.assertFalse(report['location_verified'])
            self.assertFalse(report['aircraft_identity_confirmed'])
            self.assertFalse(report['truth_files_read'])
            self.assertFalse(report['network_access_used'])
            for metric in ('detection_precision', 'detection_recall', 'location_error_m'):
                self.assertIsNone(report[metric])
            self.assertEqual(report['geolocation']['status'], 'not_attempted')
            self.assertEqual(report['private_case_count'], 1)
            self.assertEqual(len(report['channels']), 2)
            for channel in report['channels']:
                self.assertEqual(channel['discovery_status'], 'no_events')
                self.assertEqual(channel['group_count'], 0)
                self.assertEqual(channel['fits'], [])
                self.assertTrue(channel['all_groups_accounted_for'])
            frozen = audit.load_json(output / 'frozen-plan.json')
            self.assertIsNone(frozen['expected_scientific_outcome'])
            self.assertIsNone(frozen['fit_pair_count_limit'])
            self.assertEqual(frozen['discovery_defaults'], DISCOVERY_DEFAULTS)


if __name__ == '__main__':
    unittest.main()
