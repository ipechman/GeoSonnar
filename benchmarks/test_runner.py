"""Failure controls for the validation harness, without running scientific jobs."""
from copy import deepcopy
from pathlib import Path
from types import SimpleNamespace
import hashlib
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

import run_validation as runner

_package_spec=importlib.util.spec_from_file_location('package_windows',runner.ROOT/'analysis/package_windows.py')
packager=importlib.util.module_from_spec(_package_spec)
_package_spec.loader.exec_module(packager)


class HarnessControls(unittest.TestCase):
    def setUp(self):
        parent=runner.ROOT/'output'/'benchmarks'/'harness-controls'
        parent.mkdir(parents=True,exist_ok=True)
        self.temporary=tempfile.TemporaryDirectory(prefix='control-',dir=parent)
        self.root=Path(self.temporary.name).resolve()
        self.assertTrue(self.root.is_relative_to(parent.resolve()))
        self.addCleanup(self.temporary.cleanup)

    def validation(self,name='run'):
        return runner.Validation(SimpleNamespace(output=self.root/name,exe=Path(sys.executable),timeout=10,source=None,skip_tests=False,skip_build=True))

    def test_existing_output_is_refused_and_preserved(self):
        out=self.root/'existing'; out.mkdir()
        marker=out/'do-not-overwrite.txt'; marker.write_text('existing evidence')
        with self.assertRaises(FileExistsError): self.validation('existing')
        self.assertEqual(marker.read_text(),'existing evidence')

    def test_unexpected_nonzero_exit_fails_stage_and_preserves_receipts(self):
        validation=self.validation()
        passed=validation.stage('failure-control',lambda:validation.command('failure',[sys.executable,'-c','import sys; print("diagnostic",file=sys.stderr); sys.exit(7)']))
        self.assertFalse(passed)
        summary=runner.load_json(validation.output/'summary.json')
        self.assertEqual(summary['stages'][0]['status'],'failed')
        receipt=runner.load_json(validation.output/'logs/failure.command.json')
        self.assertEqual(receipt['returncode'],7)
        self.assertEqual(receipt['expected_returncode'],0)
        self.assertIn('diagnostic',(validation.output/'logs/failure.stderr').read_text())

    def test_timeout_kills_owned_descendant_but_preserves_an_unrelated_process(self):
        validation=self.validation()
        started=self.root/'owned-started.txt'; late=self.root/'owned-survived.txt'; decoy_done=self.root/'decoy-finished.txt'
        child_code=f'import time; from pathlib import Path; Path({str(started)!r}).write_text("started"); time.sleep(3); Path({str(late)!r}).write_text("must not happen")'
        parent_code=f'import subprocess,sys,time; subprocess.Popen([sys.executable,"-c",{child_code!r}]); time.sleep(20)'
        decoy_code=f'import time; from pathlib import Path; time.sleep(2); Path({str(decoy_done)!r}).write_text("survived")'
        options={'creationflags':subprocess.CREATE_NO_WINDOW} if os.name=='nt' else {'start_new_session':True}
        decoy=subprocess.Popen([sys.executable,'-c',decoy_code],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,**options)
        try:
            with self.assertRaises(subprocess.TimeoutExpired):
                validation.command('timeout',[sys.executable,'-c',parent_code],timeout=1.5)
            self.assertTrue(started.is_file(),'Owned descendant must actually start before the timeout')
            self.assertEqual(decoy.wait(timeout=10),0)
            time.sleep(1.5)
            self.assertTrue(decoy_done.is_file(),'Unrelated process must survive')
            self.assertFalse(late.exists(),'Owned descendant survived the command timeout')
            receipt=runner.load_json(validation.output/'logs/timeout.command.json')
            self.assertIn('TimeoutExpired',receipt['execution_error'])
        finally:
            if decoy.poll() is None:
                decoy.kill(); decoy.wait(timeout=10)

    def test_replays_cannot_pass_with_empty_short_duplicate_or_renamed_cases(self):
        manifest=runner.load_json(runner.ROOT/'benchmarks/replay_baselines.json')
        runner.validate_replay_manifest(manifest)
        for cases in [[],manifest['cases'][:1],[manifest['cases'][0]]*2]:
            bad=deepcopy(manifest); bad['cases']=cases
            with self.assertRaises(AssertionError): runner.validate_replay_manifest(bad)
        bad=deepcopy(manifest); bad['cases'][0]['id']='optional-replacement'
        with self.assertRaises(AssertionError): runner.validate_replay_manifest(bad)
        for value in [-1,float('nan'),True,'2']:
            bad=deepcopy(manifest); bad['tolerances']['winner_coordinate_drift_m']=value
            with self.assertRaises(AssertionError): runner.validate_replay_manifest(bad)

    @staticmethod
    def candidate():
        return {'observer':{'latitude_deg':40.,'longitude_deg':-73.,'ellipsoid_height_m':-30.},'score':{
            'flight_ids':['a','b'],'rmse_hz':[1.,2.],'fitted_f0_hz':[110.,120.],
            'cpa':[{},{}],'combined_rmse_hz':1.5,'clock_offset_s':123.,'shared_shift_s':0.}}

    def test_pair_vector_truncation_cannot_exploit_empty_zip(self):
        runner.validate_pair_candidate(self.candidate())
        for key in ['rmse_hz','flight_ids','fitted_f0_hz','cpa']:
            for count in [0,1,3]:
                bad=self.candidate(); bad['score'][key]=(bad['score'][key]*2)[:count]
                with self.assertRaises(AssertionError): runner.validate_pair_candidate(bad)
        for value in [None,True,float('nan'),float('inf'),-1.]:
            bad=self.candidate(); bad['score']['rmse_hz'][0]=value
            with self.assertRaises(AssertionError): runner.validate_pair_candidate(bad)

    def test_full_archive_negative_allows_hypotheses_but_checks_every_reference_pair(self):
        manifest=runner.load_json(runner.ROOT/'benchmarks/replay_baselines.json')
        case=manifest['negative_simplified']
        candidate=self.candidate(); candidate['score']['flight_ids']=['a79771:source#part1','a3915a:source#part4']
        result={'status':'hypotheses_ranked','location_verified':False,'ranked_count':1,'candidates':[candidate]}
        self.assertTrue(all(runner.simplified_checks(result,case).values()))
        hidden=self.candidate(); hidden['score']['flight_ids']=['a1dfeb:source#part9','ace391:source#part5']
        result['candidates'].append(hidden); result['ranked_count']=2
        self.assertFalse(all(runner.simplified_checks(result,case).values()),'An incomplete reference pair cannot hide below the winning row')
        hidden['score']['flight_ids'].reverse()
        self.assertFalse(all(runner.simplified_checks(result,case).values()))
        result['candidates'].pop(); result['ranked_count']=1; result['location_verified']=True
        self.assertFalse(all(runner.simplified_checks(result,case).values()))

    def test_partial_pair_negative_requires_empty_result_and_unchanged_source_parts(self):
        manifest=runner.validate_replay_manifest(runner.load_json(runner.ROOT/'benchmarks/replay_baselines.json'))
        case=manifest['negative_partial_pair']
        result={'status':'no_candidate','location_verified':False,'ranked_count':0,'candidates':[],'best_per_flight_pair':[]}
        self.assertTrue(all(runner.simplified_checks(result,case).values()))
        for key,value in [('status','hypotheses_ranked'),('location_verified',True),('ranked_count',1),('candidates',[self.candidate()]),('best_per_flight_pair',[self.candidate()])]:
            bad=deepcopy(result); bad[key]=value
            self.assertFalse(all(runner.simplified_checks(bad,case).values()))
        # The harness control must work without the separately acquired replay.
        # Use multiple parts per aircraft, an excluded competitor, measured event
        # points, and source provenance so unchanged-subset rules are still tested.
        fixture_root=self.root/'partial-fixture'; fixture_root.mkdir()
        original={
            'schema_version':1,
            'evidence_kind':'fabricated harness control',
            'provenance':['Fabricated parent source'],
            'tracks':[
                {'flight_id':identifier,'points':[{'t_s':float(index),'xyz_m':[float(index),2.,3.]},
                                                  {'t_s':float(index+1),'xyz_m':[float(index+1),3.,4.]}]}
                for index,identifier in enumerate(['b00002:fixture#part1','a00001:fixture#part2',
                                                    'c00003:fixture#part1','a00001:fixture#part1'])],
            'observers':[{'id':f'fabricated-{index}','latitude_deg':0.,'longitude_deg':float(index),
                          'ellipsoid_height_m':0.} for index in range(2)],
            'events':[{'points':[{'t_rec_s':float(index),'frequency_hz':100.},
                                 {'t_rec_s':float(index+1),'frequency_hz':99.}],
                       'f0_hz':100.} for index in range(2)],
            'measured_separation_s':1.,
        }
        fixture=deepcopy(original)
        fixture['tracks']=sorted([track for track in fixture['tracks']
                                  if runner.source_aircraft_id(track['flight_id']) in ['a00001','b00002']],
                                 key=lambda track:track['flight_id'])
        fixture['provenance'].append('Fabricated declared aircraft subset')
        source_path=fixture_root/'source.json'; partial_path=fixture_root/'partial.json'
        source_path.write_text(json.dumps(original),encoding='utf-8')
        partial_path.write_text(json.dumps(fixture),encoding='utf-8')
        fixture_case={**case,'source_input':'source.json','input':'partial.json',
                      'source_input_sha256':runner.digest(source_path),'input_sha256':runner.digest(partial_path),
                      'selected_icao24':['a00001','b00002'],'track_count':3}
        with patch.object(runner,'ROOT',fixture_root):
            derivation=runner.validate_partial_fixture(fixture_case)
            self.assertEqual(derivation['track_count'],3)
            self.assertEqual(derivation['trajectory_vertices'],6)
            self.assertEqual(derivation['observer_count'],2)
            self.assertEqual(derivation['ridge_sample_counts'],[2,2])
            self.assertEqual(derivation['source']['sha256'],fixture_case['source_input_sha256'])
            mutations=[]
            changed=deepcopy(fixture); changed['tracks'].pop(); mutations.append(('missing selected part',changed))
            changed=deepcopy(fixture); changed['tracks'].reverse(); mutations.append(('unsorted source parts',changed))
            changed=deepcopy(fixture); changed['tracks'][0]['points'][0]['xyz_m'][0]+=1.; mutations.append(('changed trajectory',changed))
            changed=deepcopy(fixture); changed['tracks'].append(original['tracks'][2]); mutations.append(('extra competitor',changed))
            changed=deepcopy(fixture); changed['events'][0]['points'][0]['frequency_hz']+=1.; mutations.append(('changed event',changed))
            changed=deepcopy(fixture); changed['observers'][0]['ellipsoid_height_m']+=1.; mutations.append(('changed observer',changed))
            changed=deepcopy(fixture); changed['measured_separation_s']+=1.; mutations.append(('changed setting',changed))
            changed=deepcopy(fixture); changed['provenance'][0]='Replaced source provenance'; mutations.append(('changed provenance',changed))
            changed=deepcopy(fixture); changed['provenance'].pop(); mutations.append(('missing annotation',changed))
            for label,changed in mutations:
                with self.subTest(mutation=label):
                    partial_path.write_text(json.dumps(changed),encoding='utf-8')
                    with self.assertRaises(AssertionError):runner.validate_partial_fixture(fixture_case)
            partial_path.write_text(json.dumps(fixture),encoding='utf-8')
            with self.assertRaises(AssertionError):runner.validate_partial_fixture({**fixture_case,'track_count':4})
            source_path.write_bytes(source_path.read_bytes()+b' ')
            with self.assertRaisesRegex(AssertionError,'parent source changed'):
                runner.validate_partial_fixture(fixture_case)
        bad=deepcopy(manifest); del bad['negative_partial_pair']
        with self.assertRaises(AssertionError): runner.validate_replay_manifest(bad)

    def test_negative_decision_requires_false_acceptance_and_explicit_no_probability(self):
        observed={'status':'inconclusive','reason_code':'ambiguous_locations','accepted_location':False,
                  'diagnostics':{'probability':None,'calibration':'uncalibrated'}}
        self.assertTrue(all(runner.decision_checks(observed,'inconclusive','ambiguous_locations').values()))
        for acceptance in [True,None,0,'false']:
            bad=deepcopy(observed); bad['accepted_location']=acceptance
            self.assertFalse(all(runner.decision_checks(bad,'inconclusive','ambiguous_locations').values()))
        del observed['diagnostics']['probability']
        self.assertFalse(all(runner.decision_checks(observed,'inconclusive','ambiguous_locations').values()))

    def test_json_contract_mutation_fails_final_immutability_check(self):
        fake=self.root/'fake-project'; (fake/'benchmarks/truth').mkdir(parents=True); (fake/'cases').mkdir()
        labels=fake/'benchmarks/truth/labels.json'; labels.write_text('{"answer":1}')
        contract=fake/'cases/case.json'; contract.write_text('{}')
        validation=self.validation()
        with patch.object(runner,'ROOT',fake),patch.object(runner,'code_snapshot',return_value={'source.rs':'same'}):
            before=runner.contract_snapshot()
            validation.report['provenance']['source_sha256']={'source.rs':'same'}
            validation.report['provenance']['contract_sha256']=before
            validation.report['provenance']['executable']=runner.receipt(sys.executable)
            self.assertTrue(validation.unchanged()['benchmark_contracts_and_evaluation_files_unchanged'])
            for file in [labels,contract]:
                original=file.read_text(); file.write_text(original+' ')
                with self.assertRaisesRegex(AssertionError,'contract/evaluation'):
                    validation.unchanged()
                file.write_text(original)

    def test_pinned_fixture_is_verified_against_hash_and_git_blob_receipt(self):
        upstream=self.root/'upstream'; upstream.mkdir()
        fixture=upstream/'fixture.json'; fixture.write_bytes(b'{"fixed":true}\n')
        data=fixture.read_bytes()
        row={'local_path':'fixture.json','bytes':len(data),'sha256':hashlib.sha256(data).hexdigest(),
             'git_blob_sha1':hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()}
        (upstream/'source-receipt.json').write_text(json.dumps({'upstream_commit':runner.PIN,'files':[row]}))
        with patch.object(runner,'UPSTREAM',upstream):
            runner.verified_upstream_receipt(fixture)
            fixture.write_bytes(b'{"fixed":false}\n')
            with self.assertRaisesRegex(AssertionError,'fixture changed'): runner.verified_upstream_receipt(fixture)

    def packaging_fixture(self):
        root=self.root/'packaging-project'
        for directory in ['src/gui','tests','analysis/licenses','benchmarks','docs','bin','crate']:
            (root/directory).mkdir(parents=True,exist_ok=True)
        for name,content in [('Cargo.toml','[package]\nname="fixture"\n'),('Cargo.lock','# fixture'),
                             ('analysis/package_windows.py','# fixture'),('benchmarks/run_validation.py','# fixture'),
                             ('src/gui/world-map.json','{"fabricated_map":true}'),('LICENSE','MIT fixture'),
                             ('THIRD_PARTY_NOTICES.md','fixture notices'),('analysis/licenses/DOPPLERGEIST-MIT.txt','fixture MIT'),
                             ('docs/NATURAL_EARTH.txt','Fabricated public-domain map notice control'),('crate/LICENSE','MIT fixture dependency'),('crate/Cargo.toml','[package]\nname="dependency"\n')]:
            (root/name).write_text(content,encoding='utf-8')
        pe=bytearray(70);pe[:2]=b'MZ';pe[60:64]=(64).to_bytes(4,'little');pe[64:]=b'PE\0\0\x64\x86'
        for name in ['geosonnar.exe','geosonnar-gui.exe']:(root/'bin'/name).write_bytes(pe)
        metadata={'workspace_root':str(root),'packages':[{'id':'fixture','name':'fixture','version':'0.0.0','manifest_path':str(root/'Cargo.toml')},
            {'id':'dependency','name':'dependency','version':'1.0.0','manifest_path':str(root/'crate/Cargo.toml'),'license':'MIT'}],
            'resolve':{'root':'fixture','nodes':[{'id':'fixture','dependencies':['dependency'],'features':[]},{'id':'dependency','dependencies':[],'features':[]}]}}
        return root,metadata

    def test_embedded_map_bytes_bind_both_snapshots_and_missing_asset_fails(self):
        root,_=self.packaging_fixture()
        with patch.object(runner,'ROOT',root),patch.object(runner,'__file__',str(root/'benchmarks/run_validation.py')),patch.object(packager,'ROOT',root),patch.object(packager,'__file__',str(root/'analysis/package_windows.py')):
            before=runner.code_snapshot(); package_before=packager.source_inventory()
            key=str(Path('src/gui/world-map.json'))
            self.assertIn(key,before)
            self.assertIn('src/gui/world-map.json',{p['path'] for p in package_before})
            asset=root/'src/gui/world-map.json';asset.write_text('{"fabricated_map":false}')
            self.assertNotEqual(before[key],runner.code_snapshot()[key])
            self.assertNotEqual(package_before,packager.source_inventory())
            asset.unlink()
            with self.assertRaises(FileNotFoundError):runner.code_snapshot()
            with self.assertRaises(ValueError):packager.source_inventory()

    def test_native_experiment_adapter_changes_invalidate_validation_and_package_snapshots(self):
        root,_=self.packaging_fixture()
        (root/'examples').mkdir()
        adapter=root/'examples/regional_search.rs'
        adapter.write_text('// fabricated experiment adapter, first revision\n')
        with patch.object(runner,'ROOT',root),patch.object(runner,'__file__',str(root/'benchmarks/run_validation.py')),patch.object(packager,'ROOT',root),patch.object(packager,'__file__',str(root/'analysis/package_windows.py')):
            before=runner.code_snapshot()
            package_before=packager.source_inventory()
            key=str(Path('examples/regional_search.rs'))
            self.assertIn(key,before)
            self.assertIn('examples/regional_search.rs',{p['path'] for p in package_before})
            adapter.write_text('// fabricated experiment adapter, changed prediction policy\n')
            self.assertNotEqual(before[key],runner.code_snapshot()[key])
            self.assertNotEqual(package_before,packager.source_inventory())

    def test_package_copies_exact_map_notice_and_requires_it_before_output(self):
        root,metadata=self.packaging_fixture()
        args=SimpleNamespace(output=root/'portable',binary_dir=root/'bin',readme=None,validation=None)
        with patch.object(packager,'ROOT',root),patch.object(packager,'__file__',str(root/'analysis/package_windows.py')),patch.object(packager,'SUPPLEMENTS',root/'missing-supplements.json'),patch.object(packager.subprocess,'run',return_value=SimpleNamespace(returncode=0,stdout=json.dumps(metadata))):
            packager.package(args)
            expected=(root/'docs/NATURAL_EARTH.txt').read_bytes()
            self.assertEqual((args.output/'NATURAL_EARTH.txt').read_bytes(),expected)
            report=runner.load_json(args.output/'package-receipt.json')
            artifact=next(a for a in report['artifacts'] if a['path']=='NATURAL_EARTH.txt')
            self.assertEqual(artifact['sha256'],hashlib.sha256(expected).hexdigest())
            self.assertFalse(report['validation_run_by_packager'])
            args.output=root/'must-not-exist';(root/'docs/NATURAL_EARTH.txt').unlink()
            with self.assertRaisesRegex(ValueError,'Required file'):packager.package(args)
            self.assertFalse(args.output.exists())

    def test_package_uses_canonical_notices_without_a_previous_distribution(self):
        root,metadata=self.packaging_fixture()
        self.assertFalse((root/'dist').exists(), 'The fixture must model a clean clone')
        args=SimpleNamespace(output=root/'portable-clean',binary_dir=root/'bin',readme=None,validation=None)
        canonical={'THIRD_PARTY_NOTICES.md':root/'THIRD_PARTY_NOTICES.md',
                   'DOPPLERGEIST-MIT.txt':root/'analysis/licenses/DOPPLERGEIST-MIT.txt'}
        with patch.object(packager,'ROOT',root),patch.object(packager,'__file__',str(root/'analysis/package_windows.py')),patch.object(packager,'SUPPLEMENTS',root/'missing-supplements.json'),patch.object(packager.subprocess,'run',return_value=SimpleNamespace(returncode=0,stdout=json.dumps(metadata))):
            for output in [root/'portable-clean',root/'portable-after-old-release']:
                args.output=output
                packager.package(args)
                report=runner.load_json(output/'package-receipt.json')
                for target,source in canonical.items():
                    expected=source.read_bytes()
                    self.assertEqual((output/target).read_bytes(),expected)
                    artifact=next(a for a in report['artifacts'] if a['path']==target)
                    self.assertEqual(artifact['sha256'],hashlib.sha256(expected).hexdigest())
                # A stale ignored release must not override current source notices.
                (root/'dist').mkdir(exist_ok=True)
                for name in canonical:(root/'dist'/name).write_text('stale release notice',encoding='utf-8')

    def test_media_integration_rejects_empty_missing_duplicate_and_ignored_execution(self):
        expected=['first','second']
        good='test first ... ok\ntest second ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n'
        self.assertEqual(runner.require_executed_tests(good,expected),expected)
        for bad in ['',good.replace('test second ... ok\n',''),good.replace('second','first'),good.replace('2 passed','0 passed'),good.replace('0 ignored','1 ignored')]:
            with self.assertRaises(AssertionError):runner.require_executed_tests(bad,expected)

    def test_media_helper_selection_and_execution_bind_actual_paths_and_content(self):
        ffmpeg=self.root/'actual-ffmpeg.exe';ffmpeg.write_bytes(b'fabricated helper')
        probe=self.root/'actual-ffprobe.exe';probe.write_bytes(b'fabricated probe')
        args=SimpleNamespace(ffmpeg=ffmpeg,ffprobe=None)
        with patch.dict(os.environ,{'GEOSONNAR_TEST_FFMPEG':str(self.root/'wrong'),'GEOSONNAR_TEST_FFPROBE':str(probe)}):
            self.assertEqual(runner.media_helper_paths(args),{'ffmpeg':ffmpeg,'ffprobe':probe})
            args.ffmpeg=self.root/'missing'
            with self.assertRaises(FileNotFoundError):runner.media_helper_paths(args)
        validation=self.validation()
        validation.report['provenance']['gui_media_helpers']={name:runner.receipt(path) for name,path in [('ffmpeg',ffmpeg),('ffprobe',probe)]}
        def command(name,arguments,environment=None):
            self.assertEqual(arguments[-4:],['--test','review_media_validation','--','--ignored'])
            self.assertEqual(environment,{'GEOSONNAR_TEST_FFMPEG':str(ffmpeg),'GEOSONNAR_TEST_FFPROBE':str(probe)})
            (validation.output/'logs'/f'{name}.stdout').write_text('test control ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;\n')
            return {'returncode':0}
        with patch.object(validation,'command',side_effect=command):
            self.assertEqual(validation.media_integration('media','review_media_validation',['control'])['executed_tests'],['control'])
            ffmpeg.write_bytes(b'changed')
            with self.assertRaisesRegex(AssertionError,'changed before'):validation.media_integration('media','review_media_validation',['control'])


if __name__=='__main__':
    unittest.main()
