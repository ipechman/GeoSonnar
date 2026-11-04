"""Compare audio measurements on all 40 public paired research passages.

The author's annotated harmonic selects the frequency band, as a human would do
from a spectrogram. ADS-B reference speed/range labels are used only afterward.
This measures acoustic feature recovery, not recording-location accuracy: this
dataset does not publish an explicit recorder coordinate or audio-sample UTC clock.
"""
import json
import time
from pathlib import Path

import numpy as np
import pandas as pd
import soundfile as sf

from dopplergeist.audio import measure,fit_flyover
from dopplergeist.tracks import unix_time

root=Path(__file__).resolve().parents[1]
source=root/"data/research/zenodo-23424278"
output=root/"results/real-feature-benchmark"
output.mkdir(parents=True,exist_ok=True)
references=pd.read_csv(source/"final_results_40_cases.csv")
results=[]
for reference in references.itertuples():
    began=time.monotonic()
    folder=next((source/"extracted").glob("*/"+reference.case))
    annotations=pd.read_csv(folder/"frequency_vs_time_used.csv")
    manual_t=annotations.time_utc.map(unix_time).to_numpy()
    manual_t-=manual_t[0]
    manual_f=annotations.frequency_hz.to_numpy()
    row={"case":reference.case,"category":reference.category,
         "reference_speed_mps":reference.V_ADSB_at_closest_kmh/3.6,
         "reference_slant_m":reference.R_ADSB_m}
    try:
        # The window is the complete published WAV; selecting a harmonic is the
        # sole annotation-assisted step in automatic ridge extraction.
        spread=np.ptp(manual_f)
        band=[max(10,np.min(manual_f)-max(5,spread*0.08)),np.max(manual_f)+max(5,spread*0.08)]
        spec,ridge,weight,fit=measure(folder/"audio.wav",{
            "start_s":0,"end_s":sf.info(folder/"audio.wav").duration,
            "band_hz":band,"max_slope_hz_s":max(8,spread/12),"max_up_slope_hz_s":1},output/reference.case)
        manual_fit=fit_flyover(manual_t,manual_f)
        row.update({"detected":fit["detected"],"audio_fit":fit,"annotation_fit":manual_fit,
                    "speed_error_percent":100*(fit["speed_mps"]/row["reference_speed_mps"]-1),
                    "slant_error_percent":100*(fit["slant_m"]/row["reference_slant_m"]-1),
                    "annotation_speed_error_percent":100*(manual_fit["speed_mps"]/row["reference_speed_mps"]-1),
                    "elapsed_s":time.monotonic()-began})
    except Exception as exc:
        row["error"]=f"{type(exc).__name__}: {exc}"
    results.append(row)
    (output/"cases.json").write_text(json.dumps(results,indent=2,allow_nan=False)+"\n")
    print(reference.case,{k:row.get(k) for k in ["detected","speed_error_percent","slant_error_percent","error"]},flush=True)
detected=[r for r in results if r.get("detected")]
summary={"cases":len(results),"detected":len(detected),"errors":sum("error" in r for r in results),
         "median_absolute_speed_error_percent":float(np.median([abs(r["speed_error_percent"]) for r in detected])) if detected else None,
         "median_absolute_slant_error_percent":float(np.median([abs(r["slant_error_percent"]) for r in detected])) if detected else None,
         "validation_scope":"harmonic-band-assisted acoustic parameter recovery; not a location benchmark"}
(output/"summary.json").write_text(json.dumps(summary,indent=2)+"\n")
print(summary,flush=True)
