"""Decision/refinement regression controls, not end-to-end acceptance evidence."""
from copy import deepcopy
from types import SimpleNamespace

import numpy as np
import pytest
from pyproj import Geod, Transformer

automatic = pytest.importorskip("dopplergeist.automatic")
matching = pytest.importorskip("dopplergeist.matching")
decide_location = pytest.importorskip("dopplergeist.confidence").decide_location
from dopplergeist.geometry import Track, ecef, straight_doppler


def _decision_evidence():
    events = [{"event_id": f"e{i}", "harmonic_group_id": f"g{i}",
               "start_s": i * 100, "end_s": i * 100 + 20, "band_hz": [90, 200],
               "ridge": [{"time_s": i * 100 + t, "frequency_hz": 140 - t}
                         for t in (0, 10, 20)]} for i in range(2)]
    candidate = {"lat": 40., "lon": -100., "score": -1.,
                 "score_kind": "negative_robust_ridge_loss_hz",
                 "event_ids": ["e0", "e1"], "flight_ids": ["a", "b"],
                 "recording_start_utc": "1970-01-01T00:00:00Z",
                 "uncertainty": {"radius_m": 70., "kind": "sensitivity", "method": "unit_control"}}
    coverage = {"search_complete": True, "data_status": "available", "limitations": []}
    diagnostics = {"clock_locked": True, "alternatives_evaluated": True,
                   "refinement_boundary_limited": False,
                   "sensitivity": {"complete": True, "boundary_limited": False, "radius_m": 70.,
                                   "sources": ["frequency", "clock", "altitude", "sound_speed"]}}
    for event in events:
        diagnostics[event["event_id"]] = {"rmse_hz": 0.7, "outlier_fraction": 0.1,
                                         "support_fraction": 1., "drift_at_bound": False}
    return events, candidate, coverage, diagnostics


@pytest.mark.parametrize("separation_m", [90., 150.])
def test_near_tied_mode_outside_returned_radius_cannot_be_supported(separation_m):
    events, best, coverage, diagnostics = _decision_evidence()
    assert decide_location(events, [best], coverage, matching_diagnostics=diagnostics)["status"] == "succeeded"
    alternative = deepcopy(best)
    alternative["lon"], alternative["lat"], _ = Geod(ellps="WGS84").fwd(
        best["lon"], best["lat"], 90., separation_m)
    alternative["score"] = -1.2  # A near tie under the declared per-event loss rule.
    result = decide_location(events, [best, alternative], coverage, matching_diagnostics=diagnostics)
    # 90 m escapes the returned 70 m radius; 150 m also exceeds the 100 m target.
    # The hook does not return a new center/radius, so it must abstain here.
    assert result["status"] == "inconclusive", (
        f"accepted a {best['uncertainty']['radius_m']} m envelope while retaining "
        f"a near-tied location {separation_m} m from its center: {result}"
    )


def _numerical_case():
    """Two nonparallel Cartesian flights with analytic Doppler observations."""
    observer = ecef(40., -100., 100.)
    inverse = Transformer.from_crs("EPSG:4978", "EPSG:4979", always_xy=True)
    tracks, events, prepared, measurements = {}, [], [], []
    for i, (center, f0, velocity, cross) in enumerate([
        (70., 120., [80., 0., 0.], [0., 1600., 800.]),
        (250., 160., [0., 80., 0.], [1700., 0., 1000.]),
    ]):
        identity = "ab"[i]
        times = np.arange(center - 120, center + 121, 2.)
        xyz = observer + (times - center)[:, None] * velocity + cross
        lon, lat, altitude = inverse.transform(*xyz.T)
        tracks[identity] = Track(identity, times, lat, lon, altitude,
                                 velocity=np.tile(velocity, (len(times), 1)))
        receive = np.arange(center - 30, center + 30, 2.)
        slant = np.linalg.norm(cross)
        ridge = straight_doppler(receive, f0, 80., slant, center + slant / 343.)
        event = {"event_id": f"e{i}", "harmonic_group_id": f"g{i}",
                 "start_s": float(receive[0]), "end_s": float(receive[-1]), "band_hz": [50., 230.],
                 "ridge": [{"time_s": float(t), "frequency_hz": float(f)}
                           for t, f in zip(receive, ridge)]}
        events.append(event)
        prepared.append(dict(event, event_ids=[event["event_id"]],
                             f0_range_hz=[f0 - 1, f0 + 1], quality={"source_drift_fraction": 0.}))
        measurements.append((matching.RidgeTimes(receive), ridge, np.ones(len(receive)), {}))
    terrain = SimpleNamespace(sample=lambda lat, lon: np.array(100.))
    seed = {"lat": 40.0002, "lon": -99.9998, "elev_m": 100.,
            "recording_start_time_s": 0., "flight_ids": ["a", "b"],
            "pass_emission_times_s": [70., 250.], "point_id": 0}
    return tracks, events, prepared, measurements, terrain, seed


def test_optimizer_exhaustion_cannot_establish_complete_sensitivity(monkeypatch):
    tracks, _, prepared, measurements, terrain, seed = _numerical_case()
    original_minimize, convergence = matching.minimize, []

    def exhausted_minimize(objective, initial, **kwargs):
        kwargs["options"] = dict(kwargs.get("options", {}), maxiter=0)
        result = original_minimize(objective, initial, **kwargs)
        convergence.append(bool(result.success))
        return result

    monkeypatch.setattr(matching, "minimize", exhausted_minimize)
    try:
        result = matching.refine_candidate(seed, tracks, prepared, measurements, terrain,
                                           recording_start_window_s=(0., 0.))
    except ValueError:
        # Explicit failure is acceptable after a real solver exhausts its budget.
        assert convergence and not all(convergence)
        return
    assert convergence and not all(convergence), "control must exercise actual nonconvergence"
    assert result["sensitivity"]["complete"] is not True, (
        "finite fallback profiles are not completed optimization evidence"
    )


def test_unrefined_distant_same_association_mode_is_not_fully_assessed(monkeypatch, tmp_path):
    tracks, events, prepared, measurements, terrain, seed = _numerical_case()
    raw = [dict(seed), dict(seed, lat=40.015, point_id=1)]
    for row in raw:
        row.update(matching.score_ridges(list(tracks.values()), prepared, measurements,
                                        ecef(row["lat"], row["lon"], 100.), 0.))
    raw.sort(key=lambda row: row["score"], reverse=True)
    artifacts, refined_modes = {}, set()
    original_refine = matching.refine_candidate

    def recorded_refine(candidate, *args, **kwargs):
        refined_modes.add((candidate["lat"], candidate["lon"]))
        return original_refine(candidate, *args, **kwargs)

    monkeypatch.setattr(matching, "refine_candidate", recorded_refine)
    monkeypatch.setattr(automatic, "write_json", lambda path, value: artifacts.__setitem__(path.name, deepcopy(value)))
    search = {"ranked": raw, "status": "complete", "scoring_time_is_locked": True}
    state = {"recording": {"time_window": {"start_utc": "1970-01-01T00:00:00Z",
                                          "end_utc": "1970-01-01T00:00:00Z"}},
             "coverage": {"search_complete": True, "data_status": "available", "limitations": []}}
    index = SimpleNamespace(query=lambda bounds, interval: list(tracks.values()))
    finish = lambda status, reason, message, **fields: {"status": status, "reason_code": reason, **fields}
    automatic._decide(search, events, prepared, measurements, [index], terrain,
                      state, tmp_path, {"refine_finalists": 5}, finish, lambda: None)
    unrefined = {(row["lat"], row["lon"]) for row in raw} - refined_modes
    diagnostics = artifacts["refined-candidates.json"]["matching_diagnostics"]
    assert not unrefined or diagnostics["alternatives_evaluated"] is not True, (
        f"retained spatial modes were never refined but alternatives were certified: {unrefined}"
    )

