"""Create manually initialized observed ridges and fit a coarse acoustic model.

The seed points were selected by inspecting event_pair_inspection.png, before
using flight paths or predicted curves. Local maximum refinement remains within
3 Hz of each seed interpolation. These ridges are semi-manual, not blind event
detection, and the ideal-pass fits are not geolocation or historical-flight fits.
"""
from pathlib import Path
import json
import numpy as np
from scipy.optimize import least_squares
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'output'/'audio-validation'
s=np.load(OUT/'channel_0_spectrogram.npz'); f,t,db=s['f_hz'],s['t_s'],s['p_db']
seed_sets=[
 np.array([[2090,178],[2100,173],[2110,164],[2120,155],[2130,144],[2140,130],[2150,116],[2160,106],[2170,99],[2180,95],[2190,91],[2200,88],[2220,85],[2240,82],[2260,80],[2278,79]],float),
 np.array([[3030,178],[3040,174],[3050,166],[3060,155],[3070,139],[3080,127],[3090,115],[3100,108],[3110,105],[3120,102],[3134,101]],float)
]
C=343.
def ideal_heard(freq0,speed,distance,tau,times):
    # tau is arrival time of sound emitted at geometric closest approach.
    w=times-tau+distance/C
    u=(C*C*w-np.sqrt(C*C*speed*speed*w*w+(C*C-speed*speed)*distance*distance))/(C*C-speed*speed)
    r=np.sqrt(distance*distance+(speed*u)**2)
    vr=speed*speed*u/r
    return freq0*C/(C+vr)
results=[]
fig,axes=plt.subplots(2,1,figsize=(15,8),layout='constrained')
for idx,(seed,ax) in enumerate(zip(seed_sets,axes),1):
    times=np.arange(seed[0,0],seed[-1,0]+.1,1.)
    predicted=np.interp(times,seed[:,0],seed[:,1])
    ridges=[]; strengths=[]
    for ti,fi in zip(times,predicted):
        j=int(np.argmin(abs(t-ti))); k=np.where(abs(f-fi)<=3)[0]
        vals=db[k,j]; peak=k[np.argmax(vals)]
        ridges.append(f[peak])
        outside=(abs(f-f[peak])>=5)&(abs(f-f[peak])<=12)
        strengths.append(float(db[peak,j]-np.median(db[outside,j])))
    ridges=np.array(ridges); strengths=np.array(strengths)
    # Fixed visible end limits only. Entire selected interval is scored for all fits.
    p0=[115,140,5500,2152] if idx==1 else [132,100,3000,3073]
    lower=[70,30,500,p0[3]-20]; upper=[200,200,15000,p0[3]+20]
    fit=least_squares(lambda p:ideal_heard(*p,times)-ridges,p0,bounds=(lower,upper),loss='soft_l1',f_scale=1.5,x_scale='jac')
    model=ideal_heard(*fit.x,times)
    residual=ridges-model
    data=np.column_stack((times,ridges,np.full(len(times),1.0),strengths))
    np.savetxt(OUT/f'event_{idx}_ridge.csv',data,delimiter=',',header='recording_time_s,frequency_hz,frequency_uncertainty_hz,local_peak_contrast_db',comments='',fmt='%.6f')
    np.savetxt(OUT/f'event_{idx}_manual_seeds.csv',seed,delimiter=',',header='recording_time_s,frequency_hz',comments='',fmt='%.3f')
    res={'event_id':idx,'manual_seed_file':f'event_{idx}_manual_seeds.csv','ridge_file':f'event_{idx}_ridge.csv','window_s':[float(times[0]),float(times[-1])],'ridge_method':'Manual approximate ridge seeds from measured spectrogram; refine at 1-second samples by strongest bin within +/-3 Hz. No flight prediction used.','nominal_frequency_uncertainty_hz':1.0,'uncertainty_note':'Nominal measurement scale only, not a calibrated confidence interval; compression and overlaps may cause larger errors.','guide_original_anchor_s':5752 if idx==1 else 6673,'guide_anchor_minus_3600_s':2152 if idx==1 else 3073,'ideal_pass_fit':{'frequency_hz':float(fit.x[0]),'speed_mps':float(fit.x[1]),'slant_cpa_m':float(fit.x[2]),'heard_cpa_recording_s':float(fit.x[3]),'speed_of_sound_mps':C,'rmse_hz':float(np.sqrt(np.mean(residual**2))),'max_abs_error_hz':float(np.max(abs(residual))),'limitations':'Ideal straight constant-speed stable-tone approximation; not real flight geometry. Parameters are correlated; these are descriptive fits, not validated physical measurements.'},'weak_samples_below_6db_contrast':int(sum(strengths<6)),'total_samples':len(times)}
    results.append(res)
    fm=(f>=70)&(f<=220); tm=(t>=times[0])&(t<=times[-1]); p=db[fm][:,tm]
    whiten=p-np.median(db[fm],axis=1,keepdims=True)
    ax.imshow(whiten,origin='lower',aspect='auto',extent=[t[tm][0],t[tm][-1],f[fm][0],f[fm][-1]],cmap='magma',vmin=-5,vmax=24)
    ax.plot(times,ridges,'.',color='cyan',ms=2,label='Observed ridge (manual seed + local peak)')
    ax.plot(times,model,color='white',lw=1.4,label=f'Ideal-pass fit (RMSE {res["ideal_pass_fit"]["rmse_hz"]:.2f} Hz)')
    ax.set(xlabel='Time in downloaded raw recording (s)',ylabel='Frequency (Hz)',title=f'Event {idx}: observed ridge and coarse model')
    ax.legend(loc='upper right')
fig.suptitle('Audio evidence only — no historical flight trajectories or inferred location',fontsize=16)
fig.savefig(OUT/'event_ridges_and_coarse_fits.png',dpi=140)
plt.close(fig)
report={'status':'audio_events_confirmed_geolocation_not_performed','source_time_alignment':{'hypothesis_offset_s':3600,'relation':'original_recording_time = downloaded_file_time + 3600','basis':'Both event spectrograms visually match the explanatory video with one shared 3600-second offset; source filename shown in explanation at 03:50 contains 60to120min.','precision_note':'This establishes minute/hour indexing, not sample-accurate alignment or UTC recording date.'},'events':results,'next_required_inputs':['historical flight tracks on confirmed recording date','compatible flight and observer height references','bounded search region/time assumptions for reproducible ranking']}
(OUT/'event_measurements.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report,indent=2))
