"""Local, label-free validation of a registered private recording.

This runs the frozen native program, never imports private truth, and never
interprets a proposal or a successful acoustic fit as a confirmed aircraft.
All channels and all group representatives are accounted for. The current
native fitter requires two nonoverlapping groups; unpairable groups remain
explicitly unevaluated. No location search or network request is performed.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

from benchmark_audio_priors import (digest, finite, load_json, receipt, require,
                                    validate_fit_report, write_json)
from benchmark_discovery import validate_prediction
from run_validation import WindowsChildJob

ROOT = Path(__file__).resolve().parents[1]
DISCOVERY_DEFAULTS = {
    'chunk_s': 120., 'overlap_s': 60., 'min_frequency_hz': 40.,
    'max_frequency_hz': 1600., 'window_s': 2.048, 'hop_s': .5,
    'min_contrast': 2.5, 'min_duration_s': 12., 'min_support': .65,
    'min_drop_hz': 4., 'min_fractional_drop': .03, 'max_gap_s': 2.,
    'max_fragment_gap_s': 20., 'max_peaks_per_frame': 60,
}
FIT_DEFAULTS = {
    'sound_speed_mps': 343., 'speed_bounds_mps': [10., 280.],
    'slant_bounds_m': [100., 30000.], 'width_bounds_s': [.5, 1000.],
    'center_padding_spans': 1., 'min_samples': 20, 'min_duration_s': 12.,
    'min_drop_hz': 4., 'min_drop_sigma': 4., 'profile_sigma_multiplier': 1.,
    'profile_grid_size': 65, 'max_iterations': 300,
}
PADDING_DEFAULTS = {
    'frequency_margin_hz': 2., 'timing_margin_s': 2.5, 'speed_fraction': .2,
    'slant_fraction': .35, 'slant_min_margin_m': 500.,
    'frequency_step_hz': .5, 'shift_step_s': .5,
}
REVIEW_SCOPE = (
    'Agent diagnostic inclusion of every automatic acoustic group, without '
    'source identities or truth labels. Keep_for_matching authorizes acoustic '
    'fitting only: it does not confirm an aircraft, a human review, a date, '
    'or a location. One deterministically chosen ridge per group is fitted; '
    'other harmonic ridges remain inventoried but unscored.'
)
REPRESENTATIVE_POLICY = 'Most measured samples, then highest median contrast, then lexical event ID.'
PAIR_POLICY = (
    'Visit uncovered representatives by first measured time, last measured '
    'time, group ID and event ID. Pair with the first compatible uncovered '
    'representative, otherwise the first compatible covered representative. '
    'Compatibility requires strictly disjoint actual measured time intervals. '
    'With no compatible partner report unpairable; never duplicate a group '
    'or manufacture an event. Order each pair chronologically.'
)


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def native_snapshot():
    paths = set((ROOT / 'src').glob('*.rs')) | set((ROOT / 'src').glob('*.html'))
    paths.update(ROOT / name for name in ('Cargo.toml', 'Cargo.lock'))
    return {str(path.relative_to(ROOT)): digest(path) for path in sorted(paths)}


def harness_snapshot():
    names = ['analysis/validate_private_audio.py', 'analysis/benchmark_audio_priors.py',
             'analysis/benchmark_discovery.py', 'analysis/benchmark_reviewed_search.py',
             'analysis/benchmark_faint.py', 'benchmarks/run_validation.py']
    return {name: digest(ROOT / name) for name in names}


def run_command(arguments, prefix, timeout, allowed=(0,)):
    """Own the whole native/decoder process tree; persist errors, never skip."""
    prefix = Path(prefix)
    argv = list(map(str, arguments))
    row = {'command': argv, 'timeout_s': timeout, 'allowed_returncodes': list(allowed)}
    started = time.monotonic()
    try:
        with prefix.with_suffix('.stdout').open('wb') as stdout, prefix.with_suffix('.stderr').open('wb') as stderr:
            job = WindowsChildJob() if os.name == 'nt' else None
            options = {'creationflags': subprocess.CREATE_NO_WINDOW | 0x4} if job else {'start_new_session': True}
            try:
                with subprocess.Popen(argv, cwd=ROOT, stdout=stdout, stderr=stderr, **options) as child:
                    try:
                        if job:
                            job.assign_and_resume(child)
                        row['returncode'] = child.wait(timeout=timeout)
                    except BaseException:
                        if job:
                            job.close()
                        else:
                            try:
                                os.killpg(child.pid, signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        if child.poll() is None:
                            child.kill()
                        child.wait(timeout=15)
                        raise
            finally:
                if job:
                    job.close()
        require(row['returncode'] in allowed, f'Native command failed with exit {row["returncode"]}; inspect private command logs')
    except BaseException as error:
        row['execution_error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        row['elapsed_s'] = time.monotonic() - started
        write_json(prefix.with_suffix('.command.json'), row)
    return row


def check_receipt(actual, expected, label):
    require(isinstance(actual, dict), f'Missing {label} receipt')
    require(actual.get('sha256') == expected['sha256'] and actual.get('bytes') == expected['bytes'],
            f'{label} receipt belongs to different bytes')
    require(path_identity(actual['path']) == path_identity(expected['path']), f'{label} receipt belongs to a different path')


def path_identity(path):
    # Rust canonicalize uses Windows extended paths while Python receipts use
    # ordinary absolute paths. They identify the same registered local bytes.
    value = str(Path(path).resolve())
    if os.name == 'nt':
        if value.startswith('\\\\?\\UNC\\'):
            value = '\\\\' + value[8:]
        elif value.startswith('\\\\?\\'):
            value = value[4:]
    return os.path.normcase(value)


def require_finite_tree(value):
    if isinstance(value, float):
        require(finite(value), 'Artifact contains a nonfinite number')
    elif isinstance(value, dict):
        for item in value.values():
            require_finite_tree(item)
    elif isinstance(value, list):
        for item in value:
            require_finite_tree(item)


def validate_discovery(prediction, source_receipt, channel, channels, audio_duration):
    require_finite_tree(prediction)
    require(prediction.get('schema_version') == 1, 'Unsupported discovery schema')
    require(prediction.get('source_sha256') == source_receipt['sha256'], 'Discovery belongs to a different source hash')
    require(path_identity(prediction['source']) == path_identity(source_receipt['path']), 'Discovery belongs to a different source path')
    require(prediction.get('settings') == DISCOVERY_DEFAULTS, 'Native discovery defaults changed')
    meta = prediction['source_metadata']
    require(meta.get('selected_audio_stream') == '0:a:0' and meta.get('selected_channel_index') == channel,
            'Discovery used a different stream or channel')
    require(meta.get('available_channels') == channels and meta.get('channels_averaged') is False,
            'Channel count or channel preservation is inconsistent')
    require(meta.get('sample_rate_hz') == 4000 and type(meta.get('decoded_samples')) is int and meta['decoded_samples'] > 0,
            'Invalid decoded sample accounting')
    duration = meta['decoded_samples'] / meta['sample_rate_hz']
    require(finite(meta.get('duration_s')) and abs(meta['duration_s'] - duration) < 1e-8, 'Decode duration contradicts sample count')
    require(abs(duration - audio_duration) <= .1, 'Decoded audio duration differs from inspected audio stream by over 0.1 seconds')
    groups = validate_prediction(prediction, duration)
    require(prediction.get('status') == ('events_proposed' if groups else 'no_events'), 'Discovery status contradicts proposal count')
    reports = prediction.get('groups')
    require(isinstance(reports, list) and len(reports) == len(groups), 'Missing group inventory')
    require({g['harmonic_group_id'] for g in reports} == set(groups), 'Group inventory membership differs')
    for group in reports:
        members = groups[group['harmonic_group_id']]
        require(group.get('review_status') == 'unreviewed' and group.get('aircraft_identity_confirmed') is False,
                'Discovery cannot confirm aircraft or pre-review groups')
        require(group.get('ridge_count') == len(members), 'Group ridge count differs')
        require(finite(group.get('start_s')) and finite(group.get('end_s')) and 0 <= group['start_s'] < group['end_s'] <= duration + .1,
                'Invalid group interval')
        for event in members:
            require(group['start_s'] <= event['start_s'] < event['end_s'] <= group['end_s'], 'Event outside group interval')
            require(3 <= len(event['ridge']) <= 100000, 'Ridge sample count outside native review domain')
            require(all(p['weight'] > 0 for p in event['ridge']), 'Nonpositive measured contrast weight')
            for name in ('median_contrast', 'frequency_sigma_hz'):
                require(finite(event['quality'].get(name)) and event['quality'][name] > 0, f'Invalid quality field {name}')
    return groups


def chronological(event):
    return (event['ridge'][0]['time_s'], event['ridge'][-1]['time_s'], event['harmonic_group_id'], event['event_id'])


def representatives(groups):
    return sorted((min(events, key=lambda e: (-len(e['ridge']), -e['quality']['median_contrast'], e['event_id']))
                   for events in groups.values()), key=chronological)


def compatible(one, two):
    return (one['harmonic_group_id'] != two['harmonic_group_id'] and
            (one['ridge'][-1]['time_s'] < two['ridge'][0]['time_s'] or
             two['ridge'][-1]['time_s'] < one['ridge'][0]['time_s']))


def pair_plan(selected):
    ordered = sorted(selected, key=chronological)
    require(len({e['harmonic_group_id'] for e in ordered}) == len(ordered), 'Duplicate group representative')
    covered, pairs, unpairable = set(), [], []
    for event in ordered:
        if event['event_id'] in covered:
            continue
        candidates = [e for e in ordered if e['event_id'] not in covered and compatible(event, e)]
        if not candidates:
            candidates = [e for e in ordered if e['event_id'] in covered and compatible(event, e)]
        if not candidates:
            unpairable.append(event)
            continue
        partner = candidates[0]
        pairs.append(sorted((event, partner), key=chronological))
        covered.update((event['event_id'], partner['event_id']))
    require(covered | {e['event_id'] for e in unpairable} == {e['event_id'] for e in ordered}, 'Pair plan omitted a representative')
    return pairs, unpairable


def choice(event):
    return {key: event[key] for key in ('harmonic_group_id', 'event_id')}


def review_selection(discovery, discovery_receipt, groups):
    return {'schema_version': 2, 'discovery_sha256': discovery_receipt['sha256'],
            'source_sha256': discovery['source_sha256'],
            'selected_channel_index': discovery['source_metadata']['selected_channel_index'],
            'location_verified': False, 'review_actor': 'agent', 'scope': REVIEW_SCOPE,
            'groups': [{'harmonic_group_id': name, 'review': 'keep_for_matching',
                        'event_ids': sorted(e['event_id'] for e in events)} for name, events in sorted(groups.items())]}


def validate_priors(priors, manifest, priors_receipt, expected, discovery, selected, returncode):
    require_finite_tree(priors)
    require_finite_tree(manifest)
    require(priors.get('schema_version') == 1 and manifest.get('schema_version') == 1, 'Unsupported prior/receipt schema')
    require(priors.get('settings') == FIT_DEFAULTS and priors.get('search_padding') == PADDING_DEFAULTS,
            'Native acoustic fit or search padding defaults changed')
    require(priors.get('selected_ridges') == [choice(e) for e in selected], 'Fitted selection differs from frozen pair plan')
    require(priors.get('selection_basis') == REPRESENTATIVE_POLICY + ' ' + PAIR_POLICY, 'Fit selection provenance changed')
    reviewed = priors['provenance']['reviewed_audio']
    for name in ('discovery', 'review_selection', 'source'):
        check_receipt(reviewed.get(name), expected[name], name)
    check_receipt(priors['provenance'].get('request'), expected['request'], 'request')
    require(reviewed.get('review_actor') == 'agent' and reviewed.get('review_scope') == REVIEW_SCOPE, 'Review scope or actor changed')
    check_receipt(manifest.get('priors'), priors_receipt, 'priors')
    require(manifest.get('inputs_unchanged') is True and manifest.get('location_verified') is False
            and manifest.get('status') == priors['status'], 'Invalid derivation receipt status')
    for dependencies in (priors['provenance']['dependencies'], manifest['dependencies']):
        require(len(dependencies) == len(expected), 'Incomplete prior input dependency receipts')
        for item in expected.values():
            matches = [r for r in dependencies if path_identity(r['path']) == path_identity(item['path'])]
            require(len(matches) == 1, 'Missing or duplicate prior dependency')
            check_receipt(matches[0], item, 'dependency')
    require(len(priors.get('events', [])) == 2 and len(reviewed.get('selected_ridges', [])) == 2, 'Missing selected measurement evidence')
    for index, event in enumerate(selected):
        require(priors['events'][index]['points'] == [{'t_rec_s': p['time_s'], 'frequency_hz': p['frequency_hz']} for p in event['ridge']],
                'Fitter changed measured samples')
        evidence = reviewed['selected_ridges'][index]
        require(evidence['quality'] == event['quality'] and evidence['contrast_weights'] == [p['weight'] for p in event['ridge']],
                'Fitter dropped measurement quality or contrast provenance')
        require(priors['fits'][index].get('settings') == FIT_DEFAULTS, 'Individual fit defaults changed')
    # Reuse the established independent arrival-time inversion oracle without
    # loading its benchmark contract or any reference/private truth file.
    contract = {'review': {'selected': [choice(e) for e in selected]},
                'regional_geometry': {'sound_speed_mps': FIT_DEFAULTS['sound_speed_mps']}}
    return validate_fit_report(priors, discovery, contract, returncode)


def run(case_path, executable, output, timeout):
    case_path, executable, output = Path(case_path).resolve(), Path(executable).resolve(), Path(output).resolve()
    private_roots = [ROOT / 'output/private-validation', ROOT / 'data/private-validation']
    require(any(output.is_relative_to(root.resolve()) for root in private_roots), 'Outputs must remain in an ignored private-validation directory')
    require(not output.exists(), 'Output directory already exists; use a new directory')
    output.mkdir(parents=True)
    report = {'schema_version': 1, 'started_at_utc': utc_now(), 'status': 'running',
              'technical_validation_passed': False, 'private_case_count': 1, 'channels': [],
              'location_verified': False, 'aircraft_identity_confirmed': False,
              'detection_precision': None, 'detection_recall': None, 'location_error_m': None,
              'network_access_used': False, 'truth_files_read': False,
              'geolocation': {'status': 'not_attempted', 'reason': 'Audio-only diagnostic run. A supported bounded observer region, capture-time interpretation, flight tracks and terrain are separate required inputs.'},
              'limitations': ['No known expected detector result or independent location label is used.',
                              'Channel observations are not independent flyovers or additional private cases.',
                              'Successful acoustic fits do not confirm source identity or location.',
                              'Only one predetermined ridge per group is fitted; other harmonics are inventoried but unscored.',
                              'Unpairable representatives cannot be evaluated by the current two-group native fitting API.']}
    try:
        case = load_json(case_path)
        require(case.get('schema_version') == 1 and case.get('visibility') == 'private_local', 'Expected a registered local private case')
        source = (case_path.parent / case['source']['path']).resolve(strict=True)
        source_receipt, exe_receipt, case_receipt = receipt(source), receipt(executable), receipt(case_path)
        require(source_receipt['sha256'] == case['source']['sha256'] and source_receipt['bytes'] == case['source']['bytes'], 'Source differs from registration')
        require(exe_receipt['sha256'] == case['frozen_program']['sha256'] and exe_receipt['bytes'] == case['frozen_program']['bytes'], 'Program differs from registration')
        frozen_native, frozen_harness = native_snapshot(), harness_snapshot()
        frozen_exe = output / 'geosonnar.exe'
        shutil.copyfile(executable, frozen_exe)
        require(digest(frozen_exe) == exe_receipt['sha256'], 'Program snapshot copy differs')
        logs = output / 'logs'; logs.mkdir()
        report.update(case_id=case['case_id'], source=source_receipt, program=exe_receipt, case=case_receipt)
        run_command([frozen_exe, 'inspect', '--input', source, '--out', output / 'input-inspection.json'], logs / 'inspect', timeout)
        inspected = load_json(output / 'input-inspection.json')
        require(inspected.get('status') == 'input_found' and inspected.get('location_verified') is False, 'Media inspection failed')
        media = inspected['media']
        audio_streams = [s for s in media['metadata']['streams'] if s.get('codec_type') == 'audio']
        require(audio_streams and type(media.get('channels')) is int and media['channels'] > 0, 'No selected audio channels')
        audio_duration = float(audio_streams[0].get('duration', media['duration_s']))
        require(finite(audio_duration) and 0 < audio_duration <= 86401, 'Invalid inspected audio duration')
        require(media['bytes'] == source_receipt['bytes'] and media['channels'] == case['media']['channels'], 'Media differs from registration')
        plan = {'schema_version': 1, 'frozen_at_utc': utc_now(), 'source': source_receipt,
                'program': exe_receipt, 'case': case_receipt, 'native_sources': frozen_native,
                'audit_sources': frozen_harness, 'channels': list(range(media['channels'])),
                'audio_duration_s': audio_duration, 'duration_tolerance_s': .1,
                'discovery_defaults': DISCOVERY_DEFAULTS, 'fit_defaults': FIT_DEFAULTS,
                'search_padding_defaults': PADDING_DEFAULTS, 'review_scope': REVIEW_SCOPE,
                'representative_policy': REPRESENTATIVE_POLICY, 'pair_policy': PAIR_POLICY,
                'fit_pair_count_limit': None, 'command_timeout_s': timeout,
                'expected_scientific_outcome': None, 'location_verified': False}
        write_json(output / 'frozen-plan.json', plan)
        write_json(output / 'summary.json', report)
        payloads = []
        for channel in plan['channels']:
            directory = output / f'channel-{channel}'; directory.mkdir()
            row = {'channel': channel, 'status': 'running', 'fits': [], 'inventory': []}
            report['channels'].append(row)
            write_json(output / 'summary.json', report)
            execution = run_command([frozen_exe, 'discover', '--input', source, '--out', directory / 'discovery', '--channel', channel],
                                    logs / f'channel-{channel}-discovery', timeout)
            discovery_path = directory / 'discovery/discovery.json'
            discovery = load_json(discovery_path)
            groups = validate_discovery(discovery, source_receipt, channel, media['channels'], audio_duration)
            selected = representatives(groups)
            pairs, unpairable = pair_plan(selected)
            payloads.append({'events': discovery['events'], 'groups': discovery['groups']})
            row.update(discovery=receipt(discovery_path), discovery_status=discovery['status'],
                       discovery_execution=execution, decoded_samples=discovery['source_metadata']['decoded_samples'],
                       decoded_duration_s=discovery['source_metadata']['duration_s'],
                       group_count=len(groups), ridge_count=len(discovery['events']), pair_count=len(pairs),
                       unpairable_group_count=len(unpairable), fitted_group_count=0, usable_fit_group_count=0)
            for event in selected:
                row['inventory'].append({**choice(event), 'measured_samples': len(event['ridge']),
                    'start_s': event['ridge'][0]['time_s'], 'end_s': event['ridge'][-1]['time_s'],
                    'median_contrast': event['quality']['median_contrast'],
                    'all_group_event_ids': sorted(e['event_id'] for e in groups[event['harmonic_group_id']]),
                    'other_group_ridges_not_scored': sorted(e['event_id'] for e in groups[event['harmonic_group_id']] if e['event_id'] != event['event_id']),
                    'fit_status': 'unpairable_native_api' if event in unpairable else 'pending',
                    'usable_for_search': None, 'pair_indices': []})
            review_path = directory / 'review-selection.json'
            write_json(review_path, review_selection(discovery, row['discovery'], groups))
            write_json(directory / 'pair-plan.json', {'pairs': [[choice(e) for e in pair] for pair in pairs], 'unpairable': [choice(e) for e in unpairable]})
            write_json(output / 'summary.json', report)
            fitted = {}
            for index, pair in enumerate(pairs):
                request_path = directory / f'pair-{index:04d}-request.json'
                write_json(request_path, {'schema_version': 1, 'discovery': str(discovery_path),
                    'review_selection': str(review_path), 'selected_ridges': [choice(e) for e in pair],
                    'selection_basis': REPRESENTATIVE_POLICY + ' ' + PAIR_POLICY})
                destination = directory / f'pair-{index:04d}'
                run_result = run_command([frozen_exe, 'derive-audio-priors', '--config', request_path, '--out', destination],
                                         logs / f'channel-{channel}-pair-{index:04d}', timeout, (0, 3))
                priors_path = destination / 'priors.json'
                priors, manifest = load_json(priors_path), load_json(destination / 'receipt.json')
                expected = {'discovery': row['discovery'], 'review_selection': receipt(review_path),
                            'source': source_receipt, 'request': receipt(request_path)}
                checks = validate_priors(priors, manifest, receipt(priors_path), expected, discovery, pair, run_result['returncode'])
                row['fits'].append({'pair_index': index, 'selected_ridges': [choice(e) for e in pair],
                    'priors': receipt(priors_path), 'status': priors['status'], 'usable_for_search': priors['usable_for_search'],
                    'technical_checks': checks, 'execution': run_result})
                for event, fit in zip(pair, priors['fits'], strict=True):
                    identifier = event['event_id']
                    require(identifier not in fitted or fitted[identifier] == fit, 'Individual acoustic fit changed with pair partner')
                    fitted[identifier] = fit
                    inventory = next(r for r in row['inventory'] if r['event_id'] == identifier)
                    inventory.update(fit_status=fit['status'], usable_for_search=fit['usable_for_search'], fit=fit)
                    inventory['pair_indices'].append(index)
                row['fitted_group_count'] = len(fitted)
                row['usable_fit_group_count'] = sum(f['usable_for_search'] for f in fitted.values())
                write_json(output / 'summary.json', report)
            require(len(fitted) + len(unpairable) == len(groups), 'Group fit coverage incomplete')
            row.update(status='technical_checks_passed', all_groups_accounted_for=True,
                       all_representatives_fitted=not unpairable)
            print(f'Channel {channel}: {len(groups)} groups; {len(discovery["events"])} ridges; '
                  f'{len(fitted)} fitted representatives; {row["usable_fit_group_count"]} usable; '
                  f'{len(unpairable)} unpairable.', flush=True)
            write_json(output / 'summary.json', report)
        report['channel_proposals_identical'] = all(p == payloads[0] for p in payloads[1:]) if len(payloads) > 1 else None
        require(receipt(source) == source_receipt, 'Source changed during private validation')
        require(receipt(executable) == exe_receipt and digest(frozen_exe) == exe_receipt['sha256'], 'Program changed during private validation')
        require(receipt(case_path) == case_receipt, 'Case registration changed during validation')
        require(native_snapshot() == frozen_native and harness_snapshot() == frozen_harness, 'Native sources or audit harness changed during validation')
        report.update(status='technical_checks_passed', technical_validation_passed=True,
                      inputs_unchanged=True, native_sources_unchanged=True, audit_sources_unchanged=True)
        return 0
    except BaseException as error:
        report.update(status='failed', technical_validation_passed=False, error=f'{type(error).__name__}: {error}')
        return 1
    finally:
        report['completed_at_utc'] = utc_now()
        write_json(output / 'summary.json', report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--case', required=True, type=Path)
    parser.add_argument('--exe', default=ROOT / 'dist/geosonnar.exe', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--timeout', type=int, default=1200, help='Per native command limit in seconds; timeouts fail the run')
    arguments = parser.parse_args()
    require(arguments.timeout > 0, 'Timeout must be positive')
    return run(arguments.case, arguments.exe, arguments.output, arguments.timeout)


if __name__ == '__main__':
    sys.exit(main())
