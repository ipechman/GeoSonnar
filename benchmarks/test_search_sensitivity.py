"""Independent synthetic controls for bounded search-domain widening.

No source recording, historical winner, or evaluation location is loaded here.
The fixtures check preservation, coverage and conservative stability reporting.
"""
from copy import deepcopy
from pathlib import Path
import hashlib
import math
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'analysis'))
import benchmark_search_sensitivity as audit


POLICY = {'location_m': 50., 'frequency_hz': .5, 'clock_s': 1.,
          'shared_shift_s': .5, 'rmse_hz': .05, 'required_transitions': 2}


class Raster:
    def __init__(self, value):
        self.value = value
        self.calls = []

    def sample(self, latitude, longitude):
        self.calls.append((latitude, longitude))
        if isinstance(self.value, Exception):
            raise self.value
        return self.value


def observers():
    return [{'id': f'base-{north}-{east}', 'latitude_deg': 40. + north * .002,
             'longitude_deg': -73. + east * .002, 'ellipsoid_height_m': 17.}
            for north in range(2) for east in range(3)]


def search():
    return {'schema_version': 1, 'evidence_kind': 'synthetic',
            'provenance': ['Fabricated preservation control'],
            'observers': observers(),
            'tracks': [{'flight_id': name, 'points': [{'t_s': 1., 'xyz_m': [1., 2., 3.]}]}
                       for name in ('one', 'two', 'three')],
            'events': [{'points': [{'t_rec_s': 10., 'frequency_hz': 103.},
                                   {'t_rec_s': 22., 'frequency_hz': 99.}],
                        'f0_hz': [100.1, 100.5, 100.7, 101., 101.3]},
                       {'points': [{'t_rec_s': 100., 'frequency_hz': 155.},
                                   {'t_rec_s': 117., 'frequency_hz': 148.}],
                        'f0_hz': [150.1, 150.5, 150.8, 151., 151.4]}],
            'pair_config': {'shared_shifts_s': [-1., 0., 1.],
                            'event1_cpa_recording_s': 16.,
                            'clock_offset_bounds_s': [0., 86400.],
                            'model': {'sound_speed_mps': 343., 'max_gap_s': 30.}},
            'coarse_bounds': [{'speed_mps': [30., 200.], 'slant_distance_m': [100., 8000.]}] * 2,
            'measured_separation_s': 90., 'separation_tolerance_s': 6.}


def stage(latitude=40., longitude=-73., pair=('one', 'two'), frequency=(100., 150.),
          clock=500., shift=0., rmse=.5, boundary=False):
    row = {'observer': {'id': 'synthetic', 'latitude_deg': latitude,
                        'longitude_deg': longitude, 'ellipsoid_height_m': 17.},
           'score': {'flight_ids': list(pair), 'fitted_f0_hz': list(frequency),
                     'clock_offset_s': clock, 'shared_shift_s': shift,
                     'combined_rmse_hz': rmse, 'rmse_hz': [rmse, rmse]}}
    return {'best': row, 'best_diagnostics': {'coverage_incomplete': boundary},
            'any_retained_pair_coverage_incomplete': boundary}


class FrequencyExpansionControls(unittest.TestCase):
    def test_nested_frequency_domains_preserve_original_nonround_samples(self):
        original = search()
        base = original['events'][0]['f0_hz']
        untouched = list(base)
        previous = [list(event['f0_hz']) for event in original['events']]
        for width in (2., 4., 8.):
            expanded = audit.expanded_search(original, original['observers'], width, .5,
                                             previous_frequency_grids=previous)
            grid = expanded['events'][0]['f0_hz']
            with self.subTest(width=width):
                self.assertEqual(grid, sorted(set(grid)))
                self.assertEqual(grid[0], base[0] - width)
                self.assertEqual(grid[-1], base[-1] + width)
                for earlier, event in zip(previous, expanded['events'], strict=True):
                    self.assertTrue(set(earlier).issubset(event['f0_hz']))
                self.assertTrue(all(math.isfinite(f) and f > 0 for f in grid))
                expected_lattice = [n * .5 for n in range(math.ceil(grid[0] / .5), math.floor(grid[-1] / .5) + 1)]
                self.assertTrue(set(expected_lattice).issubset(grid))
                previous = [event['f0_hz'] for event in expanded['events']]
        self.assertEqual(base, untouched)

    def test_invalid_or_unbounded_frequency_requests_are_rejected(self):
        for grid, margin, step in [([], 2., .5), ([100., float('nan')], 2., .5),
                                   ([100., 101.], -1., .5), ([100., 101.], 2., 0.),
                                   ([1., 2.], 2., .5), ([100., 101.], float('inf'), .5),
                                   ([100., 101.], 1e7, .00001)]:
            with self.subTest(grid=grid, margin=margin, step=step), self.assertRaises((ValueError, AssertionError)):
                audit.widen_frequencies(grid, margin, step)

    def test_search_expansion_cannot_change_measurements_tracks_clock_or_filters(self):
        base = search(); before = deepcopy(base)
        wider_observers = deepcopy(base['observers'])
        wider_observers.append(dict(wider_observers[0], id='new', latitude_deg=39.998))
        changed = audit.expanded_search(base, wider_observers, 4., .5)
        self.assertEqual(base, before)
        self.assertEqual(changed['observers'], wider_observers)
        for key in ('schema_version', 'evidence_kind', 'tracks', 'pair_config', 'coarse_bounds',
                    'measured_separation_s', 'separation_tolerance_s'):
            self.assertEqual(changed[key], base[key], key)
        for current, original in zip(changed['events'], base['events'], strict=True):
            self.assertEqual(current['points'], original['points'])
            self.assertTrue(set(original['f0_hz']).issubset(current['f0_hz']))
        changed['events'][0]['points'][0]['frequency_hz'] = 0.
        changed['tracks'][0]['points'][0]['xyz_m'][0] = 99.
        self.assertEqual(base, before, 'Expanded searches must not alias original measured data')


class SpatialExpansionControls(unittest.TestCase):
    def test_regional_rings_preserve_lattice_and_sample_only_added_cells(self):
        base = observers(); before = deepcopy(base)
        dem, geoid = Raster(25.), Raster(-5.)
        expanded = audit.extend_observers(base, 1, dem, geoid, 3.)
        self.assertEqual(len(expanded), 20)
        for original in base:
            self.assertIn(original, expanded)
        self.assertEqual(len(dem.calls), 14)
        self.assertEqual(geoid.calls, dem.calls)
        added = [item for item in expanded if item not in base]
        self.assertTrue(all(item['ellipsoid_height_m'] == 23. for item in added))
        coordinates = {(o['latitude_deg'], o['longitude_deg']) for o in expanded}
        self.assertEqual(len(coordinates), 20)
        wider = audit.extend_observers(base, 2, Raster(25.), Raster(-5.), 3.)
        self.assertEqual(len(wider), 42)
        self.assertTrue(coordinates.issubset({(o['latitude_deg'], o['longitude_deg']) for o in wider}))
        self.assertEqual(base, before)

    def test_missing_cells_irregular_lattices_and_terrain_failure_are_not_hidden(self):
        incomplete = observers()[:-1]
        irregular = observers()
        irregular[0]['latitude_deg'] += .0001
        for base in (incomplete, irregular):
            with self.subTest(base=base), self.assertRaises((ValueError, AssertionError)):
                audit.extend_observers(base, 1, Raster(0.), Raster(0.), 3.)
        for terrain in (Raster(float('nan')), Raster(ValueError('No terrain coverage'))):
            with self.subTest(terrain=terrain), self.assertRaises((ValueError, AssertionError)):
                audit.extend_observers(observers(), 1, terrain, Raster(0.), 3.)

    def test_union_preserves_old_locations_heights_and_has_unique_stable_ids(self):
        first = observers()
        second = [dict(first[0], id='other-id'), dict(first[1], id='same-name', ellipsoid_height_m=29.),
                  dict(first[2], id='same-name', longitude_deg=-72.9)]
        before = deepcopy((first, second))
        merged = audit.merge_observers(first, second)
        expected = {(o['latitude_deg'], o['longitude_deg'], o['ellipsoid_height_m']) for o in first + second}
        actual = {(o['latitude_deg'], o['longitude_deg'], o['ellipsoid_height_m']) for o in merged}
        self.assertEqual(actual, expected)
        self.assertEqual(len(merged), len(expected))
        self.assertEqual(len({o['id'] for o in merged}), len(merged))
        repeated = audit.merge_observers(merged, first, second)
        self.assertEqual({(o['latitude_deg'], o['longitude_deg'], o['ellipsoid_height_m']): o['id'] for o in merged},
                         {(o['latitude_deg'], o['longitude_deg'], o['ellipsoid_height_m']): o['id'] for o in repeated})
        self.assertEqual((first, second), before)

    def test_spatial_interior_uses_covered_footprints_not_bounding_box_of_union(self):
        footprints = [{'south': 0., 'west': 0., 'north': 2., 'east': 2.},
                      {'south': 4., 'west': 4., 'north': 6., 'east': 6.}]
        def observer(lat, lon):
            return {'latitude_deg': lat, 'longitude_deg': lon, 'ellipsoid_height_m': 0.}
        self.assertTrue(audit.spatial_interior(observer(1., 1.), footprints))
        self.assertTrue(audit.spatial_interior(observer(5., 5.), footprints))
        self.assertFalse(audit.spatial_interior(observer(3., 3.), footprints))
        self.assertFalse(audit.spatial_interior(observer(2., 1.), footprints))
        self.assertFalse(audit.spatial_interior(observer(1., 1.), []))
        wider = footprints + [{'south': -1., 'west': -1., 'north': 3., 'east': 3.}]
        self.assertTrue(audit.spatial_interior(observer(2., 1.), wider),
                        'A former edge may become interior only when a new sampled footprint covers it')

    def test_coarse_coverage_cannot_mask_a_boundary_at_required_fine_resolution(self):
        winner = stage(latitude=1., longitude=1., frequency=(100.7, 150.8))['best']
        winner['score']['cpa'] = [{'speed_mps': 100., 'slant_distance_m': 1000.}] * 2
        coarse = {'south': 0., 'west': 0., 'north': 2., 'east': 2., 'spacing_m': 100.}
        fine = {'south': .5, 'west': .5, 'north': 1.5, 'east': 1., 'spacing_m': 25.}
        diagnosed = audit.union_diagnostics(winner, search(), [coarse, fine],
                                           required_spacing_m=25., newest_footprint=fine)
        self.assertFalse(diagnosed['at_sampled_support_boundary'])
        self.assertTrue(diagnosed['at_required_resolution_boundary'])
        self.assertTrue(diagnosed['at_observer_grid_boundary'])
        self.assertTrue(diagnosed['at_new_grid_boundary'])
        self.assertTrue(diagnosed['coverage_incomplete'])

        covering_fine = {'south': .75, 'west': .75, 'north': 1.25, 'east': 1.25,
                         'spacing_m': 25.}
        closed = audit.union_diagnostics(winner, search(), [coarse, fine, covering_fine],
                                        required_spacing_m=25., newest_footprint=covering_fine)
        self.assertFalse(closed['at_required_resolution_boundary'])
        self.assertFalse(closed['at_new_grid_boundary'])
        self.assertFalse(closed['coverage_incomplete'])

        # The identical covering area sampled only at 100 m must not close it.
        covering_coarse = dict(covering_fine, spacing_m=100.)
        still_open = audit.union_diagnostics(winner, search(), [coarse, fine, covering_coarse],
                                            required_spacing_m=25., newest_footprint=covering_coarse)
        self.assertTrue(still_open['at_required_resolution_boundary'])
        self.assertTrue(still_open['coverage_incomplete'])


class StabilityControls(unittest.TestCase):
    def assert_not_stable(self, before, after):
        try:
            report = audit.compare_stages(before, after, POLICY)
        except (ValueError, AssertionError):
            return
        self.assertIs(report['stable'], False)

    def test_small_interior_changes_are_conditional_stability_not_location_verification(self):
        one = stage()
        two = stage(latitude=40.0001, frequency=(100.25, 149.75), clock=500.5, shift=.25, rmse=.48)
        report = audit.compare_stages(one, two, POLICY)
        self.assertIs(report['stable'], True)
        self.assertFalse(report.get('location_verified', False))
        self.assertEqual(one, stage(), 'Evaluation must not modify previous results')

    def test_pair_location_tone_clock_shift_and_score_changes_break_stability(self):
        cases = [stage(pair=('two', 'one')), stage(latitude=40.001),
                 stage(frequency=(100.5001, 150.)), stage(frequency=(100., 149.4999)),
                 stage(clock=501.0001), stage(shift=.5001), stage(rmse=.5501)]
        for changed in cases:
            with self.subTest(changed=changed):
                self.assert_not_stable(stage(), changed)

    def test_equal_locations_at_any_winning_limit_are_not_stable(self):
        for before, after in [(stage(boundary=True), stage()),
                              (stage(), stage(boundary=True)),
                              (stage(boundary=True), stage(boundary=True))]:
            with self.subTest(before=before, after=after):
                self.assert_not_stable(before, after)

    def test_missing_candidate_missing_diagnostics_or_nonfinite_results_cannot_pass(self):
        missing = stage(); missing['best'] = None
        unchecked = stage(); unchecked['best_diagnostics'] = {}
        for changed in (missing, unchecked, stage(latitude=float('nan')),
                        stage(frequency=(float('nan'), 150.)), stage(clock=float('inf'))):
            with self.subTest(changed=changed):
                self.assert_not_stable(stage(), changed)

    def test_two_final_transitions_are_required_and_changed_winner_resets_history(self):
        self.assertFalse(audit.stability_summary([stage()], POLICY)['stable'])
        self.assertFalse(audit.stability_summary([stage(), stage()], POLICY)['stable'])
        self.assertTrue(audit.stability_summary([stage(), stage(), stage()], POLICY)['stable'])
        self.assertFalse(audit.stability_summary([stage(pair=('other', 'pair')), stage(), stage()], POLICY)['stable'])
        self.assertFalse(audit.stability_summary([stage(), stage(), stage(boundary=True)], POLICY)['stable'])
        self.assertTrue(audit.stability_summary([stage(pair=('old', 'pair')), stage(), stage(), stage()], POLICY)['stable'])

    def test_winning_stability_does_not_erase_an_unresolved_alternative(self):
        rows = [stage(), stage(), stage()]
        rows[-1]['any_retained_pair_coverage_incomplete'] = True
        summary = audit.stability_summary(rows, POLICY)
        self.assertTrue(summary['stable'])
        self.assertTrue(summary['any_retained_pair_coverage_incomplete'])
        self.assertFalse(summary.get('location_verified', False))


class FrozenInputControls(unittest.TestCase):
    def test_changed_or_missing_frozen_artifact_fails(self):
        directory = ROOT / 'tmp'; directory.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='sensitivity-control-', dir=directory) as temporary:
            path = Path(temporary) / 'fabricated-input.json'
            path.write_bytes(b'{"value":1}')
            frozen = {str(path): hashlib.sha256(path.read_bytes()).hexdigest()}
            audit.audit_frozen(frozen)
            path.write_bytes(b'{"value":2}')
            with self.assertRaises((ValueError, AssertionError)):
                audit.audit_frozen(frozen)
            path.unlink()
            with self.assertRaises((ValueError, AssertionError, FileNotFoundError)):
                audit.audit_frozen(frozen)


if __name__ == '__main__':
    unittest.main()
