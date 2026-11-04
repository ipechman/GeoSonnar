"""Run GeoSonnar's implemented validation profile in a fresh output directory.

This validates native code and executable behavior. It does not execute the full
upstream Python application, invent unavailable features, or turn source-informed
reference replays into independently surveyed accuracy claims. No large source
inputs are copied into the run: their paths and cryptographic hashes are saved.

Default: formatting, locked offline tests, Clippy, static-CRT release build,
16 frozen decision fixtures, real codec/audio audit, both native regional replays,
simplified-archive nonacceptance, incomplete-pair abstention, and the known-clock
faint-event development lane.
"""
from __future__ import annotations
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
PIN='70626ad44fb9c7f4e2f5b225956c1f9281f91c13'
UPSTREAM=ROOT/'benchmarks'/'upstream'/PIN
REASONS={
 'supported':'supported_by_evidence_checks',
 'noise':'no_doppler_events',
 'missing':'missing_flight_data',
 'ambiguous':'ambiguous_locations',
 'harmonics':'insufficient_independent_events',
 'residual':'inconsistent_event_match',
 'altitude_only':'uncertainty_not_established',
 'missing_diagnostics':'matching_diagnostics_missing',
}
GUI_BRIDGE_MEDIA_TESTS=['actual_companion_creates_case_and_discovery_job_then_verifies_review_artifact']
REVIEW_MEDIA_TESTS=[
    'actual_stereo_chirp_windows_keep_channel_time_frequency_and_rms',
    'actual_video_frame_reports_decoded_timestamp_aspect_and_source_stamp',
    'shifted_first_audio_stream_stays_aligned_with_frame_time_and_spectrum',
]


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()


def load_json(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def write_json(path,value):
    Path(path).write_text(json.dumps(value,indent=2,allow_nan=False)+'\n',encoding='utf-8')


def receipt(path):
    path=Path(path).resolve()
    if not path.is_file():
        raise FileNotFoundError(f'Required input is missing: {path}')
    return {'path':str(path),'bytes':path.stat().st_size,'sha256':digest(path)}


def code_snapshot():
    # Source identity only; no evaluation labels enter native process inputs.
    files=set().union(*((ROOT/folder).rglob('*.rs') for folder in ('src','tests','examples')))
    files.update((ROOT/'src').rglob('*.html'))
    # Native include_str!/include_bytes! fixtures are compiled inputs too.
    files.update(path for path in (ROOT/'tests'/'fixtures').rglob('*') if path.is_file())
    files.update((ROOT/'analysis').glob('*.py'))
    files.update((ROOT/'benchmarks').glob('*.py'))
    files.update([Path(__file__),ROOT/'Cargo.toml',ROOT/'Cargo.lock',ROOT/'src/gui/world-map.json'])
    return {str(p.relative_to(ROOT)):digest(p) for p in sorted(files)}


def media_helper_paths(args):
    result={}
    for name in ['ffmpeg','ffprobe']:
        selected=getattr(args,name,None) or os.environ.get('GEOSONNAR_TEST_'+name.upper())
        if selected is None:
            actual=Path('C:/ProgramData/chocolatey/lib/ffmpeg-full/tools/ffmpeg/bin')/(name+'.exe')
            selected=actual if os.name=='nt' and actual.is_file() else shutil.which(name)
        if not selected:
            raise FileNotFoundError(f'GUI media integration requires an actual {name} executable; pass --{name}')
        path=Path(selected).resolve(strict=True)
        if not path.is_file():
            raise ValueError(f'Media helper must be a file: {path}')
        result[name]=path
    return result


def require_executed_tests(stdout,expected):
    # An empty/renamed Cargo filter exits successfully. It is not execution evidence.
    passed=re.findall(r'^test ([A-Za-z0-9_:]+) \.\.\. ok\s*$',stdout,re.MULTILINE)
    summaries=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',stdout)
    if sorted(passed)!=sorted(expected) or summaries!=[(str(len(expected)),'0','0')]:
        raise AssertionError('Installed-media integration did not execute every declared test exactly once')
    return passed


def contract_snapshot():
    # Opaque byte hashes establish immutability. Evaluation JSON is not parsed
    # until the corresponding predictions exist, and is never sent to Rust.
    files=set((ROOT/'benchmarks').rglob('*.json')) | set((ROOT/'cases').glob('*.json'))
    return {str(p.relative_to(ROOT)):digest(p) for p in sorted(files)}


def validate_replay_manifest(manifest):
    cases=manifest.get('cases')
    if manifest.get('schema_version')!=2 or not isinstance(cases,list) or len(cases)!=2:
        raise AssertionError('Replay manifest requires exactly two declared regional cases')
    if {case.get('id') for case in cases}!={'regional-detailed','regional-fine'}:
        raise AssertionError('Replay manifest must contain both unique required regional case IDs')
    for case in cases:
        for key in ['input','baseline']:
            if not isinstance(case.get(key),str) or not case[key].strip():
                raise AssertionError(f'Replay case needs {key}')
            expected=case.get(key+'_sha256')
            if not isinstance(expected,str) or len(expected)!=64 or any(c not in '0123456789abcdef' for c in expected):
                raise AssertionError(f'Replay case needs a frozen {key} hash')
    for key in ['winner_coordinate_drift_m','rmse_absolute_difference_hz','clock_absolute_difference_s']:
        value=manifest.get('tolerances',{}).get(key)
        if isinstance(value,bool) or not isinstance(value,(int,float)) or not math.isfinite(value) or value<0:
            raise AssertionError(f'Invalid replay tolerance: {key}')
    for key,identifier,status,code in [
        ('negative_simplified','simplified-archive-remains-unverified','hypotheses_ranked',0),
        ('negative_partial_pair','simplified-reference-pair-incomplete','no_candidate',3),
    ]:
        negative=manifest.get(key,{})
        if negative.get('id')!=identifier or not negative.get('input') or not negative.get('input_sha256') or negative.get('expected_status')!=status or negative.get('expected_exit_code')!=code:
            raise AssertionError(f'Required negative control is missing or changed: {key}')
    if manifest['negative_simplified'].get('unavailable_reference_pair_icao24')!=['a1dfeb','ace391']:
        raise AssertionError('Full simplified archive must account for the declared unavailable reference pair')
    if manifest['negative_partial_pair'].get('selected_icao24')!=['a1dfeb','ace391']:
        raise AssertionError('Incomplete-pair fixture must preserve both declared source aircraft')
    return manifest


def validate_pair_candidate(candidate):
    def finite(value,nonnegative=False):
        if isinstance(value,bool) or not isinstance(value,(int,float)) or not math.isfinite(value) or (nonnegative and value<0):
            raise AssertionError('Replay candidate contains an invalid numeric field')
    score=candidate['score']
    for key in ['flight_ids','rmse_hz','fitted_f0_hz','cpa']:
        if not isinstance(score.get(key),list) or len(score[key])!=2:
            raise AssertionError(f'Replay pair requires exactly two {key} entries')
    if any(not isinstance(v,str) or not v.strip() for v in score['flight_ids']):
        raise AssertionError('Replay pair needs two flight identities')
    for value in score['rmse_hz']: finite(value,True)
    for value in score['fitted_f0_hz']:
        finite(value)
        if value<=0: raise AssertionError('Replay fitted tones must be positive')
    finite(score['combined_rmse_hz'],True)
    finite(score['clock_offset_s'])
    finite(score['shared_shift_s'])
    for key in ['latitude_deg','longitude_deg','ellipsoid_height_m']: finite(candidate['observer'][key])
    if abs(candidate['observer']['latitude_deg'])>90 or abs(candidate['observer']['longitude_deg'])>180:
        raise AssertionError('Replay coordinate is out of range')


def decision_checks(observed,expected_status,expected_reason):
    diagnostics=observed.get('diagnostics',{})
    return {'status_matches':observed.get('status')==expected_status,
            'reason_matches':observed.get('reason_code')==expected_reason,
            'acceptance_matches_status':observed.get('accepted_location') is (expected_status=='succeeded'),
            'no_probability_claim':'probability' in diagnostics and diagnostics['probability'] is None,
            'uncalibrated_label':diagnostics.get('calibration')=='uncalibrated'}


def source_aircraft_id(track_id):
    value=track_id.split(':',1)[0].split('#',1)[0].lower()
    # The source also retains '~' non-ICAO addresses. They are legitimate
    # competing tracks and must not be discarded while selecting the fixture.
    address=value[1:] if value.startswith('~') else value
    if len(address)!=6 or any(c not in '0123456789abcdef' for c in address):
        raise AssertionError(f'Unrecognized source track identity: {track_id}')
    return value


def simplified_checks(result,case):
    candidates=result.get('candidates')
    if not isinstance(candidates,list):
        raise AssertionError('Negative-control result must contain a candidate array')
    checks={'expected_status':result.get('status')==case['expected_status'],
            'location_not_verified':result.get('location_verified') is False,
            'ranked_count_consistent':result.get('ranked_count')==len(candidates)}
    if case['expected_status']=='no_candidate':
        checks['empty_candidates']=candidates==[]
        checks['empty_distinct_pairs']=result.get('best_per_flight_pair')==[]
    else:
        checks['unverified_hypotheses_reported']=bool(candidates)
        unavailable=set(case['unavailable_reference_pair_icao24'])
        for candidate in candidates: validate_pair_candidate(candidate)
        checks['incomplete_reference_pair_absent_from_every_candidate']=all(
            {source_aircraft_id(identifier) for identifier in candidate['score']['flight_ids']}!=unavailable
            for candidate in candidates)
    return checks


def validate_partial_fixture(case):
    original_path=ROOT/case['source_input']
    if digest(original_path)!=case['source_input_sha256']:
        raise AssertionError('Incomplete-pair parent source changed')
    original=load_json(original_path)
    fixture=load_json(ROOT/case['input'])
    identifiers=set(case['selected_icao24'])
    selected=sorted([track for track in original['tracks'] if source_aircraft_id(track['flight_id']) in identifiers],key=lambda track:track['flight_id'])
    if fixture['tracks']!=selected or len(selected)!=case['track_count']:
        raise AssertionError('Incomplete-pair fixture must contain every unchanged selected source part, sorted by ID')
    for key,value in original.items():
        if key not in ['tracks','provenance'] and fixture.get(key)!=value:
            raise AssertionError(f'Incomplete-pair fixture changed the original {key}')
    if fixture['provenance'][:len(original['provenance'])]!=original['provenance'] or len(fixture['provenance'])<=len(original['provenance']):
        raise AssertionError('Incomplete-pair fixture must preserve and annotate source provenance')
    return {'source':receipt(original_path),'track_count':len(selected),'trajectory_vertices':sum(len(track['points']) for track in selected),
            'observer_count':len(fixture['observers']),'ridge_sample_counts':[len(event['points']) for event in fixture['events']],
            'unchanged_fields':'Every original field except the declared aircraft subset and appended provenance'}


def verified_upstream_receipt(path):
    path=Path(path).resolve()
    source_receipt=load_json(UPSTREAM/'source-receipt.json')
    if source_receipt.get('upstream_commit')!=PIN:
        raise AssertionError('Upstream source receipt has the wrong commit')
    relative=path.relative_to(UPSTREAM.resolve()).as_posix()
    matches=[row for row in source_receipt['files'] if row['local_path']==relative]
    if len(matches)!=1:
        raise AssertionError(f'No unique pinned source receipt for {relative}')
    expected=matches[0]
    actual=receipt(path)
    if actual['sha256']!=expected['sha256'] or actual['bytes']!=expected['bytes']:
        raise AssertionError(f'Pinned upstream fixture changed: {relative}')
    data=path.read_bytes()
    blob=hashlib.sha1(b'blob '+str(len(data)).encode()+b'\0'+data).hexdigest()
    if blob!=expected['git_blob_sha1']:
        raise AssertionError(f'Pinned upstream Git blob changed: {relative}')
    return actual


class WindowsChildJob:
    """One noninherited kill-on-close job, containing only our suspended child.

    Assigning before ResumeThread prevents descendants from escaping the job.
    Nested jobs are supported on Windows 8+; inability to assign fails closed.
    https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects
    """
    def __init__(self):
        import ctypes
        from ctypes import wintypes as w
        self.ctypes=ctypes
        self.kernel=ctypes.WinDLL('kernel32',use_last_error=True)
        class BasicLimits(ctypes.Structure):
            _fields_=[('process_time',ctypes.c_int64),('job_time',ctypes.c_int64),('flags',w.DWORD),
                      ('minimum_working_set',ctypes.c_size_t),('maximum_working_set',ctypes.c_size_t),
                      ('active_process_limit',w.DWORD),('affinity',ctypes.c_size_t),('priority',w.DWORD),('scheduling',w.DWORD)]
        class IoCounters(ctypes.Structure):
            _fields_=[(name,ctypes.c_uint64) for name in ['read_operations','write_operations','other_operations','read_bytes','write_bytes','other_bytes']]
        class ExtendedLimits(ctypes.Structure):
            _fields_=[('basic',BasicLimits),('io',IoCounters),('process_memory',ctypes.c_size_t),('job_memory',ctypes.c_size_t),
                      ('peak_process_memory',ctypes.c_size_t),('peak_job_memory',ctypes.c_size_t)]
        class ThreadEntry(ctypes.Structure):
            _fields_=[('size',w.DWORD),('usage',w.DWORD),('thread_id',w.DWORD),('process_id',w.DWORD),
                      ('base_priority',w.LONG),('priority_delta',w.LONG),('flags',w.DWORD)]
        self.ThreadEntry=ThreadEntry
        signatures={
            'CreateJobObjectW':([ctypes.c_void_p,w.LPCWSTR],w.HANDLE),
            'SetInformationJobObject':([w.HANDLE,ctypes.c_int,ctypes.c_void_p,w.DWORD],w.BOOL),
            'AssignProcessToJobObject':([w.HANDLE,w.HANDLE],w.BOOL),
            'CloseHandle':([w.HANDLE],w.BOOL),
            'CreateToolhelp32Snapshot':([w.DWORD,w.DWORD],w.HANDLE),
            'Thread32First':([w.HANDLE,ctypes.POINTER(ThreadEntry)],w.BOOL),
            'Thread32Next':([w.HANDLE,ctypes.POINTER(ThreadEntry)],w.BOOL),
            'OpenThread':([w.DWORD,w.BOOL,w.DWORD],w.HANDLE),
            'ResumeThread':([w.HANDLE],w.DWORD),
        }
        for name,(arguments,result) in signatures.items():
            function=getattr(self.kernel,name); function.argtypes=arguments; function.restype=result
        self.handle=self.kernel.CreateJobObjectW(None,None)
        if not self.handle: raise ctypes.WinError(ctypes.get_last_error())
        limits=ExtendedLimits(); limits.basic.flags=0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not self.kernel.SetInformationJobObject(self.handle,9,ctypes.byref(limits),ctypes.sizeof(limits)):
            error=ctypes.WinError(ctypes.get_last_error()); self.close(); raise error

    def assign_and_resume(self,child):
        c=self.ctypes
        if not self.kernel.AssignProcessToJobObject(self.handle,int(child._handle)):
            raise c.WinError(c.get_last_error())
        snapshot=self.kernel.CreateToolhelp32Snapshot(0x4,0)  # TH32CS_SNAPTHREAD
        if snapshot==c.c_void_p(-1).value: raise c.WinError(c.get_last_error())
        resumed=0
        try:
            entry=self.ThreadEntry(); entry.size=c.sizeof(entry)
            available=self.kernel.Thread32First(snapshot,c.byref(entry))
            while available:
                if entry.process_id==child.pid:
                    thread=self.kernel.OpenThread(0x2,False,entry.thread_id)  # THREAD_SUSPEND_RESUME
                    if not thread: raise c.WinError(c.get_last_error())
                    try:
                        if self.kernel.ResumeThread(thread)==0xffffffff: raise c.WinError(c.get_last_error())
                        resumed+=1
                    finally: self.kernel.CloseHandle(thread)
                available=self.kernel.Thread32Next(snapshot,c.byref(entry))
        finally: self.kernel.CloseHandle(snapshot)
        if not resumed: raise RuntimeError('Created child has no resumable thread; refusing unowned execution')

    def close(self):
        if self.handle:
            self.kernel.CloseHandle(self.handle)
            self.handle=None


class Validation:
    def __init__(self,args):
        self.args=args
        self.output=args.output.resolve()
        self.output.mkdir(parents=True,exist_ok=False)
        (self.output/'logs').mkdir()
        self.began=time.monotonic()
        self.report={'schema_version':1,'status':'running','passed':False,
            'profile':'implemented_native_validation','upstream_commit':PIN,
            'full_upstream_parity':False,'independent_location_accuracy_established':False,
            'started_utc':datetime.now(timezone.utc).isoformat(),
            'project':str(ROOT),'output':str(self.output),
            'provenance':{'python':sys.version,'platform':platform.platform(),'argv':sys.argv},
            'stages':[], 'explicit_omissions':[], 'coverage_catalog_is_not_execution_evidence':True}
        self.executable=args.exe.resolve() if args.exe else ROOT/'target'/'release'/('geosonnar.exe' if os.name=='nt' else 'geosonnar')
        self.save()

    def save(self):
        self.report['elapsed_s']=time.monotonic()-self.began
        write_json(self.output/'summary.json',self.report)

    def stage(self,name,operation):
        row={'name':name,'status':'running'}
        self.report['stages'].append(row)
        self.save()
        print(json.dumps({'stage':name,'status':'running'}),flush=True)
        began=time.monotonic()
        try:
            details=operation()
            row.update(status='passed',details=details)
        except Exception as error:
            row.update(status='failed',error=f'{type(error).__name__}: {error}')
        row['elapsed_s']=time.monotonic()-began
        self.save()
        print(json.dumps({'stage':name,'status':row['status'],'elapsed_s':row['elapsed_s'],'error':row.get('error')}),flush=True)
        return row['status']=='passed'

    def omitted(self,name,reason):
        self.report['explicit_omissions'].append({'stage':name,'reason':reason})
        self.report['stages'].append({'name':name,'status':'not_run_by_request','reason':reason})
        self.save()

    def blocked(self,name,reason):
        self.report['stages'].append({'name':name,'status':'blocked','reason':reason})
        self.save()

    def command(self,name,arguments,expected=0,environment=None,timeout=None):
        prefix=self.output/'logs'/name
        began=time.monotonic()
        command=list(map(str,arguments))
        record={'command':command,'cwd':str(ROOT),'expected_returncode':expected,
                'environment_overrides':environment or {}}
        env=os.environ.copy()
        if environment:
            env.update(environment)
        try:
            with prefix.with_suffix('.stdout').open('wb') as stdout,prefix.with_suffix('.stderr').open('wb') as stderr:
                job=WindowsChildJob() if os.name=='nt' else None
                options={'creationflags':subprocess.CREATE_NO_WINDOW|0x4} if job else {'start_new_session':True}
                try:
                    with subprocess.Popen(command,cwd=ROOT,env=env,stdout=stdout,stderr=stderr,**options) as child:
                        try:
                            if job: job.assign_and_resume(child)
                            returncode=child.wait(timeout=timeout or self.args.timeout)
                        except BaseException:
                            if job:
                                job.close()
                            else:
                                try: os.killpg(child.pid,signal.SIGKILL)
                                except ProcessLookupError: pass
                            if child.poll() is None: child.kill()
                            child.wait(timeout=15)
                            raise
                finally:
                    if job: job.close()
            record['returncode']=returncode
        except Exception as error:
            record['execution_error']=f'{type(error).__name__}: {error}'
            record['elapsed_s']=time.monotonic()-began
            write_json(prefix.with_suffix('.command.json'),record)
            raise
        record['elapsed_s']=time.monotonic()-began
        write_json(prefix.with_suffix('.command.json'),record)
        if returncode!=expected:
            raise RuntimeError(f'{name}: expected exit {expected}, got {returncode}; see {prefix.with_suffix(".stderr")}')
        return record

    def snapshot(self):
        self.report['provenance']['source_sha256']=code_snapshot()
        self.report['provenance']['contract_sha256']=contract_snapshot()
        self.report['provenance']['evaluation_isolation']='Evaluation files are hashed as opaque bytes for immutability; labels/results are parsed only after native predictions and never enter native inputs. Same filesystem, not a sealed holdout.'
        for name,command in [('rustc-version',['rustc','--version']),('cargo-version',['cargo','--version']),('ffmpeg-version',['ffmpeg','-version'])]:
            self.command(name,command)
            self.report['provenance'][name]=(self.output/'logs'/f'{name}.stdout').read_text(encoding='utf-8',errors='replace').splitlines()[0]
        if getattr(self.args,'gui',False):
            helpers=media_helper_paths(self.args)
            self.report['provenance']['gui_media_helpers']={name:receipt(path) for name,path in helpers.items()}
            for name,path in helpers.items():
                self.command('gui-'+name+'-version',[path,'-version'])
        self.report['provenance']['catalog']=receipt(ROOT/'benchmarks'/'upstream_catalog.json')
        self.report['provenance']['replay_manifest']=receipt(ROOT/'benchmarks'/'replay_baselines.json')
        self.report['provenance']['upstream_source_receipt']=receipt(UPSTREAM/'source-receipt.json')
        validate_replay_manifest(load_json(ROOT/'benchmarks'/'replay_baselines.json'))
        return {'source_file_count':len(self.report['provenance']['source_sha256'])}

    def media_integration(self,name,target,expected_tests):
        helpers=self.report['provenance'].get('gui_media_helpers')
        if not helpers:
            raise AssertionError('Installed-media tools were not bound by the provenance stage')
        for helper in helpers.values():
            if receipt(helper['path'])!=helper:
                raise AssertionError('Installed-media helper changed before integration')
        environment={'GEOSONNAR_TEST_'+key.upper():value['path'] for key,value in helpers.items()}
        arguments=['cargo','test','--offline','--locked','--features','gui','--test',target,'--','--ignored']
        execution=self.command(name,arguments,environment=environment)
        stdout=(self.output/'logs'/f'{name}.stdout').read_text(encoding='utf-8',errors='replace')
        observed=require_executed_tests(stdout,expected_tests)
        for helper in helpers.values():
            if receipt(helper['path'])!=helper:
                raise AssertionError('Installed-media helper changed during integration')
        return {'execution':execution,'executed_tests':observed,'media_helpers':helpers,'synthetic_media_only':True}

    def frozen_decisions(self):
        source=UPSTREAM/'decision-inputs.json'
        source_identity=verified_upstream_receipt(source)
        cases=load_json(source)['cases']
        if len(cases)!=16 or len({c['case_id'] for c in cases})!=16:
            raise AssertionError('Expected 16 unique pinned decision cases')
        destination=self.output/'decisions'; destination.mkdir()
        produced=[]
        for case in cases:
            identifier=case['case_id']
            if not identifier.startswith('decision-') or not identifier[len('decision-'):].isdigit():
                raise AssertionError('Unsafe decision case identifier')
            # Only producer evidence goes to the Rust executable. Labels and
            # control-kind metadata do not enter its process input.
            evidence={key:case[key] for key in ['events','candidates','coverage','matching_diagnostics']}
            input_path=destination/f'{identifier}-input.json'
            result_path=destination/f'{identifier}-decision.json'
            write_json(input_path,evidence)
            execution=self.command(identifier,[self.executable,'decide','--input',input_path,'--out',result_path])
            produced.append({'case_id':identifier,'split':case['split'],'input':receipt(input_path),'result':receipt(result_path),'execution':execution})
        # Evaluation boundary: all 16 native decisions already exist.
        truth_path=UPSTREAM/'truth'/'decision-labels.json'
        truth_identity=verified_upstream_receipt(truth_path)
        labels=load_json(truth_path)
        if set(labels)!={case['case_id'] for case in cases}:
            raise AssertionError('Decision labels do not match the produced cases')
        rows=[]
        for item in produced:
            label=labels[item['case_id']]
            observed=load_json(item['result']['path'])
            expected_status='inconclusive' if label['expect_inconclusive'] else 'succeeded'
            expected_reason=REASONS[label['control_kind']]
            checks=decision_checks(observed,expected_status,expected_reason)
            rows.append({**item,'control_kind':label['control_kind'],'expected_status':expected_status,'expected_reason':expected_reason,
                         'observed_status':observed['status'],'observed_reason':observed['reason_code'],'checks':checks,'passed':all(checks.values())})
        result={'passed':all(row['passed'] for row in rows),'cases':rows,'inputs':source_identity,
                'evaluation_only_labels':truth_identity,'labels_parsed_after_all_native_decisions':True,
                'isolation_limit':'Procedural separation on the same filesystem; not a sealed holdout.',
                'scope':'Frozen simulated decision evidence, not audio detection or real location validation.'}
        write_json(destination/'report.json',result)
        if not result['passed']:
            raise AssertionError(f'Decision fixtures mismatch: {[r["case_id"] for r in rows if not r["passed"]]}')
        return {'report':receipt(destination/'report.json'),'case_count':16,'development_cases':8,'held_out_cases':8,'labels_parsed_after_decisions':True}

    def audio(self):
        source=self.args.source.resolve() if self.args.source else self.discover_source()
        self.report['provenance']['raw_recording']=receipt(source)
        child=self.output/'audio'
        execution=self.command('audio-audit',[sys.executable,ROOT/'analysis'/'benchmark_audio.py','--exe',self.executable,'--output',child,'--source',source])
        report=load_json(child/'report.json')
        if report.get('passed') is not True:
            raise AssertionError('Audio child report did not pass')
        return {'report':receipt(child/'report.json'),'execution':execution,'coverage_gaps':report['coverage_gaps']}

    @staticmethod
    def discover_source():
        source_folder=ROOT/'Known Videos with coordinates'
        files=[p for p in source_folder.iterdir() if p.is_file() and p.suffix.lower() in ['.mp4','.mkv','.mov','.wav','.flac','.m4a','.webm']]
        if len(files)!=1:
            raise ValueError(f'Expected one source recording, found {len(files)}; pass --source explicitly')
        return files[0]

    def regional_replays(self):
        manifest=validate_replay_manifest(load_json(ROOT/'benchmarks'/'replay_baselines.json'))
        destination=self.output/'regional-replays'; destination.mkdir()
        produced=[]
        for case in manifest['cases']:
            path=ROOT/case['input']
            if digest(path)!=case['input_sha256']:
                raise AssertionError(f'Frozen replay input changed: {path}')
            result_path=destination/f'{case["id"]}-results.json'
            execution=self.command(case['id'],[self.executable,'rank','--input',path,'--out',result_path])
            produced.append({'case':case,'input':receipt(path),'result_path':result_path,'execution':execution})
        # Both searches are complete before inspecting prior outputs or location
        # references. No evaluation information is forwarded to either search.
        sys.path.insert(0,str(ROOT/'analysis'))
        from benchmark_faint import wgs84_distance_m
        references_path=ROOT/'benchmarks'/'truth'/'reference-locations.json'
        references=load_json(references_path)
        thresholds=manifest['tolerances']
        rows=[]
        for item in produced:
            case=item['case']; current=load_json(item['result_path'])
            baseline_path=ROOT/case['baseline']
            if digest(baseline_path)!=case['baseline_sha256']:
                raise AssertionError(f'Frozen prior replay output changed: {baseline_path}')
            prior=load_json(baseline_path)
            if not current.get('candidates') or not prior.get('candidates'):
                raise AssertionError(f'{case["id"]} has no winning candidate')
            winner,expected=current['candidates'][0],prior['candidates'][0]
            validate_pair_candidate(winner)
            validate_pair_candidate(expected)
            location=[winner['observer']['latitude_deg'],winner['observer']['longitude_deg']]
            old_location=[expected['observer']['latitude_deg'],expected['observer']['longitude_deg']]
            drift=wgs84_distance_m(location,old_location)
            rmse_change=abs(winner['score']['combined_rmse_hz']-expected['score']['combined_rmse_hz'])
            per_event=[abs(a-b) for a,b in zip(winner['score']['rmse_hz'],expected['score']['rmse_hz'])]
            clock_change=abs(winner['score']['clock_offset_s']-expected['score']['clock_offset_s'])
            errors={name:wgs84_distance_m(location,info['coordinate']) for name,info in references.items()}
            checks={'search_completed':current['status']=='hypotheses_ranked','same_flight_pair':winner['score']['flight_ids']==expected['score']['flight_ids'],
                    'coordinate_replay_within_2m':drift<=thresholds['winner_coordinate_drift_m'],
                    'combined_score_replay_within_tolerance':rmse_change<=thresholds['rmse_absolute_difference_hz'],
                    'per_event_scores_replay_within_tolerance':all(e<=thresholds['rmse_absolute_difference_hz'] for e in per_event),
                    'same_declared_tone_fits':winner['score']['fitted_f0_hz']==expected['score']['fitted_f0_hz'],
                    'clock_replay_within_tolerance':clock_change<=thresholds['clock_absolute_difference_s'],
                    'not_automatically_verified':current['location_verified'] is False,
                    'input_unchanged':digest(item['input']['path'])==case['input_sha256']}
            rows.append({'case_id':case['id'],'passed':all(checks.values()),'checks':checks,'winner':winner,
                         'drift_m':drift,'combined_rmse_difference_hz':rmse_change,'event_rmse_differences_hz':per_event,'clock_difference_s':clock_change,
                         'horizontal_error_m':errors,'tiers_m':{name:{str(t):error<=t for t in [100,500,1000]} for name,error in errors.items()},
                         'tiers_are_reported_measurements_not_replay_acceptance_gates':True,
                         'independent_location_accuracy_eligible':False,'search_provenance':current['provenance'],
                         'input':item['input'],'result':receipt(item['result_path']),'prior_baseline':receipt(baseline_path),'execution':item['execution']})
        result={'passed':all(row['passed'] for row in rows),'cases':rows,'tolerances':thresholds,
                'evaluation_references':receipt(references_path),'references_parsed_after_both_searches':True,
                'method':'WGS84 inverse; frozen native-result reproducibility and separately labelled distances to two published references.',
                'scope':'Source-informed regional development replays. Neither published coordinate is surveyed microphone truth.'}
        write_json(destination/'report.json',result)
        if not result['passed']:
            raise AssertionError(f'Replay checks failed: {[r["case_id"] for r in rows if not r["passed"]]}')
        return {'report':receipt(destination/'report.json'),'cases':[{key:r[key] for key in ['case_id','drift_m','combined_rmse_difference_hz','clock_difference_s','horizontal_error_m','tiers_m']} for r in rows]}

    def simplified_negative(self):
        return self.negative_control('negative_simplified','simplified-archive')

    def partial_pair_negative(self):
        return self.negative_control('negative_partial_pair','incomplete-pair')

    def negative_control(self,key,folder):
        case=validate_replay_manifest(load_json(ROOT/'benchmarks'/'replay_baselines.json'))[key]
        source=ROOT/case['input']
        if digest(source)!=case['input_sha256']:
            raise AssertionError('Simplified-track negative fixture changed')
        derivation=validate_partial_fixture(case) if key=='negative_partial_pair' else None
        destination=self.output/folder; destination.mkdir()
        result_path=destination/'results.json'
        execution=self.command(case['id'],[self.executable,'rank','--input',source,'--out',result_path],expected=case['expected_exit_code'])
        result=load_json(result_path)
        checks=simplified_checks(result,case)
        checks['input_unchanged']=digest(source)==case['input_sha256']
        baseline=None
        if key=='negative_simplified':
            # Prior-result assertions are evaluated only after native search.
            baseline_path=ROOT/case['baseline']
            if digest(baseline_path)!=case['baseline_sha256']:
                raise AssertionError('Simplified-archive prior result changed')
            baseline=receipt(baseline_path)
            checks['prior_status_and_unavailable_pair_contract']=all(simplified_checks(load_json(baseline_path),case).values())
        report={'passed':all(checks.values()),'checks':checks,'input':receipt(source),'result':receipt(result_path),'execution':execution,
                'case_id':case['id'],'scope':case['scope'],'prior_result':baseline,'derivation':derivation,
                'independent_location_accuracy_eligible':False,'ranking_is_not_location_acceptance':True}
        write_json(destination/'report.json',report)
        if not report['passed']:
            raise AssertionError(f'Negative support contract failed: {case["id"]}')
        return report

    def faint(self):
        script=ROOT/'analysis'/'benchmark_faint.py'
        if not script.is_file():
            raise FileNotFoundError('Required faint-event benchmark adapter is absent')
        destination=self.output/'faint'
        execution=self.command('faint-event',[sys.executable,script,'--project',ROOT,'--out',destination,'--exe',self.executable])
        result=load_json(destination/'report.json')
        if result.get('passed') is not True:
            raise AssertionError('Faint-event child report did not pass its declared development gate')
        return {'report':receipt(destination/'report.json'),'execution':execution,'checks':result['checks'],
                'horizontal_error_m':result['horizontal_error_m'],'tiers':result['tiers'],
                'historical_upstream_100m_passed':result['historical_upstream_100m_passed'],
                'independent_location_accuracy_eligible':False}

    def coverage(self):
        catalog=load_json(ROOT/'benchmarks'/'upstream_catalog.json')
        incomplete=[]
        for family in catalog['families']:
            state=family['support']
            if family.get('gaps') or state in ['partial','not_implemented','not_run_feature_gap','evidence_unavailable','catalogued_not_reexecuted','adaptation_planned']:
                incomplete.append({'id':family['id'],'support':state,'gaps':family.get('gaps',[]),'sources':family['sources']})
        self.report['unsupported_or_partial_upstream_families']=incomplete
        self.report['coverage_note']='These are explicit capability/evidence gaps, not skipped tests counted as passes. Only the named executed stages are validated.'
        return {'catalog':receipt(ROOT/'benchmarks'/'upstream_catalog.json'),'incomplete_family_count':len(incomplete),'full_upstream_parity':False}

    def unchanged(self):
        after=code_snapshot()
        before=self.report['provenance']['source_sha256']
        changed=[p for p in before.keys()|after.keys() if before.get(p)!=after.get(p)]
        if changed:
            raise AssertionError(f'Source changed during validation: {changed}')
        after_contracts=contract_snapshot()
        before_contracts=self.report['provenance']['contract_sha256']
        changed_contracts=[p for p in before_contracts.keys()|after_contracts.keys() if before_contracts.get(p)!=after_contracts.get(p)]
        if changed_contracts:
            raise AssertionError(f'Benchmark contract/evaluation files changed during validation: {changed_contracts}')
        final_executable=receipt(self.executable)
        initial=self.report['provenance'].get('executable')
        if initial is not None and initial['sha256']!=final_executable['sha256']:
            raise AssertionError('Executable changed during validation')
        if self.report['provenance'].get('gui_executable'):
            original=self.report['provenance']['gui_executable']
            if receipt(original['path'])['sha256']!=original['sha256']:
                raise AssertionError('GUI executable changed during validation')
        for helper in self.report['provenance'].get('gui_media_helpers',{}).values():
            if receipt(helper['path'])!=helper:
                raise AssertionError('Installed-media helper changed during validation')
        return {'source_files_unchanged':True,'benchmark_contracts_and_evaluation_files_unchanged':True,'executable':final_executable}

    def execute(self):
        gui_features=['--features','gui'] if getattr(self.args,'gui',False) else []
        prerequisites=self.stage('provenance_and_tools',self.snapshot)
        self.stage('harness_controls',lambda:self.command('harness-controls',[sys.executable,'-m','unittest','discover','-s','benchmarks','-p','test_runner.py','-v']))
        self.stage('format',lambda:self.command('format',['cargo','fmt','--all','--','--check']))
        if self.args.skip_tests:
            self.omitted('native_tests','--skip-tests explicitly requested; this run cannot certify the test suite')
            if gui_features:
                for name in ['gui_media_integration','review_media_integration']:
                    self.omitted(name,'--skip-tests explicitly requested; installed-media integration was not run')
        else:
            self.stage('native_tests',lambda:self.command('native-tests',['cargo','test','--offline','--locked',*gui_features,'--all-targets']))
            if gui_features:
                self.stage('gui_media_integration',lambda:self.media_integration('gui-media-integration','gui_bridge_validation',GUI_BRIDGE_MEDIA_TESTS))
                self.stage('review_media_integration',lambda:self.media_integration('review-media-integration','review_media_validation',REVIEW_MEDIA_TESTS))
        self.stage('clippy',lambda:self.command('clippy',['cargo','clippy','--offline','--locked',*gui_features,'--all-targets','--','-D','warnings']))
        if self.args.skip_build:
            self.omitted('static_crt_build','--skip-build explicitly requested; existing executable identity is retained without a fresh build claim')
            executable_ready=self.stage('existing_executable',lambda:receipt(self.executable))
        else:
            executable_ready=self.stage('static_crt_build',lambda:self.command('release-build',['cargo','build','--release','--offline','--locked',*gui_features],
                environment={'RUSTFLAGS':'-C target-feature=+crt-static','CARGO_TARGET_DIR':str(ROOT/'target')}))
            if executable_ready:
                executable_ready=self.stage('built_executable_identity',lambda:receipt(self.executable))
                if gui_features:
                    self.report['provenance']['gui_executable']=receipt(ROOT/'target'/'release'/'geosonnar-gui.exe')
        if prerequisites and executable_ready:
            self.report['provenance']['executable']=receipt(self.executable)
            for name,operation in [('frozen_decision_fixtures',self.frozen_decisions),('codec_and_real_audio',self.audio),
                                   ('regional_replays',self.regional_replays),('simplified_archive_nonacceptance',self.simplified_negative),
                                   ('incomplete_pair_abstention',self.partial_pair_negative),('known_clock_faint',self.faint)]:
                self.stage(name,operation)
        else:
            for name in ['frozen_decision_fixtures','codec_and_real_audio','regional_replays','simplified_archive_nonacceptance','incomplete_pair_abstention','known_clock_faint']:
                self.blocked(name,'Required tools/provenance or executable build failed; an older executable was not substituted')
        self.stage('upstream_coverage_inventory',self.coverage)
        if prerequisites:
            self.stage('source_immutability',self.unchanged)
        failed=[s['name'] for s in self.report['stages'] if s['status'] in ['failed','blocked']]
        self.report['failed_stages']=failed
        self.report['passed']=not failed
        self.report['status']='failed' if failed else ('passed_with_explicit_omissions' if self.report['explicit_omissions'] else 'passed_implemented_profile')
        self.report['finished_utc']=datetime.now(timezone.utc).isoformat()
        self.save()
        print(json.dumps({'summary':str(self.output/'summary.json'),'status':self.report['status'],'failed_stages':failed,
                          'full_upstream_parity':False,'elapsed_s':self.report['elapsed_s']}),flush=True)
        return 1 if failed else 0


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True,help='New run directory; existing directories are refused')
    parser.add_argument('--source',type=Path,help='Pinned raw source recording; otherwise discover the single local recording')
    parser.add_argument('--exe',type=Path,help='Existing executable; requires explicit --skip-build')
    parser.add_argument('--skip-build',action='store_true',help='Explicit omission recorded in report; use an existing executable')
    parser.add_argument('--skip-tests',action='store_true',help='Explicit omission recorded in report; useful only for limited diagnostics')
    parser.add_argument('--gui',action='store_true',help='Include the Windows GUI, lifecycle tests, and installed-media-tool integration')
    parser.add_argument('--ffmpeg',type=Path,help='Actual FFmpeg executable for GUI integration (otherwise GEOSONNAR_TEST_FFMPEG or installed helper)')
    parser.add_argument('--ffprobe',type=Path,help='Actual FFprobe executable for GUI integration (otherwise GEOSONNAR_TEST_FFPROBE or installed helper)')
    parser.add_argument('--timeout',type=int,default=3600,help='Maximum seconds for each child command')
    args=parser.parse_args()
    if args.exe and not args.skip_build:
        parser.error('--exe requires --skip-build; otherwise the fresh static-CRT release is used')
    if args.timeout<=0:
        parser.error('--timeout must be positive')
    try:
        validation=Validation(args)
    except Exception as error:
        parser.error(str(error))
    return validation.execute()


if __name__=='__main__':
    raise SystemExit(main())

