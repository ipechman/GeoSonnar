//! Engineering evidence checks adapted from DOPPLERGEIST's pinned confidence
//! contract. These checks are uncalibrated and never read evaluation labels.
//! See THIRD_PARTY_NOTICES.md and benchmarks/upstream for the MIT source.
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

type Check<T> = Result<T, String>;
fn number(v: &Value) -> Check<f64> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| "Expected a finite number".into())
}
fn text(v: &Value) -> Check<&str> {
    v.as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Expected nonempty text".into())
}
fn array(v: &Value) -> Check<&Vec<Value>> {
    v.as_array().ok_or_else(|| "Expected an array".into())
}
fn texts(v: &Value) -> Check<Vec<&str>> {
    array(v)?.iter().map(text).collect()
}
fn set(v: &Value) -> Check<BTreeSet<&str>> {
    Ok(texts(v)?.into_iter().collect())
}
fn record(v: &Value, required: &[&str], optional: &[&str]) -> Check<()> {
    let object = v.as_object().ok_or("Expected an object")?;
    if required.iter().any(|k| !object.contains_key(*k))
        || object
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err("Missing or unknown contract fields".into());
    }
    Ok(())
}
fn bounded(v: &Value, low: f64, high: f64) -> Check<f64> {
    let n = number(v)?;
    if n < low || n > high {
        Err("Number outside its declared range".into())
    } else {
        Ok(n)
    }
}
fn interval(v: &Value) -> Check<[f64; 2]> {
    let a = array(v)?;
    if a.len() != 2 {
        return Err("Interval needs two endpoints".into());
    }
    let lo = number(&a[0])?;
    let hi = number(&a[1])?;
    if lo > hi {
        return Err("Reversed interval".into());
    }
    Ok([lo, hi])
}
fn positive_interval(v: &Value, low: f64) -> Check<[f64; 2]> {
    let [a, b] = interval(v)?;
    if a < low || a >= b {
        Err("Interval must be positive and strictly ordered".into())
    } else {
        Ok([a, b])
    }
}

/// Explicit RFC3339 timestamp to Unix seconds; no local-zone or epoch guessing.
pub fn utc_seconds(value: &str) -> Check<f64> {
    if !value.is_ascii()
        || value.len() < 20
        || &value[4..5] != "-"
        || &value[7..8] != "-"
        || &value[10..11] != "T"
        || &value[13..14] != ":"
        || &value[16..17] != ":"
    {
        return Err("Timestamp needs RFC3339 date, time and explicit zone".into());
    }
    let digit = |s: &str| {
        if s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<i64>()
                .map_err(|_| "Invalid timestamp number".to_string())
        } else {
            Err("Invalid timestamp number".to_string())
        }
    };
    let (year, month, day) = (
        digit(&value[..4])?,
        digit(&value[5..7])?,
        digit(&value[8..10])?,
    );
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let lengths = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > lengths[(month - 1) as usize]
    {
        return Err("Invalid calendar date".into());
    }
    let (end, offset) = if value.ends_with('Z') {
        (value.len() - 1, 0)
    } else {
        let start = value.len().checked_sub(6).ok_or("Timezone missing")?;
        if start < 19
            || &value[start + 3..start + 4] != ":"
            || !matches!(&value[start..start + 1], "+" | "-")
        {
            return Err("Explicit timezone required".into());
        }
        let h = digit(&value[start + 1..start + 3])?;
        let m = digit(&value[start + 4..])?;
        if h > 23 || m > 59 {
            return Err("Invalid timezone".into());
        }
        (
            start,
            (h * 3600 + m * 60)
                * if &value[start..start + 1] == "-" {
                    -1
                } else {
                    1
                },
        )
    };
    let hour = digit(&value[11..13])?;
    let minute = digit(&value[14..16])?;
    let seconds = &value[17..end];
    if seconds.len() < 2
        || !seconds[..2].bytes().all(|b| b.is_ascii_digit())
        || (seconds.len() > 2
            && (seconds.as_bytes()[2] != b'.'
                || seconds.len() == 3
                || !seconds[3..].bytes().all(|b| b.is_ascii_digit())))
    {
        return Err("Invalid seconds format".into());
    }
    let sec = seconds.parse::<f64>().map_err(|_| "Invalid seconds")?;
    if hour > 23 || minute > 59 || !(0.0..60.0).contains(&sec) || !sec.is_finite() {
        return Err("Invalid clock time".into());
    }
    let y = year - 1;
    let days = 365 * y + y / 4 - y / 100
        + y / 400
        + lengths[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1
        - 719162;
    Ok((days * 86400 + hour * 3600 + minute * 60 - offset) as f64 + sec)
}

/// WGS84 inverse geodesic (Vincenty). Near-antipodal failure is explicit; it
/// cannot silently shrink an ambiguity envelope.
pub fn distance_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> Check<f64> {
    if ![lat1, lon1, lat2, lon2].iter().all(|x| x.is_finite())
        || lat1.abs() > 90.
        || lat2.abs() > 90.
        || lon1.abs() > 180.
        || lon2.abs() > 180.
    {
        return Err("Invalid coordinates".into());
    }
    let f = 1. / 298.257_223_563;
    let a = 6_378_137.;
    let b = a * (1. - f);
    let u1 = ((1. - f) * lat1.to_radians().tan()).atan();
    let u2 = ((1. - f) * lat2.to_radians().tan()).atan();
    let l = (lon2 - lon1).to_radians();
    let mut lambda = l;
    for _ in 0..200 {
        let (sl, cl) = lambda.sin_cos();
        let su = ((u2.cos() * sl).powi(2)
            + (u1.cos() * u2.sin() - u1.sin() * u2.cos() * cl).powi(2))
        .sqrt();
        let cu = u1.sin() * u2.sin() + u1.cos() * u2.cos() * cl;
        if su < 1e-15 && cu > 0. {
            return Ok(0.);
        }
        if su < 1e-15 {
            return Err("Antipodal geodesic is not supported by this inverse solver".into());
        }
        let sigma = su.atan2(cu);
        let sa = u1.cos() * u2.cos() * sl / su;
        let ca2 = 1. - sa * sa;
        let c2 = if ca2.abs() < 1e-15 {
            0.
        } else {
            cu - 2. * u1.sin() * u2.sin() / ca2
        };
        let c = f / 16. * ca2 * (4. + f * (4. - 3. * ca2));
        let next = l + (1. - c) * f * sa * (sigma + c * su * (c2 + c * cu * (-1. + 2. * c2 * c2)));
        if (next - lambda).abs() < 1e-12 {
            let u2v = ca2 * (a * a - b * b) / (b * b);
            let aa = 1. + u2v / 16384. * (4096. + u2v * (-768. + u2v * (320. - 175. * u2v)));
            let bb = u2v / 1024. * (256. + u2v * (-128. + u2v * (74. - 47. * u2v)));
            let ds = bb
                * su
                * (c2
                    + bb / 4.
                        * (cu * (-1. + 2. * c2 * c2)
                            - bb / 6. * c2 * (-3. + 4. * su * su) * (-3. + 4. * c2 * c2)));
            return Ok(b * aa * (sigma - ds));
        }
        lambda = next;
    }
    Err("Geodesic did not converge; location separation is unresolved".into())
}

fn validate(input: &Value) -> Check<()> {
    let events = array(&input["events"])?;
    let candidates = array(&input["candidates"])?;
    let mut ids = BTreeSet::new();
    for e in events {
        record(
            e,
            &["event_id", "start_s", "end_s", "band_hz", "ridge"],
            &[
                "harmonic_group_id",
                "cpa_receive_s",
                "f0_range_hz",
                "speed_range_mps",
                "slant_range_m",
                "quality",
            ],
        )?;
        if !ids.insert(text(&e["event_id"])?) {
            return Err("Duplicate event ID".into());
        }
        let start = bounded(&e["start_s"], 0., f64::MAX)?;
        let end = number(&e["end_s"])?;
        if start >= end {
            return Err("Invalid event window".into());
        }
        let [low, high] = positive_interval(&e["band_hz"], 1e-12)?;
        if let Some(g) = e.get("harmonic_group_id") {
            text(g)?;
        }
        if let Some(c) = e.get("cpa_receive_s") {
            bounded(c, 0., f64::MAX)?;
        }
        for key in ["f0_range_hz", "speed_range_mps", "slant_range_m"] {
            if let Some(v) = e.get(key) {
                positive_interval(v, 1e-12)?;
            }
        }
        let points = array(&e["ridge"])?;
        if points.len() < 3 {
            return Err("Ridge needs three points".into());
        }
        let mut previous = f64::NEG_INFINITY;
        for p in points {
            record(p, &["time_s", "frequency_hz"], &["weight"])?;
            let t = bounded(&p["time_s"], start, end)?;
            bounded(&p["frequency_hz"], low, high)?;
            if t <= previous {
                return Err("Ridge times must increase".into());
            }
            previous = t;
            if let Some(w) = p.get("weight") {
                bounded(w, 0., f64::MAX)?;
            }
        }
        if let Some(q) = e.get("quality") {
            for (k, v) in q.as_object().ok_or("Quality must be an object")? {
                if k.trim().is_empty() {
                    return Err("Empty quality name".into());
                }
                number(v)?;
            }
        }
    }
    let mut last = f64::INFINITY;
    let mut kind = None;
    for c in candidates {
        record(
            c,
            &[
                "lat",
                "lon",
                "score",
                "score_kind",
                "event_ids",
                "flight_ids",
                "recording_start_utc",
            ],
            &["uncertainty"],
        )?;
        bounded(&c["lat"], -90., 90.)?;
        bounded(&c["lon"], -180., 180.)?;
        let score = number(&c["score"])?;
        if score > last {
            return Err("Candidates must be ranked in descending score order".into());
        }
        last = score;
        let k = text(&c["score_kind"])?;
        if kind.is_some_and(|old| old != k) {
            return Err("Incomparable score kinds".into());
        }
        kind = Some(k);
        let ei = texts(&c["event_ids"])?;
        let fi = texts(&c["flight_ids"])?;
        if ei.is_empty()
            || ei.len() != fi.len()
            || ei.iter().collect::<BTreeSet<_>>().len() != ei.len()
            || ei.iter().any(|id| !ids.contains(*id))
        {
            return Err("Invalid event/flight association".into());
        }
        utc_seconds(text(&c["recording_start_utc"])?)?;
        if let Some(u) = c.get("uncertainty") {
            record(
                u,
                &["radius_m", "method", "kind"],
                &["coverage_probability", "calibration_reference"],
            )?;
            bounded(&u["radius_m"], 0., f64::MAX)?;
            text(&u["method"])?;
            match text(&u["kind"])? {
                "calibrated" => {
                    bounded(&u["coverage_probability"], 0., 1.)?;
                    text(&u["calibration_reference"])?;
                }
                "sensitivity"
                    if u.get("coverage_probability").is_none()
                        && u.get("calibration_reference").is_none() => {}
                _ => return Err("Invalid uncertainty/probability claim".into()),
            }
        }
    }
    if let Some(cov) = input.get("coverage").filter(|v| !v.is_null()) {
        record(
            cov,
            &["search_complete", "data_status", "limitations"],
            &["requested_area", "missing_data"],
        )?;
        if cov["search_complete"].as_bool().is_none()
            || !["available", "partial", "unavailable"].contains(&text(&cov["data_status"])?)
        {
            return Err("Invalid coverage flags".into());
        }
        texts(&cov["limitations"])?;
        if let Some(m) = cov.get("missing_data") {
            texts(m)?;
        }
        if let Some(area) = cov.get("requested_area") {
            record(area, &["coverage"], &["bounds_wsen"])?;
            if area["coverage"] != "conus" {
                return Err("Unknown upstream area contract".into());
            }
            if let Some(v) = area.get("bounds_wsen") {
                let a = array(v)?;
                if a.len() != 4 {
                    return Err("Area requires four bounds".into());
                }
                for (i, x) in a.iter().enumerate() {
                    bounded(
                        x,
                        if i % 2 == 0 { -180. } else { -90. },
                        if i % 2 == 0 { 180. } else { 90. },
                    )?;
                }
                if number(&a[0])? >= number(&a[2])? || number(&a[1])? >= number(&a[3])? {
                    return Err("Invalid area order".into());
                }
            }
        }
    }
    Ok(())
}

fn outcome(status: &str, reason: &str, details: Value) -> Value {
    json!({"status":status,"reason_code":reason,"accepted_location":status=="succeeded","diagnostics":{"policy_version":"engineering-v3-with-finite-envelope","calibration":"uncalibrated","probability":null,"details":details,"limits":{"target_radius_m":1000.,"min_independent_events":2,"max_event_rmse_hz":3.,"max_outlier_fraction":0.25,"min_support_fraction":0.95},"limitations":["Regression-tested engineering rules, not calibrated location confidence","Availability of archives does not establish capture of every aircraft","Evaluated grids do not prove continuous-space uniqueness"]}})
}
fn inconclusive(reason: &str) -> Value {
    outcome("inconclusive", reason, json!({}))
}
fn invalid(error: impl ToString) -> Value {
    outcome(
        "failed",
        "invalid_evidence",
        json!({"validation_error":error.to_string()}),
    )
}
fn group(event: &Value) -> &str {
    event
        .get("harmonic_group_id")
        .unwrap_or(&event["event_id"])
        .as_str()
        .unwrap()
}
fn tied(gap: f64, margin: f64) -> bool {
    gap <= margin + 1e-12_f64.max(margin.abs() * 1e-12)
}

/// Assess supplied producer evidence. No benchmark labels or true coordinates
/// are parameters. Returning succeeded only passes this uncalibrated policy.
pub fn decide(input: &Value) -> Value {
    if let Err(e) = validate(input) {
        return invalid(e);
    }
    let events = input["events"].as_array().unwrap();
    let candidates = input["candidates"].as_array().unwrap();
    let empty = json!({});
    let evidence = input
        .get("matching_diagnostics")
        .filter(|v| !v.is_null())
        .unwrap_or(&empty);
    if !evidence.is_object() {
        return invalid("Matching diagnostics must be an object");
    }
    if events.is_empty() {
        return inconclusive("no_doppler_events");
    }
    let cov = &input["coverage"];
    if cov.is_null() {
        return inconclusive("coverage_unknown");
    }
    if cov["data_status"] == "unavailable" {
        return inconclusive("missing_flight_data");
    }
    if cov["search_complete"] != true {
        return inconclusive("search_incomplete");
    }
    if cov["data_status"] != "available"
        || cov
            .get("missing_data")
            .is_some_and(|m| m.as_array().is_some_and(|v| !v.is_empty()))
    {
        return inconclusive("partial_flight_coverage");
    }
    if candidates.is_empty() {
        return inconclusive("no_matching_flights");
    }
    let lookup: BTreeMap<_, _> = events
        .iter()
        .map(|e| (e["event_id"].as_str().unwrap(), e))
        .collect();
    let best = &candidates[0];
    let selected = set(&best["event_ids"]).unwrap();
    let mut groups = BTreeMap::new();
    for (id, flight) in texts(&best["event_ids"])
        .unwrap()
        .into_iter()
        .zip(texts(&best["flight_ids"]).unwrap())
    {
        let event = lookup[id];
        let g = group(event);
        if groups.get(g).is_some_and(|(_, old)| *old != flight) {
            return inconclusive("inconsistent_harmonic_association");
        }
        groups.insert(g, (id, flight));
    }
    let sensitivity = &evidence["sensitivity"];
    let window = evidence.get("recording_start_window_s");
    if let Some(w) = window {
        let Ok([lo, hi]) = interval(w) else {
            return invalid("Invalid trusted clock window");
        };
        let fitted = utc_seconds(best["recording_start_utc"].as_str().unwrap()).unwrap();
        if fitted < lo || fitted > hi {
            return inconclusive("recording_clock_mismatch");
        }
    }
    let known_clock = window
        .and_then(|w| interval(w).ok())
        .zip(interval(&sensitivity["settings"]["clock_bounds_s"]).ok())
        .is_some_and(|([a, b], [c, d])| c <= a && d >= b);
    if groups.len() < 2 && !(groups.len() == 1 && known_clock) {
        return inconclusive("insufficient_independent_events");
    }
    let unmatched: BTreeSet<_> = lookup
        .keys()
        .copied()
        .filter(|k| !selected.contains(k))
        .collect();
    if let Some(v) = evidence.get("supported_event_ids") {
        let Ok(ids) = set(v) else {
            return invalid("Invalid supported events");
        };
        if ids.iter().any(|id| !lookup.contains_key(id)) {
            return invalid("Unknown supported event");
        }
        if !ids.is_subset(&selected) {
            return inconclusive("unmatched_supported_events");
        }
    }
    let subset = best["score_kind"] == "matched_group_support_minus_normalized_residual";
    if !unmatched.is_empty() {
        let declared = evidence.get("unmatched_event_ids");
        if !subset
            || declared.and_then(|v| set(v).ok()).as_ref() != Some(&unmatched)
            || declared
                .and_then(|v| v.as_array())
                .is_none_or(|v| v.len() != unmatched.len())
        {
            return inconclusive("unexplained_events");
        }
        if unmatched
            .iter()
            .any(|id| groups.contains_key(group(lookup[id])))
        {
            return inconclusive("incomplete_harmonic_group");
        }
    }
    if evidence["clock_locked"] != true {
        return inconclusive("timing_not_established");
    }
    // Check every measured harmonic as well as every independent flyover. This
    // is deliberately stricter than allowing a later harmonic to hide a bad one.
    for id in &selected {
        let fit = &evidence[*id];
        let (Ok(rmse), Ok(outliers), Ok(support)) = (
            number(&fit["rmse_hz"]),
            number(&fit["outlier_fraction"]),
            number(&fit["support_fraction"]),
        ) else {
            return inconclusive("matching_diagnostics_missing");
        };
        if rmse < 0. || !(0.0..=1.0).contains(&outliers) || !(0.0..=1.0).contains(&support) {
            return invalid("Impossible event diagnostics");
        }
        if support < 0.95 {
            return inconclusive("insufficient_track_support");
        }
        if rmse > 3. || outliers > 0.25 {
            return inconclusive("inconsistent_event_match");
        }
        if fit["drift_at_bound"] != false {
            return inconclusive("source_drift_unresolved");
        }
    }
    if best["score_kind"] != "negative_robust_ridge_loss_hz" && !subset {
        return inconclusive("unsupported_score_kind");
    }
    let finite = evidence.get("finite_search_evidence");
    if let Some(f) = finite {
        match finite_check(f, evidence, candidates, &lookup, subset) {
            Ok(Some(reason)) => return inconclusive(reason),
            Err(e) => return invalid(e),
            _ => {}
        }
    }
    let radius = best["uncertainty"]["radius_m"]
        .as_f64()
        .unwrap_or(1000.)
        .min(1000.);
    let mut near = vec![(0, 0.)];
    let associations = |c: &Value| {
        texts(&c["event_ids"])
            .unwrap()
            .into_iter()
            .zip(texts(&c["flight_ids"]).unwrap())
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect::<BTreeMap<_, _>>()
    };
    for (i, alt) in candidates.iter().enumerate().skip(1) {
        if !subset && set(&alt["event_ids"]).unwrap() != selected {
            return inconclusive("incomparable_candidates");
        }
        let separation = match distance_m(
            best["lat"].as_f64().unwrap(),
            best["lon"].as_f64().unwrap(),
            alt["lat"].as_f64().unwrap(),
            alt["lon"].as_f64().unwrap(),
        ) {
            Ok(x) => x,
            Err(_) => return inconclusive("ambiguous_locations"),
        };
        let same = associations(best) == associations(alt);
        let count = texts(&alt["event_ids"])
            .unwrap()
            .iter()
            .map(|id| group(lookup[id]))
            .collect::<BTreeSet<_>>()
            .len();
        let gap = (best["score"].as_f64().unwrap() - alt["score"].as_f64().unwrap())
            / groups.len().max(count) as f64;
        if tied(gap, if subset { 0.2 } else { 0.5 }) {
            if !same || (finite.is_none() && separation > radius) {
                return inconclusive(if separation > radius {
                    "ambiguous_locations"
                } else {
                    "ambiguous_flight_match"
                });
            }
            near.push((i, separation));
        }
    }
    if finite.is_none() && evidence["alternatives_evaluated"] != true {
        return inconclusive("alternatives_not_assessed");
    }
    let sources = match sensitivity.get("sources") {
        Some(v) => match set(v) {
            Ok(s) => s,
            Err(e) => return invalid(e),
        },
        None => BTreeSet::new(),
    };
    if !["frequency", "clock", "altitude", "sound_speed"]
        .iter()
        .all(|s| sources.contains(s))
        || sensitivity["complete"] != true
    {
        return inconclusive("uncertainty_not_established");
    }
    if sensitivity["boundary_limited"] != false || evidence["refinement_boundary_limited"] != false
    {
        return inconclusive("refinement_boundary_limited");
    }
    let u = &best["uncertainty"];
    if u.is_null() {
        return inconclusive("uncertainty_not_established");
    }
    if u["kind"] != "sensitivity" {
        return inconclusive("calibration_not_verified");
    }
    let Ok(tested) = bounded(&sensitivity["radius_m"], 0., f64::MAX) else {
        return inconclusive("uncertainty_not_established");
    };
    let reported = u["radius_m"].as_f64().unwrap();
    if reported < tested {
        return inconclusive("uncertainty_not_established");
    }
    if reported > 1000. {
        return inconclusive("location_uncertainty_too_large");
    }
    if finite.is_some()
        && near
            .iter()
            .any(|(i, _)| candidates[*i]["uncertainty"]["kind"] != "sensitivity")
    {
        return inconclusive("uncertainty_not_established");
    }
    let union = near
        .iter()
        .map(|(i, d)| {
            d + candidates[*i]["uncertainty"]["radius_m"]
                .as_f64()
                .unwrap_or(0.)
        })
        .fold(reported, f64::max);
    if finite.is_some() && union > 1000. {
        return inconclusive("ambiguous_locations");
    }
    let mut result = outcome(
        "succeeded",
        "supported_by_evidence_checks",
        json!({"independent_event_groups":groups.len(),"radius_m":if finite.is_some(){union}else{reported},"known_clock_single_event":groups.len()==1 && known_clock,"unmatched_event_ids":unmatched,"finite_grid_only":finite.is_some()}),
    );
    result["candidates"] = json!(candidates);
    if finite.is_some() {
        result["candidates"][0]["uncertainty"]["radius_m"] = json!(union);
        result["candidates"][0]["uncertainty"]["method"] =
            json!("finite_mode_and_tested_profile_union");
    }
    result
}

fn finite_check<'a>(
    f: &Value,
    d: &Value,
    candidates: &[Value],
    lookup: &BTreeMap<&'a str, &'a Value>,
    subset: bool,
) -> Check<Option<&'static str>> {
    if f["schema_version"].as_u64() != Some(1) {
        return Err("Unsupported finite-search schema".into());
    }
    if f["scope"] != "evaluated_finite_grid" {
        return Ok(Some("finite_search_scope_unsupported"));
    }
    for k in ["complete", "pruning_accounted_for"] {
        if f[k].as_bool().is_none() {
            return Err("Finite completion flags must be boolean".into());
        }
    }
    if f["complete"] != true || f["pruning_accounted_for"] != true {
        return Ok(Some("finite_search_incomplete"));
    }
    if f["score_kind"] != candidates[0]["score_kind"] {
        return Ok(Some("incomparable_candidates"));
    }
    for k in ["spacing_m", "refine_spacing_m"] {
        if number(&f[k])? <= 0. {
            return Err("Grid spacing must be positive".into());
        }
    }
    let counts: Vec<_> = candidates
        .iter()
        .map(|c| {
            texts(&c["event_ids"])
                .unwrap()
                .iter()
                .map(|id| group(lookup[id]))
                .collect::<BTreeSet<_>>()
                .len()
        })
        .collect();
    let maximum = f["max_event_groups"]
        .as_u64()
        .ok_or("Invalid maximum event count")? as usize;
    if maximum < *counts.iter().max().unwrap() {
        return Err("Inconsistent maximum groups".into());
    }
    let modes = array(&f["retained_modes"])?;
    if modes.is_empty() {
        return Err("Retained modes missing".into());
    }
    let mut by_id = BTreeMap::new();
    for m in modes {
        let id = text(&m["mode_id"])?;
        let score = number(&m["score"])?;
        let count = m["event_groups"]
            .as_u64()
            .ok_or("Invalid mode group count")? as usize;
        if count == 0 || count > maximum || by_id.insert(id, (score, count)).is_some() {
            return Err("Duplicate/inconsistent mode".into());
        }
    }
    let (Ok(assessed), Ok(mapped)) = (
        set(&d["assessed_mode_ids"]),
        texts(&d["candidate_mode_ids"]),
    ) else {
        return Ok(Some("finite_modes_not_assessed"));
    };
    let ids: BTreeSet<_> = by_id.keys().copied().collect();
    if mapped.len() != candidates.len() || mapped.iter().copied().collect::<BTreeSet<_>>() != ids {
        return Ok(Some("finite_candidate_mapping_incomplete"));
    }
    let mut returned = BTreeMap::new();
    for ((id, count), c) in mapped.iter().zip(counts).zip(candidates) {
        if by_id[id].1 != count {
            return Ok(Some("finite_candidate_mapping_incomplete"));
        }
        let score = c["score"].as_f64().unwrap();
        returned
            .entry(*id)
            .and_modify(|v: &mut f64| *v = v.max(score))
            .or_insert(score);
    }
    if by_id
        .iter()
        .any(|(id, (score, _))| *score > returned[id] + 1e-6)
    {
        return Ok(Some("finite_candidate_scores_inconsistent"));
    }
    if !ids.is_subset(&assessed) {
        return Ok(Some("finite_modes_not_assessed"));
    }
    let discarded = f["discarded_count"]
        .as_u64()
        .ok_or("Invalid discarded count")?;
    texts(&f["limitations"])?;
    if discarded == 0 {
        if !f["best_discarded_score"].is_null() {
            return Err("Zero discarded candidates require a null bound".into());
        }
    } else {
        let bound = number(&f["best_discarded_score"])?;
        let gap = (candidates[0]["score"].as_f64().unwrap() - bound) / maximum as f64;
        if tied(gap, if subset { 0.2 } else { 0.5 }) {
            return Ok(Some("retention_may_hide_alternatives"));
        }
    }
    Ok(None)
}
