import numpy as np
import pytest
import soundfile as sf

from dopplergeist.contracts import validate_event
from dopplergeist.discovery import DiscoverySettings, _stitch_fragments, discover_events
from dopplergeist.intake import intake_recording
from dopplergeist.matching import prepare_events


def _groups(events):
    return list(dict.fromkeys(event["harmonic_group_id"] for event in events))


def _audio(tmp_path, events, duration=180, mute=None):
    sr = 4000
    t = np.arange(sr * duration) / sr
    data = np.random.default_rng(9133).normal(0, 0.08, len(t))
    data += 0.12 * np.sin(2 * np.pi * 400 * t)
    for center, f0, harmonics in events:
        relative = t - center
        frequency = f0 * (1 - 0.18 * relative / np.sqrt(relative ** 2 + 8 ** 2))
        phase = 2 * np.pi * np.cumsum(frequency) / sr
        amplitude = 0.16 * np.exp(-(relative / 24) ** 2) * (np.abs(relative) < 44)
        if mute is not None:
            amplitude *= (t < mute[0]) | (t > mute[1])
        for harmonic in harmonics:
            data += amplitude / harmonic * np.sin(harmonic * phase)
    path = tmp_path / "recording.wav"
    sf.write(path, data, sr, subtype="FLOAT")
    return intake_recording(path, tmp_path / "intake")


def test_blind_whole_file_seams_harmonics_and_separate_events(tmp_path):
    recording = _audio(tmp_path, [(65, 120, [1, 2, 3]), (135, 250, [1, 2])])
    events = discover_events(recording, output_dir=tmp_path / "discovery")
    groups = _groups(events)
    assert len(groups) == 2, [(e["start_s"], e["end_s"], e["band_hz"]) for e in events]
    for center, group in zip((65, 135), groups):
        event = next(e for e in events if e["harmonic_group_id"] == group)
        assert event["start_s"] < center < event["end_s"]
        assert event["quality"]["harmonic_count"] >= 2
    for event in events:
        assert validate_event(event) == event
        assert np.all(np.diff([point["time_s"] for point in event["ridge"]]) > 0)
    assert (tmp_path / "discovery" / "discovery.png").is_file()


def test_noise_stationary_tones_and_upward_chirp_are_rejected(tmp_path):
    sr = 4000
    t = np.arange(sr * 120) / sr
    data = np.random.default_rng(791).normal(0, 0.1, len(t))
    data += 0.3 * np.sin(2 * np.pi * 180 * t)
    data += 0.1 * signal_chirp(t)
    path = tmp_path / "negative.wav"
    sf.write(path, data, sr)
    recording = intake_recording(path, tmp_path / "intake")
    assert discover_events(recording) == []


def signal_chirp(t):
    return np.sin(2 * np.pi * (500 * t + 0.25 * t ** 2))


def test_overlapping_nonharmonic_events_are_retained(tmp_path):
    recording = _audio(tmp_path, [(60, 120, [1, 2]), (76, 210, [1])], duration=140)
    events = discover_events(recording)
    groups = _groups(events)
    assert len(groups) == 2, [(e["start_s"], e["end_s"], e["band_hz"]) for e in events]
    first, second = (next(e for e in events if e["harmonic_group_id"] == group) for group in groups)
    assert first["end_s"] > second["start_s"]


def test_short_input_has_no_forced_event(tmp_path):
    recording = _audio(tmp_path, [], duration=1)
    assert discover_events(recording) == []


def test_missing_fundamental_harmonics_are_one_event(tmp_path):
    recording = _audio(tmp_path, [(65, 120, [2, 3])], duration=140)
    events = discover_events(recording)
    assert len(_groups(events)) == 1
    assert events[0]["quality"]["harmonic_count"] == 2
    assert len(events) == 2
    prepared, measurements = prepare_events(events)
    assert len(prepared) == 1
    assert len(measurements[0][1]) == sum(len(event["ridge"]) for event in events)


def test_switching_stationary_tones_do_not_make_a_flyover(tmp_path):
    sr = 4000
    t = np.arange(sr * 120) / sr
    data = np.random.default_rng(811).normal(0, 0.04, len(t))
    data += 0.2 * (1 / (1 + np.exp((t - 60) / 2))) * np.sin(2 * np.pi * 74 * t)
    data += 0.2 * (1 / (1 + np.exp(-(t - 60) / 2))) * np.sin(2 * np.pi * 65 * t)
    path = tmp_path / "switching.wav"
    sf.write(path, data, sr)
    assert discover_events(intake_recording(path, tmp_path / "intake")) == []


def test_interrupted_tone_is_one_event_without_invented_gap_observations(tmp_path):
    recording = _audio(tmp_path, [(65, 120, [1, 2])], duration=140, mute=(58, 69))
    events = discover_events(recording)
    assert len(_groups(events)) == 1
    event = events[0]
    assert event["quality"]["fragment_count"] >= 2
    assert event["quality"]["max_observed_gap_s"] >= 7
    assert not any(60 < point["time_s"] < 67 for point in event["ridge"])
    assert event["quality"]["frequency_sigma_hz"] == event["quality"]["frequency_uncertainty_hz"]


def test_ambiguous_fragment_continuations_remain_separate():
    def path(start, end, offset=0):
        t = np.arange(start, end, 0.5)
        return np.column_stack((t, 180 - 0.7 * t + offset, np.full(len(t), 5)))
    first, second, competitor = path(0, 24), path(30, 55), path(31, 56, 0.5)
    assert len(_stitch_fragments([first, second, competitor], DiscoverySettings())) == 3
    assert len(_stitch_fragments([first, second], DiscoverySettings())) == 1
    assert len(_stitch_fragments([first, path(50, 80)], DiscoverySettings())) == 2


@pytest.mark.parametrize("count", [4, 6])
def test_all_independent_flyovers_are_retained(tmp_path, count):
    centers = [65 + 100 * i for i in range(count)]
    frequencies = [120, 170, 230, 140, 260, 190]
    recording = _audio(tmp_path, [(center, frequency, [1, 2])
                                 for center, frequency in zip(centers, frequencies)],
                       duration=100 * count + 30)
    events = discover_events(recording)
    groups = _groups(events)
    assert len(groups) == count
    for center, group in zip(centers, groups):
        event = next(e for e in events if e["harmonic_group_id"] == group)
        assert event["start_s"] < center < event["end_s"]
        assert event["quality"]["harmonic_count"] >= 2
