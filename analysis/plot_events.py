from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path('output/audio-validation')
s=np.load(root/'channel_0_spectrogram.npz'); f,t,db=s['f_hz'],s['t_s'],s['p_db']
fig,axes=plt.subplots(2,1,figsize=(16,9),layout='constrained')
for ax,(lo,hi,tau,event) in zip(axes,[(2090,2280,2152,'Event 1'),(3030,3135,3073,'Event 2')]):
    fm=(f>=70)&(f<=220); tm=(t>=lo)&(t<=hi); p=db[fm][:,tm]
    # Frequency-wise median subtraction suppresses fixed lines for viewing only.
    whiten=p-np.median(db[fm],axis=1,keepdims=True)
    im=ax.imshow(whiten,origin='lower',aspect='auto',extent=[t[tm][0],t[tm][-1],f[fm][0],f[fm][-1]],cmap='magma',vmin=-5,vmax=24)
    ax.axvline(tau,color='#51efff',ls='--',lw=1,label=f'Guide anchor minus 3600 s = {tau} s (alignment hypothesis)')
    ax.set(xlabel='Time in downloaded raw recording (s)',ylabel='Frequency (Hz)',title=event+' — measured audio, channel 1')
    ax.legend(loc='upper right'); ax.set_xticks(np.arange(lo,hi+1,10))
    fig.colorbar(im,ax=ax,label='dB above each frequency\'s full-file median')
fig.suptitle('Two observed descending tonal bands are consistent with a one-hour crop offset',fontsize=16)
fig.savefig(root/'event_pair_inspection.png',dpi=140)
plt.close(fig)
