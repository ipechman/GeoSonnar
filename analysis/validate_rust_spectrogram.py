"""Independently compare Rust GSPEC numeric output to SciPy periodograms.

Uses the separately decoded stereo reference from investigate_audio.py. It checks
full-file dimensions, selected frame timestamps, and unrendered numeric PSD bins.
"""
from pathlib import Path
import json, struct
import numpy as np
from scipy import signal
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'output'/'audio-validation'
HEADER=ROOT/'output'/'rust-audio'/'spectrogram.json'
h=json.loads(HEADER.read_text(encoding='utf-8'))
audio=np.load(OUT/'audio_4000hz_stereo.npy',mmap_mode='r')
binary=HEADER.parent/h['binary']['filename']
expected_frames=(len(audio)-h['fft_size'])//h['hop_samples']+1
assert h['frame_count']==expected_frames,(h['frame_count'],expected_frames)
assert binary.stat().st_size==h['binary']['expected_file_bytes']
indices=sorted(set([0,1,100,1000,5500,9000,h['frame_count']-1]))
results=[]
with binary.open('rb') as src:
    assert src.read(8)==b'GSPEC001'
    for idx in indices:
        src.seek(8+idx*h['binary']['record_bytes'])
        rust_time=struct.unpack('<d',src.read(8))[0]
        rust_db=np.frombuffer(src.read(4*h['frequency_bins']),dtype='<f4')
        start=idx*h['hop_samples']
        samples=audio[start:start+h['fft_size'],h['selected_channel_index']].astype(np.float64)
        f,p=signal.periodogram(samples,fs=h['sample_rate_hz'],window='hann',detrend=False,scaling='density')
        scipy_db=10*np.log10(np.maximum(p[:h['frequency_bins']],1e-20))
        time=(start+h['fft_size']/2)/h['sample_rate_hz']
        error=np.abs(rust_db-scipy_db)
        usable=(scipy_db>-120)
        res={'frame':idx,'timestamp_s':rust_time,'timestamp_error_s':abs(rust_time-time),'max_abs_db_error_all_bins':float(error.max()),'max_abs_db_error_above_minus120db':float(error[usable].max()),'rms_db_error':float(np.sqrt(np.mean(error*error)))}
        assert res['timestamp_error_s']<1e-12
        assert res['max_abs_db_error_above_minus120db']<0.02,res
        results.append(res)
report={'status':'passed','independent_reference':'SciPy signal.periodogram with periodic Hann, 4096 samples, no detrend, one-sided PSD density; independent full-file stereo decode retains selected channel','sample_count':len(audio),'frame_count':h['frame_count'],'frequency_bins':h['frequency_bins'],'binary_length_bytes':binary.stat().st_size,'checked_frames':results,'tolerance_db_above_minus120db':0.02,'scope':'Validates Rust FFT normalization, frequency ordering, serialized timestamps and dimensions; does not establish geographic location.'}
(OUT/'rust_spectrogram_crosscheck.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,indent=2))
