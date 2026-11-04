//! Audio-only inference and explicit transfer of its engineering envelopes to
//! a regional experiment. Coordinates and flight data are not fitting inputs.
use crate::{
    acoustic_fit::{self, FitReport, FitSettings},
    acquisition::sha256_file,
    physics::{EventTrace, ModelConfig, PairConfig},
    reviewed_search::{fingerprint, load_reviewed_measurements},
    search_setup::{self, SearchSetup},
    verification::{CoarseBounds, read_json, validate_search_input, write_json},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RidgeChoice {
    pub harmonic_group_id: String,
    pub event_id: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchPadding {
    pub frequency_margin_hz: f64,
    pub timing_margin_s: f64,
    pub speed_fraction: f64,
    pub slant_fraction: f64,
    pub slant_min_margin_m: f64,
    pub frequency_step_hz: f64,
    pub shift_step_s: f64,
}
impl Default for SearchPadding {
    fn default() -> Self {
        Self {
            frequency_margin_hz: 2.,
            timing_margin_s: 2.5,
            speed_fraction: 0.2,
            slant_fraction: 0.35,
            slant_min_margin_m: 500.,
            frequency_step_hz: 0.5,
            shift_step_s: 0.5,
        }
    }
}
impl SearchPadding {
    fn validate(&self) -> Result<(), String> {
        if [
            self.frequency_margin_hz,
            self.timing_margin_s,
            self.speed_fraction,
            self.slant_fraction,
            self.slant_min_margin_m,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.)
            || [self.frequency_step_hz, self.shift_step_s]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.)
        {
            return Err(
                "Search padding must be finite and nonnegative; grid steps positive".into(),
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioRequest {
    pub schema_version: u32,
    pub discovery: PathBuf,
    pub review_selection: PathBuf,
    pub selected_ridges: [RidgeChoice; 2],
    pub selection_basis: String,
    pub source_override: Option<PathBuf>,
    #[serde(default)]
    pub fit_settings: FitSettings,
    #[serde(default)]
    pub search_padding: SearchPadding,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioGeometry {
    pub schema_version: u32,
    pub normalized_tracks: PathBuf,
    pub observer_bounds: [f64; 4],
    pub grid_spacing_m: f64,
    pub observer_ellipsoid_height_m: f64,
    pub observers_file: Option<PathBuf>,
    pub clock_offset_bounds_s: Option<[f64; 2]>,
    pub model: ModelConfig,
    pub provenance: Vec<String>,
}

fn fresh(output: &Path) -> Result<(), String> {
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir(output).map_err(|e| format!("Use a new output directory: {e}"))
}
fn failure(output: &Path, error: &str, files: &[&str]) {
    let mut cleanup = Vec::new();
    for name in files {
        let path = output.join(name);
        if path.exists()
            && let Err(e) = fs::remove_file(&path)
        {
            cleanup.push(format!("{}: {e}", path.display()));
        }
    }
    let _ = write_json(
        &output.join("failure.json"),
        &json!({"status":"failed","location_verified":false,"error":error,"completion_artifact_cleanup_errors":cleanup}),
    );
}
fn check_dependencies(rows: &[(PathBuf, Value)]) -> Result<(), String> {
    for (path, before) in rows {
        if sha256_file(path)? != before["sha256"] {
            return Err(format!("Input changed: {}", path.display()));
        }
    }
    Ok(())
}

/// Grid includes the requested envelope endpoints and fitted center, alongside
/// global step multiples. No shrinking/rounding inward of uncertainty bounds.
fn inclusive_grid(bounds: [f64; 2], center: f64, step: f64) -> Result<Vec<f64>, String> {
    let [lo, hi] = bounds;
    if !lo.is_finite()
        || !hi.is_finite()
        || lo > hi
        || !center.is_finite()
        || center < lo
        || center > hi
        || !step.is_finite()
        || step <= 0.
    {
        return Err("Invalid derived grid bounds".into());
    }
    let first = (lo / step).ceil();
    let last = (hi / step).floor();
    if !first.is_finite()
        || !last.is_finite()
        || last - first > 9995.
        || first.abs() > 9e15
        || last.abs() > 9e15
    {
        return Err("Derived grid exceeds the 10000-point budget; inspect identifiability or explicitly increase grid spacing".into());
    }
    let mut grid = vec![lo, hi, center];
    if first <= last {
        for i in 0..=((last - first) as usize) {
            grid.push((first + i as f64) * step);
        }
    }
    grid.sort_by(f64::total_cmp);
    grid.dedup();
    Ok(grid)
}

fn derive_ranges(
    fits: &[FitReport; 2],
    settings: &FitSettings,
    p: &SearchPadding,
) -> Result<Value, String> {
    p.validate()?;
    let mut ranges = Vec::new();
    for fit in fits {
        if !fit.usable_for_search {
            return Err("An acoustic fit is not usable for search".into());
        }
        let f = fit.fit.as_ref().ok_or("Missing acoustic fit")?;
        let e = fit
            .envelope
            .as_ref()
            .ok_or("Missing acoustic sensitivity envelope")?;
        let speed_pad = p.speed_fraction * f.speed_mps;
        let distance_pad = (p.slant_fraction * f.slant_distance_m).max(p.slant_min_margin_m);
        let raw_speed = [e.speed_mps[0] - speed_pad, e.speed_mps[1] + speed_pad];
        let raw_slant = [
            e.slant_distance_m[0] - distance_pad,
            e.slant_distance_m[1] + distance_pad,
        ];
        let speed = [
            raw_speed[0].max(settings.speed_bounds_mps[0]),
            raw_speed[1].min(settings.speed_bounds_mps[1]),
        ];
        let slant = [
            raw_slant[0].max(settings.slant_bounds_m[0]),
            raw_slant[1].min(settings.slant_bounds_m[1]),
        ];
        let frequency = [
            (e.emitted_frequency_hz[0] - p.frequency_margin_hz).max(f64::MIN_POSITIVE),
            e.emitted_frequency_hz[1] + p.frequency_margin_hz,
        ];
        let tau = [
            e.heard_cpa_recording_s[0] - p.timing_margin_s,
            e.heard_cpa_recording_s[1] + p.timing_margin_s,
        ];
        if speed[0] > speed[1] || slant[0] > slant[1] {
            return Err(
                "Padded acoustic bounds do not intersect the declared physical domain".into(),
            );
        }
        let f0 = inclusive_grid(frequency, f.emitted_frequency_hz, p.frequency_step_hz)?;
        ranges.push(json!({"emitted_frequency_hz":frequency,"f0_hz":f0,"heard_cpa_recording_s":tau,"speed_mps":speed,"slant_distance_m":slant,"physical_domain_clipping":{"speed":speed!=raw_speed,"slant":slant!=raw_slant}}));
    }
    let first = fits[0].fit.as_ref().unwrap().heard_cpa_recording_s;
    let second = fits[1].fit.as_ref().unwrap().heard_cpa_recording_s;
    let t1: [f64; 2] = serde_json::from_value(ranges[0]["heard_cpa_recording_s"].clone())
        .map_err(|e| e.to_string())?;
    let t2: [f64; 2] = serde_json::from_value(ranges[1]["heard_cpa_recording_s"].clone())
        .map_err(|e| e.to_string())?;
    let separation = second - first;
    let bounds = [t2[0] - t1[1], t2[1] - t1[0]];
    if bounds[0] <= 0. || separation <= 0. {
        return Err("Acoustic CPA ordering is ambiguous".into());
    }
    Ok(
        json!({"event1_cpa_recording_s":first,"shared_shifts_s":inclusive_grid([t1[0]-first,t1[1]-first],0.,p.shift_step_s)?,"coarse_bounds":ranges.iter().map(|r|json!({"speed_mps":r["speed_mps"],"slant_distance_m":r["slant_distance_m"]})).collect::<Vec<_>>(),"measured_separation_s":separation,"separation_tolerance_s":(separation-bounds[0]).max(bounds[1]-separation),"separation_bounds_s":bounds,"event_ranges":ranges,"basis":"Audio fit sensitivity envelopes plus declared engineering padding, intersected with the declared physical domain; not calibrated confidence intervals. Endpoint and center inclusion avoids shrinking the search by grid rounding."}),
    )
}

pub fn derive(config_path: &Path, output: &Path) -> Result<Value, String> {
    fresh(output)?;
    let result = derive_inner(config_path, output);
    if let Err(e) = &result {
        failure(output, e, &["priors.json", "receipt.json"]);
    }
    result
}

fn derive_inner(config_path: &Path, output: &Path) -> Result<Value, String> {
    let request_receipt = fingerprint(config_path)?;
    let request: AudioRequest = read_json(config_path)?;
    if request.schema_version != 1 || request.selection_basis.trim().is_empty() {
        return Err("Audio request needs schema 1 and an explicit selection_basis".into());
    }
    request.search_padding.validate()?;
    let selections = request
        .selected_ridges
        .each_ref()
        .map(|s| (s.harmonic_group_id.as_str(), s.event_id.as_str()));
    let base = config_path.parent().unwrap_or(Path::new("."));
    let mut reviewed = load_reviewed_measurements(
        base,
        &request.discovery,
        &request.review_selection,
        request.source_override.as_deref(),
        selections,
    )?;
    reviewed
        .dependencies
        .push((config_path.to_path_buf(), request_receipt.clone()));
    let fits = [
        acoustic_fit::fit(
            &reviewed.traces[0].points,
            reviewed.frequency_sigma_hz[0],
            &request.fit_settings,
        )?,
        acoustic_fit::fit(
            &reviewed.traces[1].points,
            reviewed.frequency_sigma_hz[1],
            &request.fit_settings,
        )?,
    ];
    let mut reason = None;
    let derived = match derive_ranges(&fits, &request.fit_settings, &request.search_padding) {
        Ok(v) => Some(v),
        Err(e) => {
            reason = Some(e);
            None
        }
    };
    let usable = derived.is_some();
    let mut events = reviewed.traces;
    if let Some(ref d) = derived {
        for (i, event) in events.iter_mut().enumerate() {
            event.f0_hz = serde_json::from_value(d["event_ranges"][i]["f0_hz"].clone())
                .map_err(|e| e.to_string())?;
        }
    }
    let report = json!({"schema_version":1,"status":if usable{"audio_priors_ready"}else{"inconclusive_audio_priors"},"usable_for_search":usable,"location_verified":false,"utc_clock_inferred":false,"selection_basis":request.selection_basis,"selected_ridges":request.selected_ridges,"settings":request.fit_settings,"search_padding":request.search_padding,"frequency_sigma_hz":reviewed.frequency_sigma_hz,"fits":fits,"derived":derived,"events":events,"inconclusive_reason":reason,"provenance":{"request":request_receipt,"reviewed_audio":reviewed.evidence,"dependencies":reviewed.dependencies.iter().map(|(_,v)|v).collect::<Vec<_>>()},"limitations":["Straight, constant-speed, stable-tone acoustic approximation; actual flight motion and tone variation may bias these parameters","Envelopes and added search padding are engineering sensitivity ranges, not statistical confidence or coverage guarantees","Only recording-relative arrival timing is inferred; UTC date/start, region and terrain remain externally supplied","Event choices remain reviewed and can be source-informed; no aircraft identity is inferred from this stage","Other harmonic ridges are not scored; contrast weights are retained but equal sample weights are used"]});
    check_dependencies(&reviewed.dependencies)?;
    write_json(&output.join("priors.json"), &report)?;
    let receipt = json!({"schema_version":1,"status":report["status"],"location_verified":false,"priors":fingerprint(&output.join("priors.json"))?,"inputs_unchanged":true,"dependencies":reviewed.dependencies.iter().map(|(_,v)|v).collect::<Vec<_>>()});
    write_json(&output.join("receipt.json"), &receipt)?;
    Ok(report)
}

pub fn prepare(priors_path: &Path, geometry_path: &Path, output: &Path) -> Result<Value, String> {
    fresh(output)?;
    let result = prepare_inner(priors_path, geometry_path, output);
    if let Err(e) = &result {
        failure(output, e, &["search_input.json", "receipt.json"]);
    }
    result
}

fn prepare_inner(priors_path: &Path, geometry_path: &Path, output: &Path) -> Result<Value, String> {
    let priors_receipt = fingerprint(priors_path)?;
    let manifest_path = priors_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("receipt.json");
    let manifest_receipt = fingerprint(&manifest_path)?;
    let manifest: Value = read_json(&manifest_path)?;
    if manifest["schema_version"] != 1
        || manifest["inputs_unchanged"] != true
        || manifest["priors"]["sha256"] != priors_receipt["sha256"]
    {
        return Err("Audio priors do not match their derivation receipt".into());
    }
    let report: Value = read_json(priors_path)?;
    if report["schema_version"] != 1
        || report["status"] != "audio_priors_ready"
        || report["usable_for_search"] != true
        || report["location_verified"] != false
        || report["utc_clock_inferred"] != false
    {
        return Err("Only usable, unverified audio priors can prepare a search".into());
    }
    let request_path = report["provenance"]["request"]["path"]
        .as_str()
        .ok_or("Missing acoustic request path")?;
    let request: AudioRequest = read_json(Path::new(request_path))?;
    let request_base = Path::new(request_path).parent().unwrap_or(Path::new("."));
    let choices = request
        .selected_ridges
        .each_ref()
        .map(|s| (s.harmonic_group_id.as_str(), s.event_id.as_str()));
    let reviewed = load_reviewed_measurements(
        request_base,
        &request.discovery,
        &request.review_selection,
        request.source_override.as_deref(),
        choices,
    )?;
    let fits: [FitReport; 2] =
        serde_json::from_value(report["fits"].clone()).map_err(|e| e.to_string())?;
    let settings: FitSettings =
        serde_json::from_value(report["settings"].clone()).map_err(|e| e.to_string())?;
    let padding: SearchPadding =
        serde_json::from_value(report["search_padding"].clone()).map_err(|e| e.to_string())?;
    let derived = derive_ranges(&fits, &settings, &padding)?;
    if derived != report["derived"]
        || serde_json::to_value(&request.fit_settings).map_err(|e| e.to_string())?
            != report["settings"]
        || serde_json::to_value(&request.search_padding).map_err(|e| e.to_string())?
            != report["search_padding"]
        || serde_json::to_value(&request.selected_ridges).map_err(|e| e.to_string())?
            != report["selected_ridges"]
    {
        return Err("Derived ranges or settings differ from their acoustic request".into());
    }
    let events: [EventTrace; 2] =
        serde_json::from_value(report["events"].clone()).map_err(|e| e.to_string())?;
    for (i, event) in events.iter().enumerate() {
        if event.points.len() != reviewed.traces[i].points.len()
            || event
                .points
                .iter()
                .zip(&reviewed.traces[i].points)
                .any(|(a, b)| a.t_rec_s != b.t_rec_s || a.frequency_hz != b.frequency_hz)
            || serde_json::to_value(&event.f0_hz).map_err(|e| e.to_string())?
                != derived["event_ranges"][i]["f0_hz"]
        {
            return Err(
                "Audio search must preserve every reviewed measurement and derived frequency grid"
                    .into(),
            );
        }
    }
    let geometry_receipt = fingerprint(geometry_path)?;
    let geometry: AudioGeometry = read_json(geometry_path)?;
    if geometry.schema_version != 1 || geometry.model.sound_speed_mps != settings.sound_speed_mps {
        return Err("Geometry requires schema 1 and the same sound speed as the audio fit".into());
    }
    let base = geometry_path.parent().unwrap_or(Path::new("."));
    let mut dependencies = reviewed.dependencies;
    for item in report["provenance"]["dependencies"]
        .as_array()
        .ok_or("Missing audio dependencies")?
    {
        let path = PathBuf::from(
            item["path"]
                .as_str()
                .ok_or("Invalid audio dependency path")?,
        );
        dependencies.push((path, item.clone()));
    }
    dependencies.extend([
        (priors_path.to_path_buf(), priors_receipt.clone()),
        (manifest_path, manifest_receipt),
        (geometry_path.to_path_buf(), geometry_receipt.clone()),
    ]);
    for path in std::iter::once(&geometry.normalized_tracks).chain(geometry.observers_file.iter()) {
        let p = base.join(path);
        dependencies.push((p.clone(), fingerprint(&p)?));
    }
    let pair_config = PairConfig {
        event1_cpa_recording_s: derived["event1_cpa_recording_s"]
            .as_f64()
            .ok_or("Missing fitted CPA")?,
        shared_shifts_s: serde_json::from_value(derived["shared_shifts_s"].clone())
            .map_err(|e| e.to_string())?,
        clock_offset_bounds_s: geometry.clock_offset_bounds_s,
        model: geometry.model,
    };
    let coarse_bounds: [CoarseBounds; 2] =
        serde_json::from_value(derived["coarse_bounds"].clone()).map_err(|e| e.to_string())?;
    let config = SearchSetup {
        normalized_tracks: geometry.normalized_tracks,
        event_files: [PathBuf::new(), PathBuf::new()],
        observer_bounds: geometry.observer_bounds,
        grid_spacing_m: geometry.grid_spacing_m,
        observer_ellipsoid_height_m: geometry.observer_ellipsoid_height_m,
        observers_file: geometry.observers_file,
        pair_config,
        coarse_bounds,
        measured_separation_s: derived["measured_separation_s"]
            .as_f64()
            .ok_or("Missing fitted separation")?,
        separation_tolerance_s: derived["separation_tolerance_s"]
            .as_f64()
            .ok_or("Missing fitted tolerance")?,
        provenance: geometry.provenance,
    };
    let mut input = search_setup::prepare_events(config, base, events)?;
    input.provenance.push(format!("Audio-derived timing/frequency/coarse envelopes from priors SHA256 {}; no manual trace, CPA, emitted-tone grid or old refined candidate is read. Reviewed selection remains source-informed as declared: {}",priors_receipt["sha256"],request.selection_basis));
    input.provenance.push("Engineering sensitivity plus declared model padding, not calibrated confidence. UTC/day/region/terrain and stable-tone/straight-pass assumptions remain supplied. Measured samples and gaps are unchanged; other harmonics remain untested.".into());
    validate_search_input(&input)?;
    check_dependencies(&dependencies)?;
    write_json(&output.join("search_input.json"), &input)?;
    let receipt = json!({"schema_version":1,"status":"prepared_audio_search","location_verified":false,"priors":priors_receipt,"geometry":geometry_receipt,"search_input":fingerprint(&output.join("search_input.json"))?,"track_count":input.tracks.len(),"observer_count":input.observers.len(),"derived":derived,"dependencies":dependencies.iter().map(|(_,v)|v).collect::<Vec<_>>(),"inputs_unchanged":true});
    write_json(&output.join("receipt.json"), &receipt)?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grids_cover_endpoints_and_center_without_inward_rounding() {
        assert_eq!(
            inclusive_grid([1.1, 2.4], 1.7, 0.5).unwrap(),
            vec![1.1, 1.5, 1.7, 2., 2.4]
        );
        assert_eq!(
            inclusive_grid([-0.7, 0.8], 0., 0.5).unwrap(),
            vec![-0.7, -0.5, 0., 0.5, 0.8]
        );
        assert!(inclusive_grid([0., 1e10], 1., 0.5).is_err());
        assert!(inclusive_grid([2., 1.], 1., 0.5).is_err());
    }
    #[test]
    fn padding_requires_finite_nonnegative_margins_and_positive_steps() {
        let mut p = SearchPadding::default();
        p.validate().unwrap();
        p.speed_fraction = -0.1;
        assert!(p.validate().is_err());
        p = SearchPadding::default();
        p.shift_step_s = 0.;
        assert!(p.validate().is_err());
    }
}
