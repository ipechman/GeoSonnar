"""Fresh audio acceptance audit for the native GeoSonnar executable.

Codec fixtures and pilot-correlation method are adapted from DOPPLERGEIST
scripts/verify_methodology.py at commit 70626ad44fb9c7f4e2f5b225956c1f9281f91c13.
Copyright (c) 2026 Cinna Davis. MIT license: licenses/DOPPLERGEIST-MIT.txt.
Native GSPEC checks, per-channel validation, source/crop checks, and provenance
are GeoSonnar additions. This does not claim automatic event discovery or a
location fix. Every invocation requires a NEW output directory.
"""
from __future__ import annotations
import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path
import platform
import subprocess
import sys
import time
import numpy as np
from scipy import signal
import soundfile as sf

ROOT = Path(__file__).resolve().parents[1]
PIN = '70626ad44fb9c7f4e2f5b225956c1f9281f91c13'
SOURCE_SHA256 = 'b1330ab9550649389b39f9031a43c12a7fb9420a0485fa1678c2b7c3d8daea7d'
RATE, FFT, HOP, BINS = 4000, 4096, 1000, 820


def sha256(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False)+'\n', encoding='utf-8')


def run(command, directory, name, expected=0):
    started=time.monotonic()
    with (directory/f'{name}.stdout').open('wb') as stdout, (directory/f'{name}.stderr').open('wb') as stderr:
        outcome=subprocess.run(list(map(str, command)), stdout=stdout, stderr=stderr, check=False, timeout=360)
    record={'command':list(map(str,command)), 'returncode':outcome.returncode, 'elapsed_s':time.monotonic()-started}
    write_json(directory/f'{name}.command.json',record)
    if outcome.returncode != expected:
        raise RuntimeError(f'{name}: expected exit {expected}, observed {outcome.returncode}; see {name}.stderr')
    return record


def timing_offsets(reference, decoded, centers, radius_s=.25):
    """Independent nonperiodic pilots; positive offset means decoded content later."""
    radius=round(radius_s*RATE)
    results=[]
    for center in centers:
        start,stop=round((center-.75)*RATE),round((center+.75)*RATE)
        if start<radius or stop+radius>len(decoded) or stop>len(reference):
            raise ValueError('Decoded audio does not contain every complete timing pilot')
        pilot=np.asarray(reference[start:stop],dtype=float).copy()
        observed=np.asarray(decoded[start-radius:stop+radius],dtype=float).copy()
        pilot-=pilot.mean(); observed-=observed.mean()
        energy=float(np.dot(pilot,pilot))
        if energy<1e-12:
            raise ValueError('Timing pilot has no usable signal')
        correlation=signal.correlate(observed,pilot,mode='valid',method='fft')
        # FFT convolution is numerically equivalent to upstream's np.convolve.
        energies=signal.convolve(observed**2,np.ones(len(pilot)),mode='valid',method='fft')
        normalized=correlation/np.sqrt(np.maximum(energies*energy,1e-30))
        peak=int(np.argmax(normalized)); coefficient=float(normalized[peak])
        if coefficient<.8 or peak in (0,2*radius):
            raise ValueError('Timing pilot is absent, distorted, or outside the measured range')
        offset=float(peak-radius)
        a,b,c=correlation[peak-1:peak+2]
        denominator=a-2*b+c
        if denominator:
            offset+=float(.5*(a-c)/denominator)
        results.append({'center_s':center,'offset_s':offset/RATE,'correlation':coefficient})
    return results


def decode(source, destination, directory, name, channel=None):
    command=['ffmpeg','-v','error','-nostdin','-i',source,'-map','0:a:0','-vn']
    command += ['-ac','1'] if channel is None else ['-af',f'pan=mono|c0=c{channel}']
    command += ['-ar',str(RATE),'-c:a','pcm_f32le','-f','f32le',destination]
    run(command,directory,name)
    return np.memmap(destination,dtype='<f4',mode='r')


def read_gspec(directory):
    header=json.loads((directory/'spectrogram.json').read_text(encoding='utf-8'))
    binary=directory/'spectrogram.gspec'
    with binary.open('rb') as stream:
        if stream.read(8)!=b'GSPEC001':
            raise AssertionError('GSPEC magic is not GSPEC001')
    dtype=np.dtype([('time','<f8'),('db','<f4',(BINS,))])
    frames=np.memmap(binary,dtype=dtype,mode='r',offset=8)
    if binary.stat().st_size!=header['binary']['expected_file_bytes']:
        raise AssertionError('GSPEC byte count disagrees with header')
    if len(frames)!=header['frame_count'] or header['frequency_bins']!=BINS:
        raise AssertionError('GSPEC shape disagrees with header')
    return header,frames


def crosscheck_native(executable, source, reference, directory, name, channel=0):
    target=directory/f'{name}-native'
    command=run([executable,'analyze','--input',source,'--out',target,'--channel',str(channel)],directory,f'{name}-native-analyze')
    header,frames=read_gspec(target)
    expected=(len(reference)-FFT)//HOP+1
    assert header['decoded_samples']==len(reference)
    assert len(frames)==expected and expected>0
    assert header['selected_channel_index']==channel and header['channels_averaged'] is False
    assert header['status']=='audio_analyzed_location_not_inferred'
    assert 'latitude_deg' not in header
    assert np.all(np.isfinite(frames['db']))
    centers=(np.arange(expected)*HOP+FFT/2)/RATE
    assert np.array_equal(frames['time'],centers), 'STFT times are not unpadded recording-relative window centers'
    assert centers[-1]+FFT/(2*RATE)<=len(reference)/RATE
    selected=sorted(set([0,1,min(100,expected-1),expected//3,expected//2,expected-1]))
    discrepancies=[]
    for idx in selected:
        data=np.asarray(reference[idx*HOP:idx*HOP+FFT],dtype=float)
        f,p=signal.periodogram(data,fs=RATE,window='hann',detrend=False,scaling='density')
        independent=10*np.log10(np.maximum(p[:BINS],1e-20))
        difference=np.abs(frames['db'][idx]-independent)
        usable=independent>-120
        max_db=float(difference[usable].max()) if usable.any() else float(difference.max())
        assert max_db<.02, f'{name} frame {idx}: PSD error {max_db}dB'
        discrepancies.append({'frame':idx,'center_s':float(frames['time'][idx]),'maximum_psd_error_db_above_minus120db':max_db})
    return {'status':'passed','native_output':str(target),'native_elapsed_s':command['elapsed_s'],'selected_channel':channel,
            'decoded_samples':len(reference),'frame_count':len(frames),'frequency_bins':BINS,
            'first_frame_center_s':float(frames['time'][0]),'last_frame_center_s':float(frames['time'][-1]),
            'gspec_sha256':sha256(target/'spectrogram.gspec'),'checked_frames':discrepancies}


def negative_timing_controls():
    reference=np.random.default_rng(441).normal(0,.1,RATE*10)
    rows=[]
    for shift in [-70,0,83]:
        measured=timing_offsets(reference,np.roll(reference,shift),[2,5,8])
        maximum=max(abs(row['offset_s']-shift/RATE) for row in measured)
        assert maximum<1/RATE
        rows.append({'name':f'known-delay-{shift}-samples','status':'passed','maximum_offset_error_s':maximum})
    unrelated=np.random.default_rng(442).normal(0,.1,RATE*10)
    for name,a,b in [('unrelated',reference,unrelated),('silent-pilot',np.zeros(RATE*10),unrelated),('truncated',reference,reference[:RATE*5])]:
        try:
            timing_offsets(a,b,[2,5,8])
        except ValueError as error:
            rows.append({'name':name,'status':'passed','expected_rejection':str(error)})
        else:
            raise AssertionError(f'Invalid {name} timing control was accepted')
    return rows


def codec_audit(executable, output):
    output.mkdir()
    rate,duration,centers=24000,45.375,[2.3,18.7,39.2]
    t=np.arange(round(rate*duration))/rate
    rng=np.random.default_rng(902613)
    noise=signal.sosfilt(signal.butter(6,[150,1300],fs=rate,btype='bandpass',output='sos'),rng.normal(size=len(t)))
    envelope=sum(np.exp(-((t-center)/.45)**8) for center in centers)
    channel=.08*np.sin(2*np.pi*(170*t-.5*t**2))+noise*envelope
    channel/=max(1.,float(np.max(np.abs(channel))))
    source=output/'timing-source.wav'
    sf.write(source,np.column_stack((channel,.7*channel)),rate,subtype='FLOAT')
    reference=decode(source,output/'reference-mono.f32',output,'reference-decode')
    native_reference=decode(source,output/'reference-channel0.f32',output,'reference-channel0-decode',channel=0)
    baseline=crosscheck_native(executable,source,native_reference,output,'uncompressed-reference')
    variants=[('mono-8k-cbr16',8000,1,['-b:a','16k'],True),('mono-16k-cbr32',16000,1,['-b:a','32k'],True),
              ('stereo-44k-cbr96',44100,2,['-b:a','96k'],True),('stereo-48k-vbr',48000,2,['-q:a','4'],True),
              ('mono-16k-no-xing',16000,1,['-b:a','32k','-write_xing','0'],False)]
    report={'upstream_commit':PIN,'source_sha256':sha256(source),'source_duration_s':duration,'source_rate_hz':rate,
            'pilot_centers_s':centers,'reference_samples':len(reference),'uncompressed_native':baseline,'variants':[]}
    for name,encoded_rate,channels,options,gapless in variants:
        mp3=output/f'{name}.mp3'
        run(['ffmpeg','-v','error','-nostdin','-i',source,'-ar',str(encoded_rate),'-ac',str(channels),'-c:a','libmp3lame',*options,mp3],output,f'{name}-encode')
        decoded=decode(mp3,output/f'{name}.f32',output,f'{name}-decode')
        offsets=timing_offsets(reference,decoded,centers)
        maximum=max(abs(p['offset_s']) for p in offsets)
        delta=(len(decoded)-len(reference))/RATE
        native_pcm=decode(mp3,output/f'{name}-channel0.f32',output,f'{name}-channel0-decode',channel=0)
        native=crosscheck_native(executable,mp3,native_pcm,output,name)
        row={'name':name,'encoded_sample_rate_hz':encoded_rate,'encoded_channels':channels,'mp3_sha256':sha256(mp3),
             'gapless_expected':gapless,'pilots':offsets,'maximum_absolute_offset_s':maximum,'duration_difference_s':delta,
             'end_to_start_drift_s':offsets[-1]['offset_s']-offsets[0]['offset_s'],'native_validation':native,
             'status':'passed' if gapless else 'diagnostic'}
        if gapless:
            assert maximum<=.002 and abs(delta)<=.002, f'{name} fails upstream 2ms timing/duration limits'
        else:
            row['limitation']='Missing Xing gapless metadata: this decoded sample zero cannot establish original recording time. Timing displacement is reported, not called a passed gapless result.'
        report['variants'].append(row)
        write_json(output/'report.json',report)
    # Separate upstream MP3 smoke fixture: frequency and duration of a 440Hz tone.
    tt=np.arange(16000*3)/16000
    wav=output/'smoke440.wav'; mp3=output/'smoke440.mp3'
    sf.write(wav,.2*np.sin(2*np.pi*440*tt),16000)
    run(['ffmpeg','-v','error','-nostdin','-i',wav,'-codec:a','libmp3lame','-b:a','64k',mp3],output,'smoke440-encode')
    data=decode(mp3,output/'smoke440.f32',output,'smoke440-decode',channel=0)
    frequency=float(np.fft.rfftfreq(len(data),1/RATE)[np.argmax(np.abs(np.fft.rfft(data)))])
    assert abs(len(data)/RATE-3)<.025 and abs(frequency-440)<1 and float(np.max(abs(data)))>.1
    report['mp3_smoke']={'status':'passed','peak_hz':frequency,'duration_s':len(data)/RATE,'native_validation':crosscheck_native(executable,mp3,data,output,'smoke440')}
    report['timing_negative_controls']=negative_timing_controls()
    report['passed']=True
    write_json(output/'report.json',report)
    return report


def real_source_audit(executable, source, output):
    output.mkdir()
    source_hash=sha256(source)
    if source_hash!=SOURCE_SHA256:
        raise AssertionError('Real-source audit requires the pinned one-hour reference recording; source hash differs')
    report={'source':str(source),'source_sha256':source_hash,'channels':[],'location_verified':False}
    decoded=[]
    for channel in [0,1]:
        data=decode(source,output/f'channel-{channel}.f32',output,f'channel-{channel}-decode',channel=channel)
        decoded.append(data)
        report['channels'].append(crosscheck_native(executable,source,data,output,f'channel-{channel}',channel=channel))
    assert len(decoded[0])==len(decoded[1])
    difference=np.asarray(decoded[0])-np.asarray(decoded[1])
    report['channel_difference_rms']=float(np.sqrt(np.mean(difference.astype(float)**2)))
    report['decoded_duration_s']=len(decoded[0])/RATE
    assert 3600<=report['decoded_duration_s']<=3600.3
    # Content check uses prior observed traces, never predicted flight curves.
    measured=[]
    for event in [1,2]:
        path=ROOT/'cases'/f'colsto_event{event}.json'
        trace=json.loads(path.read_text(encoding='utf-8'))['points']
        contrasts=[]
        for point in trace:
            center=round(point['t_rec_s']*RATE)
            segment=np.asarray(decoded[0][center-4096:center+4096],dtype=float)
            assert len(segment)==8192
            f,p=signal.periodogram(segment,fs=RATE,window='hann',detrend=False,scaling='density')
            db=10*np.log10(np.maximum(p,1e-20))
            peak=int(np.argmin(abs(f-point['frequency_hz'])))
            flank=(abs(f-f[peak])>=5)&(abs(f-f[peak])<=12)
            contrasts.append(float(db[peak]-np.median(db[flank])))
        median=float(np.median(contrasts)); fraction=float(np.mean(np.asarray(contrasts)>=6))
        assert median>=8 and fraction>=.8, f'Expected observed event {event} is absent or badly degraded'
        measured.append({'event':event,'trace_sha256':sha256(path),'points':len(trace),'median_peak_to_flank_db':median,'fraction_above6db':fraction,'status':'passed',
                         'scope':'Presence of a previously measured ridge in this pinned source; not automatic event discovery.'})
    report['recorded_event_content_checks']=measured
    crop_start=2130.
    crop=output/'known-offset-crop.wav'
    crop_data=np.asarray(decoded[0][round(crop_start*RATE):round((crop_start+4.375)*RATE)])
    sf.write(crop,crop_data,RATE,subtype='FLOAT')
    crop_check=crosscheck_native(executable,crop,crop_data,output,'known-offset-crop')
    report['crop_clock_check']={'native':crop_check,'crop_start_downloaded_s':crop_start,'original_recording_offset_s':3600.,
                              'first_window_crop_s':.512,'first_window_downloaded_s':crop_start+.512,'first_window_original_recording_s':3600+crop_start+.512,
                              'status':'passed','scope':'Native output retains crop-relative window centers; the known crop and original-file offsets are explicit external bookkeeping, not a recovered UTC clock.'}
    report['passed']=True
    write_json(output/'report.json',report)
    return report


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe',type=Path,required=True)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--source',type=Path,required=True)
    args=parser.parse_args()
    output=args.output.resolve(); output.mkdir(parents=True,exist_ok=False)
    executable=args.exe.resolve(); source=args.source.resolve()
    report={'schema_version':1,'status':'running','scope':'native audio integration, codec timing, and pinned source signal validation',
            'location_validation':False,'upstream_commit':PIN,'executable':str(executable),'executable_sha256':sha256(executable),
            'provenance':{'python':sys.version,'platform':platform.platform(),'dependencies':{p:importlib.metadata.version(p) for p in ['numpy','scipy','soundfile']},
                          'ffmpeg':subprocess.check_output(['ffmpeg','-version'],text=True).splitlines()[0],
                          'source_hashes':{str(p.relative_to(ROOT)):sha256(p) for p in [Path(__file__),ROOT/'src'/'audio.rs',ROOT/'src'/'media.rs',ROOT/'tests'/'audio_validation.rs']}},
            'coverage_gaps':['This spectral/codec audit does not evaluate automatic discovery. The separate benchmark_discovery.py suite tests native event discovery, harmonic grouping, fragment stitching and controlled negative signals. Manual source traces alone are not discovery evidence.']}
    write_json(output/'report.json',report)
    started=time.monotonic()
    try:
        report['codec']=codec_audit(executable,output/'codec')
        write_json(output/'report.json',report)
        report['real_source']=real_source_audit(executable,source,output/'real-source')
        report.update(status='passed',passed=True)
    except Exception as error:
        report.update(status='failed',passed=False,error=f'{type(error).__name__}: {error}')
    report['elapsed_s']=time.monotonic()-started
    write_json(output/'report.json',report)
    print(json.dumps({'report':str(output/'report.json'),'status':report['status'],'error':report.get('error'),'elapsed_s':report['elapsed_s']}))
    return 0 if report['status']=='passed' else 1


if __name__=='__main__':
    raise SystemExit(main())
