import numpy as np
import soundfile as sf

from dopplergeist.audio import measure
from dopplergeist.tracks import adsblol_frame


def test_noise_does_not_force_a_detection(tmp_path):
    p=tmp_path/"noise.wav"
    sf.write(p,np.random.default_rng(791).normal(0,0.1,4000*45),4000)
    *_,fit=measure(p,{"start_s":0,"end_s":45,"band_hz":[80,180]})
    assert not fit["detected"]


def test_geometric_altitude_slot_and_labelled_fallback():
    trace={"icao":"abc123","timestamp":1000,"t":"C172","trace":[
        [0,40,-100,3000,100,90,0,0,None,"adsb_icao",3100,0],
        [1,40,-100,3000,100,90,0,0,None,"adsb_icao",None,0]]}
    frame=adsblol_frame(trace)
    assert np.allclose(frame.alt_m,3100*0.3048)
    assert list(frame.altitude_basis)==["ellipsoidal","ellipsoidal_interpolated"]
    trace["trace"]=[trace["trace"][1]]
    frame=adsblol_frame(trace)
    assert frame.altitude_basis.iloc[0]=="barometric"
