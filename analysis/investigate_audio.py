"""Inspect the supplied raw video without inferring location or UTC time.

Run from project root with Python, NumPy, SciPy, Matplotlib, and FFmpeg installed.
The resampled copy keeps channels separate. All plotted times are local to the
one-hour downloaded file, not the long recording shown in the explanation.
"""
from pathlib import Path
import hashlib, json, subprocess
import numpy as np
from scipy import signal
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT/'output'/'audio-validation'
OUT.mkdir(parents=True, exist_ok=True)
SOURCE = next((ROOT/'Known Videos with coordinates').glob('*.mp4'))
RATE = 4000
CACHE = OUT/'audio_4000hz_stereo.npy'
if CACHE.exists():
    audio = np.load(CACHE, mmap_mode='r')
else:
    cmd = ['ffmpeg','-v','error','-i',str(SOURCE),'-map','0:a:0','-vn','-ar',str(RATE),'-c:a','pcm_f32le','-f','f32le','-']
    pcm = subprocess.run(cmd,check=True,stdout=subprocess.PIPE).stdout
    audio = np.frombuffer(pcm,dtype='<f4').reshape(-1,2)
    np.save(CACHE,audio)

metadata = {
  'source': str(SOURCE.relative_to(ROOT)),
  'sha256': hashlib.file_digest(SOURCE.open('rb'),'sha256').hexdigest(),
  'decoded_duration_s': len(audio)/RATE,
  'analysis_sample_rate_hz': RATE,
  'channels': 2,
  'channel_rms': [float(np.sqrt(np.mean(np.square(audio[:,i],dtype=np.float64)))) for i in range(2)],
  'channel_difference_rms': float(np.sqrt(np.mean(np.square(audio[:,0]-audio[:,1],dtype=np.float64)))),
  'channel_correlation': float(np.corrcoef(audio[::20,0],audio[::20,1])[0,1]),
  'processing': 'FFmpeg anti-aliased downsampling to 4000 Hz; channels never averaged; Hann STFT nperseg 8192, hop 1000, no padding.',
  'time_reference': 'Seconds from start of downloaded raw file. Original recording offset is not established.',
}
(OUT/'media_measurements.json').write_text(json.dumps(metadata,indent=2),encoding='utf-8')
print(json.dumps(metadata,indent=2))
for ch in range(2):
    specpath = OUT/f'channel_{ch}_spectrogram.npz'
    if specpath.exists():
        s=np.load(specpath)
        f,t,db=s['f_hz'],s['t_s'],s['p_db']
    else:
        f,t,p=signal.spectrogram(audio[:,ch],fs=RATE,window='hann',nperseg=8192,noverlap=7192,detrend=False,scaling='density',mode='psd')
        keep=(f>=5)&(f<=500)
        f,p=f[keep],p[keep]
        db=10*np.log10(np.maximum(p,1e-20))
        np.savez_compressed(specpath,f_hz=f,t_s=t,p_db=db)
    fig,axes=plt.subplots(6,1,figsize=(18,16),layout='constrained')
    vmin,vmax=np.percentile(db,[12,99.4])
    for idx,ax in enumerate(axes):
        lo,hi=idx*600,(idx+1)*600
        m=(t>=lo)&(t<hi)
        im=ax.imshow(db[:,m],origin='lower',aspect='auto',extent=[t[m][0],t[m][-1],f[0],f[-1]],cmap='magma',vmin=vmin,vmax=vmax)
        ax.set(ylim=(20,400),ylabel='Frequency (Hz)',xlabel='Downloaded recording time (seconds)',title=f'{lo//60:02d}:00 - {hi//60:02d}:00')
        ax.set_xticks(np.arange(lo,hi+1,60))
    fig.suptitle(f'Raw recording, channel {ch+1}: low-frequency spectrogram (no UTC/crop offset applied)',fontsize=18)
    fig.savefig(OUT/f'channel_{ch}_overview.png',dpi=110)
    plt.close(fig)
