"""Independent failure controls for audio-derived-prior and refinement audits."""
from copy import deepcopy
import math
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'analysis'))
import benchmark_audio_priors as audit


class AudioPriorsAuditControls(unittest.TestCase):
    def setUp(self):
        self.contract = audit.load_json(audit.CONTRACT)
        self.contract['review']['selected'] = [{'harmonic_group_id': 'a', 'event_id': 'one'}, {'harmonic_group_id': 'b', 'event_id': 'two'}]
        self.fits = []
        self.sources = []
        for name, group, tau, f0, speed, slant in [('one', 'a', 100., 100., 80., 1000.), ('two', 'b', 200., 120., 90., 2000.)]:
            parameters = {'emitted_frequency_hz': f0, 'speed_mps': speed, 'slant_distance_m': slant,
                          'heard_cpa_recording_s': tau, 'emission_cpa_recording_s': tau - slant / 343., 'rmse_hz': 0.}
            envelope = {'emitted_frequency_hz': [f0-1, f0+1], 'heard_cpa_recording_s': [tau-1, tau+1],
                        'speed_mps': [speed-2, speed+2], 'slant_distance_m': [slant-50, slant+50]}
            self.fits.append({'status': 'usable', 'usable_for_search': True, 'fit': parameters, 'envelope': envelope,
                              'diagnostics': {'sensitivity_is_confidence_interval':False, 'sample_count':4, 'reasons':[], 'leave_time_block_out_failures':0,
                                              'best_on_physical_boundary':False,'best_on_numerical_boundary':False,'profile_touches_physical_boundary':False,'profile_touches_numerical_boundary':False,
                                              'samples_before_heard_cpa':2,'samples_after_heard_cpa':2}, 'limitations': ['Engineering ranges only']})
            # Construct observations in emission time, independently of inverse fitter.
            ridge = []
            for emitted in (-30., -10., 10., 30.):
                radius = math.hypot(slant, speed * emitted)
                arrival = parameters['emission_cpa_recording_s'] + emitted + radius / 343.
                freq = f0 / (1. + speed * speed * emitted / (343. * radius))
                ridge.append({'time_s': arrival, 'frequency_hz': freq, 'weight': 2.})
            self.sources.append({'event_id': name, 'harmonic_group_id': group, 'ridge': ridge})
        self.discovery = {'events': self.sources}
        self.priors = {'status': 'audio_priors_ready', 'usable_for_search': True, 'location_verified': False, 'utc_clock_inferred': False,
                       'fits': self.fits, 'search_padding': {'frequency_margin_hz': 2., 'timing_margin_s': 2.5, 'speed_fraction': .2,
                            'slant_fraction': .35, 'slant_min_margin_m': 500., 'frequency_step_hz': .5, 'shift_step_s': .5},
                       'settings': {'sound_speed_mps': 343., 'speed_bounds_mps': [10.,280.], 'slant_bounds_m': [100.,30000.]}}
        ranges = []
        for f0, tau, speed, slant in [(100.,100.,[62.,98.],[450.,1550.]), (120.,200.,[70.,110.],[1250.,2750.])]:
            ranges.append({'emitted_frequency_hz': [f0-3, f0+3], 'f0_hz': [f0-3+i*.5 for i in range(13)],
                           'heard_cpa_recording_s': [tau-3.5,tau+3.5], 'speed_mps': speed, 'slant_distance_m': slant,
                           'physical_domain_clipping': {'speed': False, 'slant': False}})
        self.priors['derived'] = {'event1_cpa_recording_s':100., 'shared_shifts_s':[-3.5+i*.5 for i in range(15)],
            'coarse_bounds':[{'speed_mps':r['speed_mps'], 'slant_distance_m':r['slant_distance_m']} for r in ranges],
            'measured_separation_s':100., 'separation_tolerance_s':7., 'separation_bounds_s':[93.,107.], 'event_ranges':ranges}
        self.priors['events'] = [{'points':[{'t_rec_s':p['time_s'], 'frequency_hz':p['frequency_hz']} for p in source['ridge']],
                                 'f0_hz':r['f0_hz']} for source,r in zip(self.sources,ranges)]

    def test_inverse_oracle_matches_independent_emission_domain_samples(self):
        checked = audit.validate_fit_report(self.priors, self.discovery, self.contract, 0)
        self.assertTrue(checked['ready'])
        self.assertTrue(all(f['independent_rmse_hz'] < 1e-10 for f in checked['fits']))

    def test_shifted_large_timestamps_and_harmonic_scaling_preserve_oracle(self):
        fit = deepcopy(self.fits[0]['fit']); point = self.sources[0]['ridge'][0]
        fit['emission_cpa_recording_s'] += 1e8; fit['heard_cpa_recording_s'] += 1e8
        self.assertAlmostEqual(audit.emission_domain_frequency(point['time_s']+1e8,fit,343.),point['frequency_hz'],places=7)
        fit['emitted_frequency_hz'] *= 3
        self.assertAlmostEqual(audit.emission_domain_frequency(point['time_s']+1e8,fit,343.),point['frequency_hz']*3,places=7)

    def test_wrong_arrival_cpa_or_invented_good_rmse_fails(self):
        for field,value in [('heard_cpa_recording_s',99.),('rmse_hz',3.)]:
            bad=deepcopy(self.priors); bad['fits'][0]['fit'][field]=value
            with self.assertRaises(AssertionError): audit.validate_fit_report(bad,self.discovery,self.contract,0)

    def test_usability_cannot_override_inconclusive_fit_or_claim_utc(self):
        for field in ('location_verified','utc_clock_inferred'):
            bad=deepcopy(self.priors); bad[field]=True
            with self.assertRaises(AssertionError): audit.validate_fit_report(bad,self.discovery,self.contract,0)
        bad=deepcopy(self.priors); bad['fits'][0]['usable_for_search']=False
        with self.assertRaises(AssertionError): audit.validate_fit_report(bad,self.discovery,self.contract,0)
        for field in ('profile_touches_physical_boundary','best_on_numerical_boundary','sensitivity_is_confidence_interval'):
            bad=deepcopy(self.priors); bad['fits'][0]['diagnostics'][field]=True
            with self.assertRaises(AssertionError): audit.validate_fit_report(bad,self.discovery,self.contract,0)

    def test_inconclusive_preserves_diagnostics_but_no_frequency_fallback(self):
        bad=deepcopy(self.priors); bad.update(status='inconclusive_audio_priors',usable_for_search=False,derived=None)
        bad['fits'][0]['usable_for_search']=False
        for event in bad['events']: event['f0_hz']=[]
        self.assertFalse(audit.validate_fit_report(bad,self.discovery,self.contract,3)['ready'])
        bad['events'][0]['f0_hz']=[110.]
        with self.assertRaises(AssertionError): audit.validate_fit_report(bad,self.discovery,self.contract,3)

    def test_derived_interval_padding_and_two_event_separation_are_recomputed(self):
        self.assertTrue(audit.validate_derived_priors(self.priors,self.contract)['passed'])
        for field,value in [('event1_cpa_recording_s',102.),('measured_separation_s',99.),('separation_tolerance_s',3.)]:
            bad=deepcopy(self.priors); bad['derived'][field]=value
            with self.assertRaises(AssertionError): audit.validate_derived_priors(bad,self.contract)

    def test_manual_coarse_bounds_and_narrowed_frequency_grids_fail(self):
        for mutation in ('coarse','frequency','range','padding'):
            bad=deepcopy(self.priors)
            if mutation=='coarse': bad['derived']['coarse_bounds'][0]['speed_mps']=[70.,90.]
            elif mutation=='frequency': bad['events'][0]['f0_hz']=bad['events'][0]['f0_hz'][1:-1]
            elif mutation=='range': bad['derived']['event_ranges'][0]['heard_cpa_recording_s']=[99.,101.]
            else: bad['search_padding']['timing_margin_s']=1.
            with self.assertRaises(AssertionError): audit.validate_derived_priors(bad,self.contract)

    def test_grid_preserves_nonround_endpoints_and_center(self):
        self.assertEqual(audit.expected_grid([1.1,2.4],1.7,.5),[1.1,1.5,1.7,2.,2.4])
        self.assertEqual(audit.expected_grid([-.7,.8],0.,.5),[-.7,-.5,0.,.5,.8])
        with self.assertRaises(AssertionError): audit.expected_grid([0.,1e10],1.,.5)

    def test_source_binding_cannot_substitute_review_source_or_uncertainty(self):
        discovery=deepcopy(self.discovery); discovery['source_sha256']='a'*64
        for event in discovery['events']: event['quality']={'frequency_sigma_hz':.5}
        priors=deepcopy(self.priors); priors['selected_ridges']=self.contract['review']['selected']; priors['frequency_sigma_hz']=[.5,.5]
        evidence={'discovery':{'sha256':'b'*64},'review_selection':{'sha256':'c'*64},'source':{'sha256':'a'*64},'review_actor':'agent',
                  'selected_ridges':[{'quality':e['quality'],'contrast_weights':[2.]*4,'other_group_ridges_not_scored':[]} for e in discovery['events']]}
        priors['provenance']={'reviewed_audio':evidence}
        self.assertTrue(audit.validate_source_binding(priors,discovery,self.contract,'b'*64,'c'*64)['passed'])
        for area in ('source','review_selection','discovery'):
            bad=deepcopy(priors); bad['provenance']['reviewed_audio'][area]['sha256']='d'*64
            with self.assertRaises(AssertionError): audit.validate_source_binding(bad,discovery,self.contract,'b'*64,'c'*64)
        bad=deepcopy(priors); bad['frequency_sigma_hz']=[5.,5.]
        with self.assertRaises(AssertionError): audit.validate_source_binding(bad,discovery,self.contract,'b'*64,'c'*64)

    def test_refinement_cannot_substitute_old_pairs_or_skip_an_alternative(self):
        rows=[{'score':{'flight_ids':[a,b]}} for a,b in [('new-first','new-second'),('new-third','new-fourth')]]
        regional={'best_per_flight_pair':rows}
        refined={'execution_passed':True,'location_verified':False,'conditional_on_retained_pairs':True,'status':'completed',
                 'pairs':[{'flight_ids':r['score']['flight_ids'],'local':{'best':r},'fine':{'best':r}} for r in rows]}
        self.assertEqual(audit.validate_refinement(refined,regional)['retained_pairs'],2)
        bad=deepcopy(refined); bad['pairs'][0]['flight_ids']=['old-first','old-second']
        with self.assertRaises(AssertionError): audit.validate_refinement(bad,regional)
        bad=deepcopy(refined); bad['pairs'][1].pop('fine')
        with self.assertRaises(AssertionError): audit.validate_refinement(bad,regional)

    def test_refinement_boundary_is_preserved_as_incomplete_not_failure_or_accuracy(self):
        report={'execution_passed':True,'location_verified':False,'conditional_on_retained_pairs':True,'status':'coverage_incomplete','pairs':[]}
        self.assertEqual(audit.validate_refinement(report,{'best_per_flight_pair':[]})['scientific_status'],'coverage_incomplete')
        report['location_verified']=True
        with self.assertRaises(AssertionError): audit.validate_refinement(report,{'best_per_flight_pair':[]})


if __name__=='__main__':
    unittest.main()
