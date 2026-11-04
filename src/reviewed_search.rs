//! Bind reviewed acoustic proposals to an explicit, unchanged pair experiment.
//! The bridge copies measured samples, never interpolates gaps or selects a
//! harmonic by its fit to a candidate location. It does not accept a location.
use crate::{
    acquisition::sha256_file,
    physics::{EventTrace, TracePoint},
    search_setup,
    verification::{SearchInput, read_json, validate_search_input, write_json},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRidge {
    pub harmonic_group_id: String,
    pub event_id: String,
    /// Explicit emitted-frequency grid in the measured harmonic's own units.
    pub f0_hz: Vec<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewedSearchConfig {
    pub schema_version: u32,
    pub discovery: PathBuf,
    pub review_selection: PathBuf,
    pub template_search_input: Option<PathBuf>,
    pub search_config: Option<PathBuf>,
    pub selected_ridges: [SelectedRidge; 2],
    pub selection_basis: String,
    pub source_override: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewGroup {
    harmonic_group_id: String,
    review: String,
    event_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewSelection {
    schema_version: u32,
    discovery_sha256: String,
    source_sha256: String,
    selected_channel_index: usize,
    location_verified: bool,
    review_actor: String,
    scope: String,
    groups: Vec<ReviewGroup>,
}

#[derive(Deserialize)]
struct Discovery {
    schema_version: u32,
    status: String,
    location_verified: bool,
    source: PathBuf,
    source_sha256: String,
    source_metadata: SourceMetadata,
    settings: DiscoveryTiming,
    group_count: usize,
    ridge_count: usize,
    groups: Vec<DiscoveryGroup>,
    events: Vec<DiscoveryEvent>,
}

#[derive(Deserialize)]
struct SourceMetadata {
    selected_channel_index: usize,
    available_channels: usize,
    channels_averaged: bool,
    duration_s: f64,
}

#[derive(Deserialize)]
struct DiscoveryTiming {
    max_gap_s: f64,
}

#[derive(Deserialize)]
struct DiscoveryGroup {
    harmonic_group_id: String,
    start_s: f64,
    end_s: f64,
    ridge_count: usize,
    aircraft_identity_confirmed: bool,
}

#[derive(Deserialize)]
struct DiscoveryEvent {
    event_id: String,
    harmonic_group_id: String,
    start_s: f64,
    end_s: f64,
    band_hz: [f64; 2],
    ridge: Vec<MeasuredPoint>,
    quality: Value,
}

#[derive(Deserialize)]
struct MeasuredPoint {
    time_s: f64,
    frequency_hz: f64,
    weight: f64,
}

fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn window_valid(start: f64, end: f64, duration: f64) -> bool {
    start.is_finite() && end.is_finite() && start >= 0. && start < end && end <= duration
}

fn validate_discovery(d: &Discovery) -> Result<BTreeMap<&str, BTreeSet<&str>>, String> {
    if d.schema_version != 1 || d.status != "events_proposed" || d.location_verified {
        return Err(
            "Supply a completed, unverified discovery schema 1 with events_proposed".into(),
        );
    }
    let m = &d.source_metadata;
    if !digest_valid(&d.source_sha256)
        || d.source.as_os_str().is_empty()
        || !m.duration_s.is_finite()
        || m.duration_s <= 0.
        || m.duration_s > 86_401.
        || m.selected_channel_index >= m.available_channels
        || m.channels_averaged
        || !d.settings.max_gap_s.is_finite()
        || d.settings.max_gap_s <= 0.
        || d.group_count != d.groups.len()
        || d.ridge_count != d.events.len()
        || d.groups.is_empty()
        || d.groups.len() > 100_000
        || d.events.len() > 100_000
    {
        return Err("Discovery has inconsistent source, channel, timing, or counts".into());
    }
    let mut membership = BTreeMap::new();
    for g in &d.groups {
        if g.harmonic_group_id.trim().is_empty()
            || g.aircraft_identity_confirmed
            || g.ridge_count == 0
            || !window_valid(g.start_s, g.end_s, m.duration_s)
            || membership
                .insert(g.harmonic_group_id.as_str(), BTreeSet::new())
                .is_some()
        {
            return Err(
                "Discovery groups must be unique, nonempty and unconfirmed with valid windows"
                    .into(),
            );
        }
    }
    let mut event_ids = BTreeSet::new();
    let group_by_id = d
        .groups
        .iter()
        .map(|g| (g.harmonic_group_id.as_str(), g))
        .collect::<BTreeMap<_, _>>();
    let mut total_points = 0usize;
    for e in &d.events {
        total_points = total_points.saturating_add(e.ridge.len());
        if e.event_id.trim().is_empty()
            || !event_ids.insert(&e.event_id)
            || !(3..=100_000).contains(&e.ridge.len())
            || total_points > 10_000_000
            || !window_valid(e.start_s, e.end_s, m.duration_s)
            || !e.band_hz.iter().all(|f| f.is_finite() && *f > 0.)
            || e.band_hz[0] > e.band_hz[1]
            || !e.quality.is_object()
            || e.ridge.iter().any(|p| {
                !p.time_s.is_finite()
                    || p.time_s < e.start_s
                    || p.time_s > e.end_s
                    || !p.frequency_hz.is_finite()
                    || p.frequency_hz <= 0.
                    || p.frequency_hz < e.band_hz[0]
                    || p.frequency_hz > e.band_hz[1]
                    || !p.weight.is_finite()
                    || p.weight <= 0.
            })
            || e.ridge.windows(2).any(|p| p[0].time_s >= p[1].time_s)
        {
            return Err(
                "Discovery ridge IDs, measured points, weights, windows or bands are invalid"
                    .into(),
            );
        }
        membership
            .get_mut(e.harmonic_group_id.as_str())
            .ok_or("Discovery event refers to an unknown harmonic group")?
            .insert(e.event_id.as_str());
        let g = group_by_id[e.harmonic_group_id.as_str()];
        if e.start_s < g.start_s || e.end_s > g.end_s {
            return Err("Discovery event window lies outside its group".into());
        }
    }
    if d.groups
        .iter()
        .any(|g| membership[g.harmonic_group_id.as_str()].len() != g.ridge_count)
    {
        return Err("Discovery group ridge counts do not match actual membership".into());
    }
    Ok(membership)
}

fn validate_review<'a>(
    d: &'a Discovery,
    r: &'a ReviewSelection,
    discovery_hash: &str,
) -> Result<BTreeMap<&'a str, &'a str>, String> {
    let membership = validate_discovery(d)?;
    if r.schema_version != 2 || r.discovery_sha256 != discovery_hash {
        return Err("Review must be schema 2 and bind the exact discovery.json SHA-256; regenerate review with review-discovery and review its choices".into());
    }
    if r.source_sha256 != d.source_sha256
        || r.selected_channel_index != d.source_metadata.selected_channel_index
        || r.location_verified
        || !["human", "agent", "automated_test"].contains(&r.review_actor.as_str())
        || r.scope.trim().is_empty()
    {
        return Err("Review source, channel, actor or unverified scope is inconsistent".into());
    }
    let mut decisions = BTreeMap::new();
    for group in &r.groups {
        if !["unreviewed", "keep_for_matching", "reject", "uncertain"]
            .contains(&group.review.as_str())
            || decisions
                .insert(group.harmonic_group_id.as_str(), group.review.as_str())
                .is_some()
        {
            return Err("Review groups must be unique and use a supported review decision".into());
        }
        let ids = group
            .event_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if ids.len() != group.event_ids.len()
            || membership.get(group.harmonic_group_id.as_str()) != Some(&ids)
        {
            return Err("Review event membership differs from the bound discovery".into());
        }
    }
    if decisions.len() != membership.len() {
        return Err("Review must explicitly account for every discovery group".into());
    }
    Ok(decisions)
}

pub(crate) fn fingerprint(path: &Path) -> Result<Value, String> {
    Ok(
        json!({"path":path.canonicalize().map_err(|e|e.to_string())?,"bytes":fs::metadata(path).map_err(|e|e.to_string())?.len(),"sha256":sha256_file(path)?}),
    )
}

/// Shared evidence loading for audio-only inference. No historical search,
/// emitted-frequency grid, or manual timing anchor is read by this function.
pub(crate) struct ReviewedMeasurements {
    pub traces: [EventTrace; 2],
    pub frequency_sigma_hz: [f64; 2],
    pub evidence: Value,
    pub dependencies: Vec<(PathBuf, Value)>,
}

pub(crate) fn load_reviewed_measurements(
    base: &Path,
    discovery_file: &Path,
    review_file: &Path,
    source_override: Option<&Path>,
    selections: [(&str, &str); 2],
) -> Result<ReviewedMeasurements, String> {
    let discovery_path = base.join(discovery_file);
    let review_path = base.join(review_file);
    let discovery_receipt = fingerprint(&discovery_path)?;
    let review_receipt = fingerprint(&review_path)?;
    let discovery: Discovery = read_json(&discovery_path)?;
    let review: ReviewSelection = read_json(&review_path)?;
    let decisions = validate_review(
        &discovery,
        &review,
        discovery_receipt["sha256"].as_str().unwrap(),
    )?;
    let source_path = source_override.map(|p| base.join(p)).unwrap_or_else(|| {
        discovery_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&discovery.source)
    });
    let source_receipt = fingerprint(&source_path)?;
    if source_receipt["sha256"] != discovery.source_sha256 {
        return Err("Recording hash differs from the reviewed discovery".into());
    }
    if selections[0].0 == selections[1].0 || selections[0].1 == selections[1].1 {
        return Err("Select one ridge from each of two distinct reviewed groups".into());
    }
    let mut traces = Vec::new();
    let mut sigma = Vec::new();
    let mut selected_evidence = Vec::new();
    for (group, id) in selections {
        if decisions.get(group) != Some(&"keep_for_matching") {
            return Err("Every selected acoustic group must be kept for matching".into());
        }
        let event = discovery
            .events
            .iter()
            .find(|e| e.event_id == id && e.harmonic_group_id == group)
            .ok_or("Selected ridge is missing from its reviewed group")?;
        let scale = event.quality["frequency_sigma_hz"]
            .as_f64()
            .filter(|v| v.is_finite() && *v > 0.)
            .ok_or("Discovery ridge needs its positive nominal frequency_sigma_hz")?;
        sigma.push(scale);
        traces.push(EventTrace {
            points: event
                .ridge
                .iter()
                .map(|p| TracePoint {
                    t_rec_s: p.time_s,
                    frequency_hz: p.frequency_hz,
                })
                .collect(),
            f0_hz: Vec::new(),
        });
        selected_evidence.push(json!({"harmonic_group_id":group,"event_id":id,"point_count":event.ridge.len(),"quality":event.quality,"contrast_weights":event.ridge.iter().map(|p|p.weight).collect::<Vec<_>>(),"other_group_ridges_not_scored":discovery.events.iter().filter(|e|e.harmonic_group_id == group && e.event_id != id).map(|e|&e.event_id).collect::<Vec<_>>()}));
    }
    let traces: [EventTrace; 2] = traces.try_into().map_err(|_| "Expected two ridges")?;
    if traces[0].points.last().unwrap().t_rec_s >= traces[1].points[0].t_rec_s {
        return Err("Selected acoustic events must be ordered and nonoverlapping".into());
    }
    let dependencies = vec![
        (discovery_path, discovery_receipt.clone()),
        (review_path, review_receipt.clone()),
        (source_path, source_receipt.clone()),
    ];
    for (path, receipt) in &dependencies {
        if sha256_file(path)? != receipt["sha256"] {
            return Err("Reviewed audio inputs changed during reading".into());
        }
    }
    Ok(ReviewedMeasurements {
        traces,
        frequency_sigma_hz: [sigma[0], sigma[1]],
        evidence: json!({"discovery":discovery_receipt,"review_selection":review_receipt,"source":source_receipt,"review_actor":review.review_actor,"review_scope":review.scope,"selected_ridges":selected_evidence}),
        dependencies,
    })
}

/// Prepare a new evidence directory. Any failed attempt leaves failure.json,
/// not completion-looking search/receipt files; existing directories are refused.
pub fn prepare(config_path: &Path, output: &Path) -> Result<Value, String> {
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir(output)
        .map_err(|e| format!("Reviewed search needs a new output directory: {e}"))?;
    let result = prepare_inner(config_path, output);
    if let Err(error) = &result {
        let mut cleanup_errors = Vec::new();
        for name in [
            "search_input.json",
            "receipt.json",
            "event-1.json",
            "event-2.json",
        ] {
            let path = output.join(name);
            if path.exists()
                && let Err(e) = fs::remove_file(&path)
            {
                cleanup_errors.push(format!("{}: {e}", path.display()));
            }
        }
        let _ = write_json(
            &output.join("failure.json"),
            &json!({"status":"failed","location_verified":false,"error":error,"completion_artifact_cleanup_errors":cleanup_errors}),
        );
    }
    result
}

fn prepare_inner(config_path: &Path, output: &Path) -> Result<Value, String> {
    let config_receipt = fingerprint(config_path)?;
    let config: ReviewedSearchConfig = read_json(config_path)?;
    if config.schema_version != 1
        || config.selection_basis.trim().is_empty()
        || config.template_search_input.is_some() == config.search_config.is_some()
    {
        return Err("Reviewed search requires schema 1, selection_basis, and exactly one of template_search_input or search_config".into());
    }
    let base = config_path.parent().unwrap_or(Path::new("."));
    let discovery_path = base.join(&config.discovery);
    let review_path = base.join(&config.review_selection);
    let discovery_receipt = fingerprint(&discovery_path)?;
    let review_receipt = fingerprint(&review_path)?;
    let discovery: Discovery = read_json(&discovery_path)?;
    // Version check first gives legacy users an actionable migration error.
    let review_value: Value = read_json(&review_path)?;
    if review_value["schema_version"] != 2 {
        return Err(
            "Review schema 2 is required; regenerate with review-discovery and review its choices"
                .into(),
        );
    }
    let review: ReviewSelection =
        serde_json::from_value(review_value).map_err(|e| format!("Invalid review: {e}"))?;
    let decisions = validate_review(
        &discovery,
        &review,
        discovery_receipt["sha256"].as_str().unwrap(),
    )?;
    let source_path = config
        .source_override
        .as_ref()
        .map(|p| base.join(p))
        .unwrap_or_else(|| {
            discovery_path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&discovery.source)
        });
    let source_receipt = fingerprint(&source_path)?;
    if source_receipt["sha256"] != discovery.source_sha256 {
        return Err("Source recording hash differs from discovery; use source_override only for an identical relocated file".into());
    }
    if config.selected_ridges[0].harmonic_group_id == config.selected_ridges[1].harmonic_group_id
        || config.selected_ridges[0].event_id == config.selected_ridges[1].event_id
    {
        return Err("The pair requires two distinct reviewed harmonic groups and ridges".into());
    }
    let mut traces = Vec::new();
    let mut evidence = Vec::new();
    for selected in &config.selected_ridges {
        if decisions.get(selected.harmonic_group_id.as_str()) != Some(&"keep_for_matching") {
            return Err("Every selected group must explicitly be keep_for_matching".into());
        }
        if !(1..=10_000).contains(&selected.f0_hz.len())
            || selected.f0_hz.iter().any(|f| !f.is_finite() || *f <= 0.)
        {
            return Err(
                "Each selected ridge requires an explicit finite positive emitted-frequency grid"
                    .into(),
            );
        }
        let event = discovery
            .events
            .iter()
            .find(|e| {
                e.event_id == selected.event_id && e.harmonic_group_id == selected.harmonic_group_id
            })
            .ok_or("Selected event is not in the specified discovery group")?;
        traces.push(EventTrace {
            points: event
                .ridge
                .iter()
                .map(|p| TracePoint {
                    t_rec_s: p.time_s,
                    frequency_hz: p.frequency_hz,
                })
                .collect(),
            f0_hz: selected.f0_hz.clone(),
        });
        let gaps = event.ridge.windows(2).filter(|pair| pair[1].time_s - pair[0].time_s > discovery.settings.max_gap_s)
            .map(|p| json!({"after_time_s":p[0].time_s,"before_time_s":p[1].time_s,"duration_s":p[1].time_s-p[0].time_s})).collect::<Vec<_>>();
        evidence.push(json!({"harmonic_group_id":selected.harmonic_group_id,"event_id":selected.event_id,"review":"keep_for_matching","point_count":event.ridge.len(),"f0_hz":selected.f0_hz,"quality":event.quality,"contrast_weights":event.ridge.iter().map(|p|p.weight).collect::<Vec<_>>(),"preserved_gaps":gaps,"other_group_ridges_not_scored":discovery.events.iter().filter(|e|e.harmonic_group_id == selected.harmonic_group_id && e.event_id != selected.event_id).map(|e|e.event_id.as_str()).collect::<Vec<_>>()}));
    }
    let events: [EventTrace; 2] = traces.try_into().map_err(|_| "Two traces required")?;
    let search_path = base.join(
        config
            .template_search_input
            .as_ref()
            .or(config.search_config.as_ref())
            .unwrap(),
    );
    let search_receipt = fingerprint(&search_path)?;
    let mut dependencies = vec![
        (config_path.to_path_buf(), config_receipt.clone()),
        (discovery_path, discovery_receipt.clone()),
        (review_path, review_receipt.clone()),
        (source_path, source_receipt.clone()),
        (search_path.clone(), search_receipt.clone()),
    ];
    let mut input = if config.template_search_input.is_some() {
        let mut template: SearchInput = read_json(&search_path)?;
        template.events = events;
        template
    } else {
        let geometry: search_setup::SearchSetup = read_json(&search_path)?;
        let search_base = search_path.parent().unwrap_or(Path::new("."));
        for path in
            std::iter::once(&geometry.normalized_tracks).chain(geometry.observers_file.iter())
        {
            let path = search_base.join(path);
            dependencies.push((path.clone(), fingerprint(&path)?));
        }
        search_setup::prepare_with_events(&search_path, events)?
    };
    input.provenance.push(format!("Reviewed discovery SHA256 {}; source SHA256 {}; channel {}; review actor {}; selection basis: {}. Original template event traces and frequencies are replaced by the explicitly selected measured ridges and emitted-frequency grids.",discovery_receipt["sha256"],discovery.source_sha256,discovery.source_metadata.selected_channel_index,review.review_actor,config.selection_basis));
    input.provenance.push("No sample interpolation, gap filling, harmonic rescaling, contrast weighting or clock/CPA derivation. Timing, geometry and flight candidate data remain supplied assumptions. One ridge per group is scored; other harmonics are not independent flights and are not checked by this pair score. RMSE uses equal samples within each event and equal event weights. Discovery quality/uncertainty is retained in the preparation receipt, not modeled as calibrated likelihood.".into());
    validate_search_input(&input)?;
    for (path, before) in &dependencies {
        if sha256_file(path)? != before["sha256"] {
            return Err(format!(
                "Input changed during preparation: {}",
                path.display()
            ));
        }
    }
    for (i, event) in input.events.iter().enumerate() {
        write_json(&output.join(format!("event-{}.json", i + 1)), event)?;
    }
    write_json(&output.join("search_input.json"), &input)?;
    let receipt = json!({"schema_version":1,"status":"prepared_reviewed_search","location_verified":false,"aircraft_identity_confirmed":false,"review_actor":review.review_actor,"review_scope":review.scope,"selection_basis":config.selection_basis,"provenance":{"config":config_receipt,"discovery":discovery_receipt,"review_selection":review_receipt,"source":source_receipt,"search_basis":search_receipt,"search_basis_kind":if config.template_search_input.is_some(){"template_search_input"}else{"search_config"},"dependencies":dependencies.iter().map(|(_,r)|r).collect::<Vec<_>>()},"selected_ridges":evidence,"search_input":fingerprint(&output.join("search_input.json"))?,"event_files":[fingerprint(&output.join("event-1.json"))?,fingerprint(&output.join("event-2.json"))?],"track_count":input.tracks.len(),"observer_count":input.observers.len(),"measurement_transform":"Exact time/frequency copying, no interpolation, no harmonic rescaling","limitations":["The selected tones are acoustic proposals; reviewed does not mean confirmed aircraft","No automatic CPA/time anchor or emitted-frequency grid inference; supplied priors are unchanged","Only one explicit ridge per group is scored; other harmonics remain untested","Contrast weights and discovery uncertainty are retained but not used by the equal-event RMSE matcher","Partial observed ridges can give misleadingly low residuals and do not establish complete-pass support","Search returns unverified regional hypotheses, not a calibrated location probability"]});
    write_json(&output.join("receipt.json"), &receipt)?;
    Ok(receipt)
}
