//! A self-contained, analyst-readable dossier of an explicitly bounded search.
//! It preserves the original review and full machine-readable report. Exporting
//! evidence never promotes an unverified hypothesis to a verified location.
use crate::{
    physics::geodetic_to_ecef,
    regional_search::{self, ModalityReport, ModeStatus, RegionalReport, SearchMode, SearchStatus},
    workspace::{EventKind, ReviewState, ReviewWorkspace, parse_utc},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt::Write as _,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportArtifact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportReceipt {
    pub schema_version: u32,
    pub status: String,
    pub location_verified: bool,
    pub document_sha256: String,
    pub inputs_sha256: String,
    pub artifacts: Vec<ExportArtifact>,
}

pub(crate) fn joint_export_status(
    snapshot: &crate::investigation_archive::JointSnapshot,
) -> &'static str {
    match snapshot.report.status {
        crate::joint_evidence::JointStatus::Completed => "completed_joint_envelope_exported",
        crate::joint_evidence::JointStatus::Cancelled => "cancelled_joint_envelope_exported",
    }
}

fn joint_state_labels(cell: &crate::joint_evidence::JointCell) -> (String, String) {
    use crate::joint_evidence::{DopplerState, EnvelopeState};
    let doppler = match &cell.doppler {
        DopplerState::NotStarted => "Not started".into(),
        DopplerState::Unavailable { reason } => format!("Unavailable: {reason}"),
        DopplerState::Available { score, .. } => format!("{score:.9}"),
    };
    let envelope = match &cell.envelope {
        EnvelopeState::NotStarted => "Not started".into(),
        EnvelopeState::Unavailable { reason } => format!("Unavailable: {reason}"),
        EnvelopeState::Available {
            cosine_similarity, ..
        } => format!("{cosine_similarity:.9}"),
    };
    (doppler, envelope)
}

/// Human-readable shared-cell report. The full cell inventory is in CSV/JSON;
/// the HTML preview is explicitly bounded and never presented as a shortlist.
pub fn render_joint_html(
    document: &ReviewWorkspace,
    snapshot: &crate::investigation_archive::JointSnapshot,
) -> Result<String, String> {
    crate::investigation_archive::validate_joint_snapshot(document, snapshot)?;
    let report = &snapshot.report;
    let observation = &snapshot.request.observation;
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Shared-cell Doppler and thunder-envelope comparison</title><style>body{font:16px system-ui;max-width:1200px;margin:2em auto;padding:0 1em;background:#f4f5f6;color:#20252a}table{border-collapse:collapse;width:100%}td,th{padding:.4em;border:1px solid #aaa;text-align:left}code{overflow-wrap:anywhere}.warning{background:#fff0cd;padding:1em}</style><h1>Shared-cell Doppler and thunder-envelope comparison</h1><p class=\"warning\">Experimental ordering; location is unverified. Completed computation does not mean every score is available or the proposed sources are correct. The ordinal combination is not a probability. Picked thunder timing and Doppler timing are not additional votes.</p>",
    );
    write!(html, "<p>Execution: <strong>{:?}</strong>. Ranking available: <strong>{}</strong>. All cells available: {}. {} receivers × {} clocks = {} cells.</p><p>Historical case: {}. Recording SHA-256: <code>{}</code>. Channel {}.</p><p>Request SHA-256: <code>{}</code><br>Source inputs SHA-256: <code>{}</code></p>", report.status, report.ranking_available, report.all_cells_available, report.receivers.len(), report.clock_offsets_s.len(), report.cells.len(), escaped(&document.case_id), escaped(&document.recording_sha256), document.channel, escaped(&report.request_sha256), escaped(&report.inputs_sha256)).unwrap();
    write!(html, "<h2>Frozen acoustic evidence and assumptions</h2><p>Measured envelope: {} center samples, actual band {:.6}–{:.6} Hz. Review note: {}</p><p>Sound speed {:.3} m/s; source-height fallback {:?} m; energy decay {:.3} s, duration {:.3} s. Heights and response settings are assumptions. Original provider timestamps and source interpretation remain in the saved request.</p>", observation.envelope.samples.len(), observation.envelope.band_low_hz, observation.envelope.band_high_hz, escaped(&observation.reviewed_selection_note), snapshot.request.envelope_plan.model.sound_speed_mps, snapshot.request.envelope_plan.model.fallback_strike_ellipsoid_height_m, snapshot.request.envelope_plan.model.kernel_decay_s, snapshot.request.envelope_plan.model.kernel_duration_s).unwrap();
    html.push_str("<p>This self-contained result includes its reviewed workspace, observation, exact plans, receiver grid and normalized source inputs. No original recording is copied. Opening this report does not load external paths or run anything. Replay requires explicit verification of original evidence.</p><p><a href=\"joint-envelope.json\">Complete frozen request and result</a> · <a href=\"joint-cells.csv\">Every receiver-clock cell</a> · <a href=\"workspace.json\">Historical review</a></p><h2>Limitations</h2><ul>");
    for limitation in &report.limitations {
        write!(html, "<li>{}</li>", escaped(limitation)).unwrap();
    }
    html.push_str("</ul><h2>Cell inventory preview</h2><p>First 200 cells in original inventory order, not a ranked shortlist. All ties, unavailable cells and unstarted work are retained in the full CSV/JSON.</p><table><thead><tr><th>Receiver / clock</th><th>Coordinates</th><th>Doppler residual</th><th>Envelope cosine</th><th>Combined ordinal cost</th></tr></thead><tbody>");
    for cell in report.cells.iter().take(200) {
        let receiver = &report.receivers[cell.receiver_index];
        let (doppler, envelope) = joint_state_labels(cell);
        write!(
            html,
            "<tr><td>{} / {:.6} s</td><td>{:.8}, {:.8}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            escaped(&receiver.id),
            cell.clock_offset_s,
            receiver.location.latitude_deg,
            receiver.location.longitude_deg,
            escaped(&doppler),
            escaped(&envelope),
            cell.ranks
                .as_ref()
                .map(|r| format!("{:.3}", r.combined))
                .unwrap_or_else(|| "Unavailable".into())
        )
        .unwrap();
    }
    html.push_str("</tbody></table></html>");
    Ok(html)
}

fn joint_cells_csv(
    snapshot: &crate::investigation_archive::JointSnapshot,
) -> Result<String, String> {
    use crate::joint_evidence::{DopplerState, EnvelopeState};
    let mut csv = String::from(
        "receiver_index,receiver_id,latitude_deg,longitude_deg,ellipsoid_height_m,clock_index,clock_offset_s,doppler_state,doppler_reason,doppler_score,doppler_sources,envelope_state,envelope_reason,envelope_cosine,doppler_midrank,envelope_midrank,combined_ordinal_cost,spatial_boundary,clock_boundary,execution_status,location_verified\r\n",
    );
    for cell in &snapshot.report.cells {
        let receiver = &snapshot.report.receivers[cell.receiver_index];
        let (ds, dr) = match &cell.doppler {
            DopplerState::NotStarted => ("not_started", ""),
            DopplerState::Available { .. } => ("available", ""),
            DopplerState::Unavailable { reason } => ("unavailable", reason.as_str()),
        };
        let (es, er) = match &cell.envelope {
            EnvelopeState::NotStarted => ("not_started", ""),
            EnvelopeState::Available { .. } => ("available", ""),
            EnvelopeState::Unavailable { reason } => ("unavailable", reason.as_str()),
        };
        let (score, sources) = match &cell.doppler {
            DopplerState::Available {
                score, assignments, ..
            } => (
                score.to_string(),
                assignments
                    .iter()
                    .map(|a| {
                        format!(
                            "{} -> {} ({:?} Hz)",
                            a.event_id, a.source_id, a.emitted_frequency_hz
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            _ => (String::new(), String::new()),
        };
        let cosine = match &cell.envelope {
            EnvelopeState::Available {
                cosine_similarity, ..
            } => cosine_similarity.to_string(),
            _ => String::new(),
        };
        let ranks = cell.ranks.as_ref();
        writeln!(
            csv,
            "{},{},{:.10},{:.10},{:.6},{},{:.9},{},{},{},{},{},{},{},{},{},{},{},{},{:?},false\r",
            cell.receiver_index,
            csv_text(&receiver.id),
            receiver.location.latitude_deg,
            receiver.location.longitude_deg,
            receiver.location.ellipsoid_height_m,
            cell.clock_index,
            cell.clock_offset_s,
            ds,
            csv_text(dr),
            score,
            csv_text(&sources),
            es,
            csv_text(er),
            cosine,
            ranks.map(|r| r.doppler.to_string()).unwrap_or_default(),
            ranks.map(|r| r.envelope.to_string()).unwrap_or_default(),
            ranks.map(|r| r.combined.to_string()).unwrap_or_default(),
            cell.spatial_at_boundary,
            cell.clock_at_boundary,
            snapshot.report.status
        )
        .unwrap();
        if csv.len() as u64 > crate::investigation_archive::MAX_REPORT_BYTES {
            return Err("Joint CSV exceeds the bounded report size".into());
        }
    }
    Ok(csv)
}

fn bounded_report_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    struct Buffer(Vec<u8>);
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if (self.0.len() as u64).saturating_add(bytes.len() as u64)
                > crate::investigation_archive::MAX_REPORT_BYTES
            {
                return Err(std::io::Error::other(
                    "Joint export exceeds the report byte limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Buffer(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|e| e.to_string())?;
    Ok(output.0)
}

/// Always exports the original frozen snapshot, independently of current edits
/// or missing original source files. The destination must not already exist.
pub fn export_joint(
    directory: &Path,
    document: &ReviewWorkspace,
    snapshot: &crate::investigation_archive::JointSnapshot,
) -> Result<ExportReceipt, String> {
    let html = render_joint_html(document, snapshot)?;
    let workspace = bounded_report_json(document)?;
    let frozen = bounded_report_json(snapshot)?;
    let csv = joint_cells_csv(snapshot)?;
    let files = [
        ("index.html", html.as_bytes()),
        ("workspace.json", workspace.as_slice()),
        ("joint-envelope.json", frozen.as_slice()),
        ("joint-cells.csv", csv.as_bytes()),
    ];
    check_export_sizes(&files)?;
    let destination = fresh_directory(directory)?;
    let marker = destination.join("INCOMPLETE.txt");
    write_new(
        &marker,
        b"Joint export incomplete; retain partial artifacts for diagnosis.\n",
    )?;
    let mut receipt = ExportReceipt {
        schema_version: 1,
        status: joint_export_status(snapshot).into(),
        location_verified: false,
        document_sha256: snapshot.report.document_sha256.clone(),
        inputs_sha256: snapshot.report.inputs_sha256.clone(),
        artifacts: Vec::new(),
    };
    for (name, bytes) in files {
        write_new(&destination.join(name), bytes)?;
        receipt.artifacts.push(ExportArtifact {
            path: name.into(),
            bytes: bytes.len() as u64,
            sha256: fingerprint(bytes),
        });
    }
    write_new(
        &destination.join("receipt.json"),
        &bounded_report_json(&receipt)?,
    )?;
    fs::remove_file(marker).map_err(|e| e.to_string())?;
    Ok(receipt)
}

fn escaped(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#39;"),
            _ => result.push(c),
        }
    }
    result
}
fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn source_assessments_html(document: &ReviewWorkspace) -> String {
    let mut html = String::from(
        "<section><h2>Acoustic source assessments</h2><p>Measurement confirmation and source-class assessment are separate. Compatible means the sound could belong to the chosen class; it does not identify a particular aircraft or lightning strike. Legacy unassessed events remain unassessed.</p>",
    );
    for warning in document.acoustic_source_warnings() {
        write!(
            html,
            "<p class=\"status\"><strong>{}</strong></p>",
            escaped(&warning)
        )
        .unwrap();
    }
    html.push_str("<table><thead><tr><th>Active reviewed event</th><th>Sound class</th><th>Source assessment</th><th>Explicit rationale</th></tr></thead><tbody>");
    for event in document.active_scoring_events() {
        write!(
            html,
            "<tr><td><code>{}</code> · {}</td><td>{:?}</td><td>{}</td><td>{}</td></tr>",
            escaped(&event.id),
            escaped(&event.label),
            event.kind,
            escaped(event.acoustic_source.label()),
            escaped(event.acoustic_source.rationale().unwrap_or("—"))
        )
        .unwrap();
    }
    html.push_str("</tbody></table></section>");
    if let Some(policy) = &document.mix.lightning_source_policy {
        write!(html, "<section><h2>Declared lightning source interpretation</h2><p>Source bytes SHA-256: <code>{}</code>. Thunder timing {}.</p><p>ETRS89-as-WGS84 approximation rationale: {}. No datum transformation is applied.</p><p>Declared timestamp representation allowance: {:.6} s. Thunder model residual scale: {:.6} s. The allowance is an analyst-selected heuristic, not a calibrated Gaussian sigma or a verified source clock error; observed pick uncertainty and recording-clock assumptions remain separate. FMI ellipse-major units remain unresolved and are not scored.</p></section>", escaped(&policy.catalog_sha256), if document.mix.thunder_timing { "enabled" } else { "disabled; saved interpretation is inactive" }, escaped(&policy.etrs89_as_wgs84_rationale), policy.time_representation_allowance_s, document.mix.thunder_scale_s).unwrap();
    }
    html
}
fn close(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9_f64.max(a.abs().max(b.abs()) * 1e-10)
}

pub(crate) fn validate(document: &ReviewWorkspace, report: &RegionalReport) -> Result<(), String> {
    document.validate()?;
    report.plan.validate()?;
    if report.schema_version != 1
        || report.location_verified
        || regional_search::document_sha256(document)? != report.document_sha256
    {
        return Err("The dossier needs the original unmodified review snapshot and an unverified regional report".into());
    }
    for hash in [
        &report.document_sha256,
        &report.inputs_sha256,
        &report.observers_sha256,
    ] {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err("Report input identities must be lowercase SHA-256 values".into());
        }
    }
    let clocks = report.plan.clock.values()?;
    let sources: usize = report
        .plan
        .events
        .iter()
        .map(|event| event.source_ids.len())
        .sum();
    let cells = (report.estimate.observer_count as u64)
        .checked_mul(clocks.len() as u64)
        .ok_or("Report work count overflowed")?;
    let evaluations = cells
        .checked_mul(sources as u64)
        .ok_or("Report work count overflowed")?;
    if report.estimate.observer_count == 0
        || report.estimate.observer_count > crate::workspace_scoring::MAX_OBSERVERS
        || evaluations > report.plan.max_evaluations
        || report.estimate.clock_count != clocks.len()
        || report.estimate.event_count != report.plan.events.len()
        || report.estimate.source_alternatives != sources
        || report.estimate.evaluations != evaluations
        || report.progress.total_cells != cells
        || report.progress.total_evaluations != evaluations
        || report.progress.completed_cells > cells
        || report.progress.completed_evaluations > evaluations
        || report.rankable_cells.checked_add(report.unavailable_cells)
            != Some(report.progress.completed_cells)
        || report.unavailable_count < report.unavailable.len() as u64
        || report.unavailable_count > report.progress.completed_evaluations
        || report.hypotheses.len() > report.plan.retain_hypotheses
    {
        return Err("Regional report coverage counts are inconsistent".into());
    }
    if report.status == SearchStatus::Completed
        && (report.progress.completed_cells != cells
            || report.progress.completed_evaluations != evaluations)
    {
        return Err("A completed report cannot hide unfinished search work".into());
    }
    if report
        .supported_hypotheses
        .is_some_and(|n| n < report.hypotheses.len() as u64)
        || report.retained_truncated
            != report
                .supported_hypotheses
                .is_none_or(|n| n > report.hypotheses.len() as u64)
        || (report.rankable_cells > 0 && report.hypotheses.is_empty())
    {
        return Err("Retained shortlist counts are inconsistent".into());
    }
    let selected: Vec<_> = document
        .events
        .iter()
        .filter(|e| {
            e.review == ReviewState::Confirmed
                && e.selected_for_scoring
                && match e.kind {
                    EventKind::Doppler => {
                        document.mix.doppler_frequency || document.mix.doppler_timing
                    }
                    EventKind::Thunder => document.mix.thunder_timing,
                }
        })
        .collect();
    if selected.len() != report.plan.events.len()
        || report
            .plan
            .events
            .iter()
            .any(|p| !selected.iter().any(|event| event.id == p.event_id))
    {
        return Err("Search plan differs from the reviewed event set".into());
    }
    if !report.location_summaries.is_empty() {
        if report.location_summaries.len() != report.estimate.observer_count {
            return Err("Geographic summaries omit declared receivers".into());
        }
        let mut completed = 0u64;
        let mut rankable = 0u64;
        let mut unavailable = 0u64;
        for (index, row) in report.location_summaries.iter().enumerate() {
            if row.observer_index != index
                || row.completed_clocks > clocks.len()
                || row.rankable_clocks.checked_add(row.unavailable_clocks)
                    != Some(row.completed_clocks)
                || (report.status == SearchStatus::Completed
                    && row.completed_clocks != clocks.len())
            {
                return Err("Geographic clock coverage is inconsistent".into());
            }
            geodetic_to_ecef(
                row.observer.latitude_deg,
                row.observer.longitude_deg,
                row.observer.ellipsoid_height_m,
            )?;
            let [south, west, north, east] = document.map.bounds;
            if row.observer.latitude_deg < south
                || row.observer.latitude_deg > north
                || row.observer.longitude_deg < west
                || row.observer.longitude_deg > east
            {
                return Err("Geographic summary lies outside the declared region".into());
            }
            let has_best = row.rankable_clocks > 0;
            if has_best
                != (row.best_score.is_some()
                    && row.best_clock_offset_s.is_some()
                    && row.best_hypothesis_id.is_some()
                    && !row.best_assignments.is_empty())
                || (!has_best
                    && (row.best_score.is_some()
                        || row.best_clock_offset_s.is_some()
                        || row.best_hypothesis_id.is_some()
                        || !row.best_assignments.is_empty()))
                || row.best_score.is_some_and(|v| !v.is_finite() || v < 0.0)
                || row
                    .best_clock_offset_s
                    .is_some_and(|v| !clocks.contains(&v))
            {
                return Err("Geographic best-hypothesis summary is inconsistent".into());
            }
            if has_best {
                if row.best_assignments.len() != selected.len() {
                    return Err("Geographic summary omits selected events".into());
                }
                let mut assigned = BTreeSet::new();
                for assignment in &row.best_assignments {
                    let approved = report
                        .plan
                        .events
                        .iter()
                        .find(|event| event.event_id == assignment.event_id)
                        .ok_or("Geographic event was not approved")?;
                    let event = selected
                        .iter()
                        .find(|event| event.id == assignment.event_id)
                        .ok_or("Geographic event was not selected")?;
                    if !assigned.insert(&assignment.event_id)
                        || assignment.kind != event.kind
                        || !approved.source_ids.contains(&assignment.source_id)
                    {
                        return Err("Geographic summary includes unapproved source choices".into());
                    }
                    if event.kind == EventKind::Doppler && document.mix.doppler_frequency {
                        let [lo, hi] = approved
                            .emitted_frequency_hz
                            .ok_or("Missing geographic tone bounds")?;
                        if assignment
                            .emitted_frequency_hz
                            .is_none_or(|v| !v.is_finite() || v < lo || v > hi)
                        {
                            return Err("Geographic tone lies outside its approved bounds".into());
                        }
                    } else if assignment.emitted_frequency_hz.is_some() {
                        return Err("Disabled geographic frequency term has a fitted tone".into());
                    }
                }
            }
            completed += row.completed_clocks as u64;
            rankable += row.rankable_clocks as u64;
            unavailable += row.unavailable_clocks as u64;
        }
        if completed != report.progress.completed_cells
            || rankable != report.rankable_cells
            || unavailable != report.unavailable_cells
        {
            return Err("Geographic summaries disagree with search coverage".into());
        }
    }
    let base = parse_utc(&document.recording_start_utc)?;
    if clocks
        .iter()
        .any(|offset| !(0.0..253_402_300_800.0).contains(&(base + offset)))
    {
        return Err("Declared recording-start range is outside supported Unix UTC".into());
    }
    let mut ids = BTreeSet::new();
    let mut previous: Option<(f64, &str)> = None;
    for (index, hypothesis) in report.hypotheses.iter().enumerate() {
        if hypothesis.rank != index + 1
            || !ids.insert(hypothesis.id.as_str())
            || !hypothesis.score.is_finite()
            || hypothesis.score < 0.0
            || !clocks.contains(&hypothesis.clock_offset_s)
            || !hypothesis.recording_start_utc_s.is_finite()
            || (hypothesis.recording_start_utc_s - (base + hypothesis.clock_offset_s)).abs() > 1e-6
            || hypothesis.assignments.len() != selected.len()
        {
            return Err(
                "A retained hypothesis has invalid rank, clock or selected evidence".into(),
            );
        }
        if let Some((score, id)) = previous
            && (hypothesis.score.total_cmp(&score).is_lt()
                || (hypothesis.score == score && hypothesis.id.as_str() < id))
        {
            return Err("Hypotheses are not in their declared ranking order".into());
        }
        previous = Some((hypothesis.score, &hypothesis.id));
        geodetic_to_ecef(
            hypothesis.observer.latitude_deg,
            hypothesis.observer.longitude_deg,
            hypothesis.observer.ellipsoid_height_m,
        )?;
        let [south, west, north, east] = document.map.bounds;
        if hypothesis.observer.latitude_deg < south
            || hypothesis.observer.latitude_deg > north
            || hypothesis.observer.longitude_deg < west
            || hypothesis.observer.longitude_deg > east
        {
            return Err("A retained receiver lies outside the declared region".into());
        }
        let mut assigned = BTreeSet::new();
        let mut total = 0.0;
        for assignment in &hypothesis.assignments {
            let approved = report
                .plan
                .events
                .iter()
                .find(|e| e.event_id == assignment.event_id)
                .ok_or("Unknown assignment event")?;
            let event = selected
                .iter()
                .find(|e| e.id == assignment.event_id)
                .ok_or("Unselected assignment event")?;
            if !assigned.insert(&assignment.event_id)
                || event.kind != assignment.kind
                || !approved.source_ids.contains(&assignment.source_id)
                || !assignment.weighted_contribution.is_finite()
                || assignment.weighted_contribution < 0.0
            {
                return Err("Assignment is not an approved finite hypothesis".into());
            }
            if event.kind == EventKind::Doppler && document.mix.doppler_frequency {
                let tone = assignment
                    .emitted_frequency_hz
                    .ok_or("Missing fitted tone")?;
                let [lo, hi] = approved
                    .emitted_frequency_hz
                    .ok_or("Missing approved tone bounds")?;
                if !tone.is_finite() || tone < lo || tone > hi {
                    return Err("Hypothesis tone is outside its approved bounds".into());
                }
            } else if assignment.emitted_frequency_hz.is_some() {
                return Err("Disabled frequency evidence has a fitted tone".into());
            }
            let term_total = assignment.terms.iter().try_fold(0.0, |sum, term| {
                let value = term
                    .weighted_contribution
                    .ok_or("Missing term contribution")?;
                if !term.available
                    || !value.is_finite()
                    || value < 0.0
                    || term
                        .mean_normalized_squared_residual
                        .is_none_or(|v| !v.is_finite() || v < 0.0)
                {
                    return Err("Unavailable or invalid assignment term");
                }
                Ok(sum + value)
            })?;
            if !close(term_total, assignment.weighted_contribution) {
                return Err("Assignment contributions do not add up".into());
            }
            total += assignment.weighted_contribution;
        }
        if !close(total, hypothesis.score) {
            return Err("Hypothesis score differs from its recorded contributions".into());
        }
    }
    Ok(())
}

fn utc_label(seconds: f64) -> String {
    let mut whole = seconds.floor() as u64;
    let mut millis = ((seconds - whole as f64) * 1000.0).round() as u64;
    if millis == 1000 {
        whole += 1;
        millis = 0;
    }
    let mut days = whole / 86400;
    let mut year = 1970u64;
    let leap = |year: u64| {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    };
    while days >= if leap(year) { 366 } else { 365 } {
        days -= if leap(year) { 366 } else { 365 };
        year += 1;
    }
    let lengths = [
        31,
        if leap(year) { 29 } else { 28 },
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
    let mut month = 0;
    while days >= lengths[month] {
        days -= lengths[month];
        month += 1;
    }
    let time = whole % 86400;
    format!(
        "{year:04}-{:02}-{:02} {:02}:{:02}:{:02}.{millis:03} UTC",
        month + 1,
        days + 1,
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

pub fn render_html(document: &ReviewWorkspace, report: &RegionalReport) -> Result<String, String> {
    validate(document, report)?;
    let partial = report.status == SearchStatus::Cancelled;
    let heading = if partial {
        "PARTIAL — search cancelled"
    } else if report.hypotheses.is_empty() {
        "Completed search — no rankable hypotheses"
    } else {
        "Completed regional search — unverified shortlist"
    };
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><title>GeoSonnar investigation dossier</title><style>body{font:15px/1.55 system-ui,sans-serif;background:#f2f3f5;color:#202630;margin:0}main{max-width:1250px;margin:auto;padding:28px}h1,h2{line-height:1.2}section{background:white;border:1px solid #c7cdd5;border-radius:5px;padding:18px;margin:18px 0}.status{background:#fff2ce;border-left:5px solid #a56600;padding:14px}.scroll{overflow:auto}table{border-collapse:collapse;width:100%;font-size:13px}th,td{border:1px solid #ccd1d7;padding:7px;text-align:left;vertical-align:top}th{background:#e8ebef}code{overflow-wrap:anywhere}pre{white-space:pre-wrap;overflow-wrap:anywhere}.muted{color:#535f6d}.number{white-space:nowrap}.boundary{color:#895000;font-weight:bold}summary{cursor:pointer;font-weight:bold}a{color:#1758a5}@media print{body{background:white}main{padding:0}section{break-inside:avoid}.scroll{overflow:visible}table{font-size:10px}}</style></head><body><main><h1>GeoSonnar investigation dossier</h1>",
    );
    write!(html,"<p class=\"status\"><strong>{heading}</strong><br>Location verified: <strong>NO</strong>. Scores are uncalibrated residuals, not probabilities. This document records what was searched and retained; it does not authenticate the source recording or establish a location.</p><p>Case: <strong>{}</strong> · Audio channel {} · Recording duration {:.3} s</p>",escaped(&document.case_id),document.channel+1,document.duration_s).unwrap();
    html.push_str(&source_assessments_html(document));
    html.push_str("<section><h2>Search coverage and status</h2>");
    write!(html,"<p>Completed receiver/time cells: <strong>{}/{}</strong>. Rankable: {}; unavailable: {}. Completed event/source/receiver/time evaluations: <strong>{}/{}</strong>.</p>",report.progress.completed_cells,report.progress.total_cells,report.rankable_cells,report.unavailable_cells,report.progress.completed_evaluations,report.progress.total_evaluations).unwrap();
    let supported = report
        .supported_hypotheses
        .map(|n| n.to_string())
        .unwrap_or_else(|| "more than the 64-bit count can represent".into());
    write!(html,"<p>Retained <strong>{}</strong> hypotheses from <strong>{}</strong> supported joint assignments in fully evaluated cells. Retained-list truncation: <strong>{}</strong>. When truncation is reported, some supported hypotheses are omitted from the retained list. Modality-specific global best locations cannot be inferred merely from the retained joint ranking.</p>",report.hypotheses.len(),supported,if report.retained_truncated{"YES"}else{"NO"}).unwrap();
    if partial {
        html.push_str("<p class=\"status\">This is an incomplete search. Other planned locations, times or source combinations remain unevaluated. A partial shortlist must not be described as the final winner of the declared search. Incomplete cells contribute no ranked hypothesis.</p>");
    }
    if report.hypotheses.is_empty() {
        html.push_str("<p>No complete supported hypothesis was retained. This can result from missing trajectory/strike support; it does not prove that the event did not occur in this region.</p>");
    }
    write!(html,"<p>Unavailable event/source evaluations: {}. Examples retained: {}. Declared evaluation budget: {}. Estimated residual calculations: {}; trajectory-point visits: {}.</p></section>",report.unavailable_count,report.unavailable.len(),report.plan.max_evaluations,report.estimate.residual_evaluations,report.estimate.trajectory_point_visits).unwrap();
    let base = parse_utc(&document.recording_start_utc)?;
    let [south, west, north, east] = document.map.bounds;
    write!(html,"<section><h2>Declared region, clock and propagation</h2><p>Region (south, west, north, east): <code>[{south:.8}, {west:.8}, {north:.8}, {east:.8}]</code>. Requested grid spacing: {:.3} m. Actual receiver set: {} points, SHA-256 <code>{}</code>.</p><p>Recording-start reference: <code>{}</code>. Shared absolute adjustments relative to that reference: [{:.6}, {:.6}] s, step {:.6} s (both range endpoints included). This replaces the review's fixed clock adjustment; it is not added twice.</p><p>Searched recording starts: <strong>{}</strong> through <strong>{}</strong>. Clock samples: {}.</p>",document.map.grid_spacing_m,report.estimate.observer_count,escaped(&report.observers_sha256),escaped(&document.recording_start_utc),report.plan.clock.min_offset_s,report.plan.clock.max_offset_s,report.plan.clock.step_s,utc_label(base+report.plan.clock.min_offset_s),utc_label(base+report.plan.clock.max_offset_s),report.estimate.clock_count).unwrap();
    if let Some(profile) = &document.atmosphere {
        write!(html,"<p>Atmosphere: <strong>{}</strong>; provider: {}; {} levels. Frozen vertical profile, straight propagation paths; no refraction or automatic weather validation.</p>",escaped(&profile.name),escaped(&profile.provenance.provider),profile.levels.len()).unwrap();
    } else {
        write!(
            html,
            "<p>Fixed sound speed: {:.4} m/s; straight propagation paths.</p>",
            document.sound_speed_mps
        )
        .unwrap();
    }
    write!(html,"<p>Review receiver ellipsoid height: {:.3} m. Each retained row reports its actual searched height. A terrain configuration, when selected, is preserved in workspace.json; this dossier does not re-sample rasters or verify their availability. Strike fallback ellipsoid height: {}.</p></section>",document.map.receiver_height_m,document.strike_fallback_height_m.map(|v|format!("{v:.3} m (declared assumption)")).unwrap_or_else(||"none".into())).unwrap();
    html.push_str("<section><h2>Reviewed observations and approved alternatives</h2><div class=\"scroll\"><table><thead><tr><th>Event</th><th>Review / observations</th><th>Approved sources</th><th>Emitted tone</th></tr></thead><tbody>");
    for choice in &report.plan.events {
        let event = document
            .events
            .iter()
            .find(|e| e.id == choice.event_id)
            .expect("validated event");
        let frequency = choice
            .emitted_frequency_hz
            .map(|[lo, hi]| {
                if lo == hi {
                    format!("Fixed {lo:.6} Hz")
                } else {
                    format!("Analytic fit within [{lo:.6}, {hi:.6}] Hz")
                }
            })
            .unwrap_or_else(|| "not selected".into());
        write!(html,"<tr><td><strong>{}</strong><br><code>{}</code> · {:?}</td><td>Confirmed and selected<br>Interval [{:.6}, {:.6}] s; reviewed arrival {:.6} s<br>{} frequency points; timing uncertainty {:.6} s; frequency uncertainty {:.6} Hz</td><td>{}</td><td>{}</td></tr>",escaped(&event.label),escaped(&event.id),event.kind,event.start_s,event.end_s,event.arrival_s,event.points.len(),event.uncertainty_s,event.frequency_sigma_hz,choice.source_ids.iter().map(|s|format!("<code>{}</code>",escaped(s))).collect::<Vec<_>>().join("<br>"),frequency).unwrap();
    }
    html.push_str("</tbody></table></div><p>Alternatives are analyst-approved hypotheses, not identified sound sources. Each event shares one receiver and clock. Multiple events may refer to one source; correlated harmonics or arrivals are not independent confirmations.</p><p>Selected terms and weights:</p><ul>");
    for (name, enabled, weight, scale, unit) in [
        (
            "Doppler frequency",
            document.mix.doppler_frequency,
            document.mix.frequency_weight,
            document.mix.frequency_scale_hz,
            "Hz",
        ),
        (
            "Doppler timing",
            document.mix.doppler_timing,
            document.mix.doppler_timing_weight,
            document.mix.doppler_timing_scale_s,
            "s",
        ),
        (
            "Thunder timing",
            document.mix.thunder_timing,
            document.mix.thunder_weight,
            document.mix.thunder_scale_s,
            "s",
        ),
    ] {
        write!(
            html,
            "<li>{name}: {}; weight {weight:.6}; residual scale {scale:.6} {unit}</li>",
            if enabled { "selected" } else { "disabled" }
        )
        .unwrap();
    }
    html.push_str("</ul><p>Samples are averaged within each event, then events within each selected term. Terms use their declared weights. Fitting tones and comparing additional associations changes search flexibility; scores are not calibrated across arbitrary plans.</p></section>");
    html.push_str("<section><h2>Retained competing hypotheses</h2><div class=\"scroll\"><table><thead><tr><th>Rank / score</th><th>Coordinates / height</th><th>Recording start / shared adjustment</th><th>Associations / tone</th><th>Boundary flags</th></tr></thead><tbody>");
    for h in &report.hypotheses {
        let associations = h
            .assignments
            .iter()
            .map(|a| {
                format!(
                    "<code>{}</code> → <code>{}</code>{}",
                    escaped(&a.event_id),
                    escaped(&a.source_id),
                    a.emitted_frequency_hz
                        .map(|v| format!(" · {v:.6} Hz"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("<br>");
        let flags = [
            ("region", h.spatial_at_boundary),
            ("clock", h.clock_at_boundary),
            ("tone", h.frequency_at_boundary),
        ]
        .into_iter()
        .filter_map(|(name, active)| active.then_some(name))
        .collect::<Vec<_>>()
        .join(", ");
        write!(html,"<tr><td class=\"number\">{}<br>{:.8e}</td><td class=\"number\">{:.7}, {:.7}<br>{:.3} m ellipsoid</td><td>{}<br>{:+.6} s</td><td>{}</td><td class=\"boundary\">{}</td></tr>",h.rank,h.score,h.observer.latitude_deg,h.observer.longitude_deg,h.observer.ellipsoid_height_m,utc_label(h.recording_start_utc_s),h.clock_offset_s,associations,if flags.is_empty(){"none"}else{&flags}).unwrap();
    }
    html.push_str("</tbody></table></div><p>Displayed coordinate precision is numerical formatting, not demonstrated location accuracy. Boundary flags prompt review of whether the declared limits exclude alternatives.</p></section><section><h2>Recorded contributions</h2><p>These are compact summaries. Full point residuals are reproduced by explicitly applying a hypothesis and running the fixed comparison with the original inputs.</p>");
    for h in &report.hypotheses {
        write!(html,"<details><summary>Rank {} · score {:.8e}</summary><p>Hypothesis ID: <code>{}</code></p><table><thead><tr><th>Event / source</th><th>Term</th><th>Event mean normalized squared residual</th><th>Joint weighted contribution</th></tr></thead><tbody>",h.rank,h.score,escaped(&h.id)).unwrap();
        for a in &h.assignments {
            for t in &a.terms {
                write!(
                    html,
                    "<tr><td>{}<br>{}</td><td>{:?}</td><td>{:.8e}</td><td>{:.8e}</td></tr>",
                    escaped(&a.event_id),
                    escaped(&a.source_id),
                    t.term,
                    t.mean_normalized_squared_residual.unwrap_or(f64::NAN),
                    t.weighted_contribution.unwrap_or(f64::NAN)
                )
                .unwrap();
            }
        }
        html.push_str("</tbody></table></details>");
    }
    html.push_str("</section><section><h2>Geographic ambiguity across all declared receivers</h2><p>Each row keeps the best supported assignment after searching this receiver's declared clock grid. These rows are independent of the globally retained top-K list. Different rows may use different clocks and source associations; they are possibilities, not a confidence region.</p>");
    if report.location_summaries.is_empty() {
        html.push_str("<p>This older report has no complete geographic summaries; do not infer a full location surface from its retained shortlist.</p>");
    } else {
        html.push_str("<div class=\"scroll\"><table><thead><tr><th>Receiver / coordinates</th><th>Best score / clock</th><th>Completed / planned clocks</th><th>Rankable / unavailable clocks</th><th>Sources</th></tr></thead><tbody>");
        for row in &report.location_summaries {
            let score = row
                .best_score
                .map(|v| format!("{v:.8e}"))
                .unwrap_or_else(|| "unavailable".into());
            let clock = row
                .best_clock_offset_s
                .map(|v| utc_label(base + v))
                .unwrap_or_else(|| "none".into());
            let sources = row
                .best_assignments
                .iter()
                .map(|a| {
                    format!(
                        "{} → {}{}",
                        escaped(&a.event_id),
                        escaped(&a.source_id),
                        a.emitted_frequency_hz
                            .map(|v| format!(" ({v:.6} Hz)"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("<br>");
            write!(html,"<tr><td>{}<br>{:.7}, {:.7}<br>{:.3} m ellipsoid</td><td>{}<br>{}</td><td>{}/{}</td><td>{}/{}</td><td>{}</td></tr>",row.observer_index,row.observer.latitude_deg,row.observer.longitude_deg,row.observer.ellipsoid_height_m,score,clock,row.completed_clocks,report.estimate.clock_count,row.rankable_clocks,row.unavailable_clocks,sources).unwrap();
        }
        html.push_str("</tbody></table></div>");
    }
    html.push_str("</section><section><h2>Unavailable support and limitations</h2><ul>");
    for limitation in &report.limitations {
        write!(html, "<li>{}</li>", escaped(limitation)).unwrap();
    }
    html.push_str("</ul><details><summary>Retained unavailable examples</summary><ul>");
    for u in &report.unavailable {
        write!(html,"<li>Event <code>{}</code>, source <code>{}</code>, receiver index {}, clock {:+.6} s: {}</li>",escaped(&u.event_id),escaped(&u.source_id),u.observer_index,u.clock_offset_s,escaped(&u.reasons.join("; "))).unwrap();
    }
    html.push_str("</ul></details></section><section><h2>Source identity and analyst notes</h2>");
    write!(html,"<p>Recording SHA-256: <code>{}</code><br>Original review SHA-256: <code>{}</code><br>Loaded track/lightning buffers SHA-256: <code>{}</code></p><p>These identify bytes and declared inputs. A matching hash does not authenticate the recording, verify provider completeness or establish that a hypothesis is correct.</p><table><thead><tr><th>Bound data path</th><th>SHA-256</th></tr></thead><tbody>",escaped(&document.recording_sha256),escaped(&report.document_sha256),escaped(&report.inputs_sha256)).unwrap();
    for dataset in &document.datasets {
        write!(
            html,
            "<tr><td><code>{}</code></td><td><code>{}</code></td></tr>",
            escaped(&dataset.path.to_string_lossy()),
            escaped(&dataset.sha256)
        )
        .unwrap();
    }
    write!(html,"</tbody></table><h3>Notes preserved from the original review</h3><pre>{}</pre><p class=\"muted\">This local dossier contains case notes and may contain private file paths. It embeds no external assets and sends no network requests. workspace.json and regional-report.json preserve the original review and complete stored report; hypotheses.csv provides the retained rows. receipt.json fingerprints the exported files. Raw recordings and provider datasets are separate inputs and are not redistributed by this export.</p></section></main></body></html>",escaped(&document.notes)).unwrap();
    Ok(html)
}

fn csv_text(value: &str) -> String {
    // Prevent spreadsheet formula execution for user/provider-controlled text.
    let risky = value.starts_with(['=', '+', '-', '@', '\t', '\r', '\n'])
        || value.trim_start().starts_with(['=', '+', '-', '@']);
    let value = if risky {
        format!("'{value}")
    } else {
        value.into()
    };
    format!("\"{}\"", value.replace('"', "\"\""))
}
fn csv(report: &RegionalReport) -> String {
    let mut result = String::from(
        "rank,score,latitude_deg,longitude_deg,ellipsoid_height_m,clock_offset_s,recording_start_utc,hypothesis_id,assignments,spatial_boundary,clock_boundary,tone_boundary,search_status,location_verified\r\n",
    );
    for h in &report.hypotheses {
        let assignments = h
            .assignments
            .iter()
            .map(|a| {
                format!(
                    "{} -> {}{}",
                    a.event_id,
                    a.source_id,
                    a.emitted_frequency_hz
                        .map(|v| format!(" ({v:.9} Hz)"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        writeln!(
            result,
            "{},{:.17e},{:.10},{:.10},{:.6},{:.9},{},{},{},{},{},{},{},false\r",
            h.rank,
            h.score,
            h.observer.latitude_deg,
            h.observer.longitude_deg,
            h.observer.ellipsoid_height_m,
            h.clock_offset_s,
            csv_text(&utc_label(h.recording_start_utc_s)),
            csv_text(&h.id),
            csv_text(&assignments),
            h.spatial_at_boundary,
            h.clock_at_boundary,
            h.frequency_at_boundary,
            csv_text(if report.status == SearchStatus::Completed {
                "completed"
            } else {
                "cancelled_partial"
            })
        )
        .unwrap();
    }
    result
}

fn locations_csv(report: &RegionalReport) -> String {
    let mut csv = String::from(
        "observer_index,latitude_deg,longitude_deg,ellipsoid_height_m,best_score,best_clock_offset_s,completed_clocks,planned_clocks,rankable_clocks,unavailable_clocks,assignments,search_status,location_verified\r\n",
    );
    for row in &report.location_summaries {
        let sources = row
            .best_assignments
            .iter()
            .map(|a| {
                format!(
                    "{} -> {}{}",
                    a.event_id,
                    a.source_id,
                    a.emitted_frequency_hz
                        .map(|v| format!(" ({v:.9} Hz)"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        writeln!(
            csv,
            "{},{:.10},{:.10},{:.6},{},{},{},{},{},{},{},{},false\r",
            row.observer_index,
            row.observer.latitude_deg,
            row.observer.longitude_deg,
            row.observer.ellipsoid_height_m,
            row.best_score
                .map(|v| format!("{v:.17e}"))
                .unwrap_or_default(),
            row.best_clock_offset_s
                .map(|v| format!("{v:.9}"))
                .unwrap_or_default(),
            row.completed_clocks,
            report.estimate.clock_count,
            row.rankable_clocks,
            row.unavailable_clocks,
            csv_text(&sources),
            csv_text(if report.status == SearchStatus::Completed {
                "completed"
            } else {
                "cancelled_partial"
            })
        )
        .unwrap();
    }
    csv
}

fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    metadata.file_type().is_symlink()
}
fn fresh_directory(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    if absolute.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err("Export destination must be a new direct path without parent traversal".into());
    }
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if linked(&meta) {
                    return Err(
                        "Export paths may not traverse symbolic links or Windows reparse points"
                            .into(),
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    if fs::symlink_metadata(&absolute).is_ok() {
        return Err(
            "Choose a new dossier directory; existing folders are never overwritten".into(),
        );
    }
    fs::create_dir(&absolute).map_err(|e| format!("Create dossier directory: {e}"))?;
    Ok(absolute)
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

/// Export into a new directory under an existing parent. A failure keeps an
/// INCOMPLETE.txt marker and never overwrites an earlier dossier.
pub fn export(
    directory: &Path,
    document: &ReviewWorkspace,
    report: &RegionalReport,
) -> Result<ExportReceipt, String> {
    export_internal(directory, document, report, None)
}
pub fn export_with_replay_inputs(
    directory: &Path,
    document: &ReviewWorkspace,
    report: &RegionalReport,
    replay: &crate::investigation_archive::ReplayInputs,
) -> Result<ExportReceipt, String> {
    export_internal(directory, document, report, Some(replay))
}
fn export_internal(
    directory: &Path,
    document: &ReviewWorkspace,
    report: &RegionalReport,
    replay: Option<&crate::investigation_archive::ReplayInputs>,
) -> Result<ExportReceipt, String> {
    let html = render_html(document, report)?;
    let replay = replay
        .map(|inputs| {
            crate::investigation_archive::validate_replay_inputs(
                document,
                inputs,
                &report.inputs_sha256,
            )?;
            crate::investigation_archive::replay_bytes(inputs)
        })
        .transpose()?;
    let workspace = serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?;
    let regional = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
    let csv = csv(report);
    let geographic = locations_csv(report);
    let mut files = vec![
        ("index.html", html.as_bytes()),
        ("hypotheses.csv", csv.as_bytes()),
        ("locations.csv", geographic.as_bytes()),
        ("workspace.json", workspace.as_slice()),
        ("regional-report.json", regional.as_slice()),
    ];
    if let Some(bytes) = &replay {
        files.push(("replay-inputs.json", bytes.as_slice()));
    }
    check_export_sizes(&files)?;
    let destination = fresh_directory(directory)?;
    let marker = destination.join("INCOMPLETE.txt");
    write_new(&marker,b"Export incomplete. Keep partial artifacts for diagnosis; do not treat this folder as a completed dossier.\n")?;
    let mut receipt = ExportReceipt {
        schema_version: 1,
        status: if report.status == SearchStatus::Completed {
            "completed_search_exported"
        } else {
            "partial_search_exported"
        }
        .into(),
        location_verified: false,
        document_sha256: report.document_sha256.clone(),
        inputs_sha256: report.inputs_sha256.clone(),
        artifacts: Vec::new(),
    };
    for (name, bytes) in files {
        write_new(&destination.join(name), bytes)?;
        receipt.artifacts.push(ExportArtifact {
            path: name.into(),
            bytes: bytes.len() as u64,
            sha256: fingerprint(bytes),
        });
    }
    write_new(
        &destination.join("receipt.json"),
        &serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )?;
    fs::remove_file(marker).map_err(|e| e.to_string())?;
    Ok(receipt)
}

fn check_export_sizes(files: &[(&str, &[u8])]) -> Result<(), String> {
    use crate::investigation_archive::{MAX_ARCHIVE_BYTES, MAX_REPLAY_BYTES, MAX_REPORT_BYTES};
    let mut total = 0u64;
    for (name, bytes) in files {
        let size = bytes.len() as u64;
        total = total.checked_add(size).ok_or("Dossier size overflow")?;
        let limit = if *name == "replay-inputs.json" {
            MAX_REPLAY_BYTES
        } else {
            MAX_REPORT_BYTES
        };
        if size > limit || total > MAX_ARCHIVE_BYTES {
            return Err("Dossier exceeds its bounded report, replay or aggregate size".into());
        }
    }
    Ok(())
}

fn mode_name(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::DopplerOnly => "Doppler only",
        SearchMode::ThunderOnly => "Thunder only",
        SearchMode::Combined => "Combined",
    }
}
fn mode_directory(mode: SearchMode) -> &'static str {
    match mode {
        SearchMode::DopplerOnly => "doppler-only",
        SearchMode::ThunderOnly => "thunder-only",
        SearchMode::Combined => "combined",
    }
}
pub(crate) fn validate_comparison(
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
) -> Result<(), String> {
    document.validate()?;
    if comparison.schema_version != 1
        || comparison.location_verified
        || regional_search::document_sha256(document)? != comparison.document_sha256
    {
        return Err("Comparison dossier needs its unchanged original review snapshot".into());
    }
    if comparison.request.modes.is_empty()
        || comparison.request.modes.len() > 3
        || comparison
            .request
            .modes
            .iter()
            .enumerate()
            .any(|(i, mode)| comparison.request.modes[..i].contains(mode))
        || comparison.modes.len() != comparison.request.modes.len()
        || comparison.estimate.modes.len() != comparison.request.modes.len()
        || comparison.progress.total_modes != comparison.request.modes.len()
        || comparison.request.max_total_evaluations == 0
        || comparison.request.max_total_evaluations > 3 * crate::regional_search::MAX_EVALUATIONS
    {
        return Err("Comparison mode inventory is inconsistent".into());
    }
    let mut completed = 0;
    let mut evaluated = 0u64;
    let mut total = 0u64;
    let mut observer_identity = None;
    for (index, entry) in comparison.modes.iter().enumerate() {
        if entry.mode != comparison.request.modes[index]
            || entry.mode != comparison.estimate.modes[index].mode
        {
            return Err("Comparison mode order differs from its request".into());
        }
        total = total
            .checked_add(comparison.estimate.modes[index].estimate.evaluations)
            .ok_or("Comparison total overflowed")?;
        let derived =
            regional_search::mode_document(document, &comparison.request.regional, entry.mode)?;
        let estimate = &comparison.estimate.modes[index].estimate;
        let plan = derived
            .regional_plan
            .as_ref()
            .ok_or("Mode has no derived plan")?;
        let clock_count = plan.clock.values()?.len();
        let alternatives = plan
            .events
            .iter()
            .map(|event| event.source_ids.len())
            .sum::<usize>();
        let evaluations = (estimate.observer_count as u64)
            .checked_mul(clock_count as u64)
            .and_then(|value| value.checked_mul(alternatives as u64))
            .ok_or("Mode estimate overflow")?;
        if estimate.observer_count == 0
            || estimate.observer_count > crate::workspace_scoring::MAX_OBSERVERS
            || estimate.observer_count != comparison.estimate.modes[0].estimate.observer_count
            || estimate.clock_count != clock_count
            || estimate.event_count != plan.events.len()
            || estimate.source_alternatives != alternatives
            || estimate.evaluations != evaluations
            || evaluations > plan.max_evaluations
        {
            return Err(
                "Mode estimate differs from its declared work, including unstarted modes".into(),
            );
        }
        match (&entry.report, entry.status) {
            (None, ModeStatus::NotStarted) => {}
            (Some(report), ModeStatus::Completed | ModeStatus::Cancelled) => {
                validate(&derived, report)?;
                if (entry.status == ModeStatus::Completed)
                    != (report.status == SearchStatus::Completed)
                    || report.inputs_sha256 != comparison.inputs_sha256
                    || Some(&report.plan) != derived.regional_plan.as_ref()
                    || serde_json::to_value(&report.estimate).map_err(|e| e.to_string())?
                        != serde_json::to_value(&comparison.estimate.modes[index].estimate)
                            .map_err(|e| e.to_string())?
                {
                    return Err("Mode report does not match its declared independent search".into());
                }
                if observer_identity.is_some_and(|id| id != report.observers_sha256.as_str()) {
                    return Err("Scoring modes used different receiver sets".into());
                }
                observer_identity = Some(report.observers_sha256.as_str());
                evaluated = evaluated
                    .checked_add(report.progress.completed_evaluations)
                    .ok_or("Comparison progress overflowed")?;
                if entry.status == ModeStatus::Completed {
                    completed += 1;
                }
            }
            _ => return Err("Mode status and retained report disagree".into()),
        }
    }
    if total != comparison.estimate.evaluations
        || total != comparison.progress.total_evaluations
        || total > comparison.request.max_total_evaluations
        || evaluated != comparison.progress.completed_evaluations
        || completed != comparison.progress.completed_modes
        || (comparison.status == SearchStatus::Completed
            && completed != comparison.request.modes.len())
    {
        return Err("Overall comparison hides incomplete work or inconsistent totals".into());
    }
    Ok(())
}

pub fn render_modalities_html(
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
) -> Result<String, String> {
    validate_comparison(document, comparison)?;
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'\"><title>GeoSonnar independent evidence comparison</title><style>body{font:15px/1.5 system-ui,sans-serif;background:#f2f3f5;color:#202630;margin:0}main{max-width:1150px;margin:auto;padding:28px}section{background:white;border:1px solid #bbc4ce;padding:18px;margin:18px 0}.status{background:#fff0c4;padding:15px;border-left:5px solid #a46500}.scroll{overflow:auto}table{width:100%;border-collapse:collapse}th,td{border:1px solid #ccd1d7;padding:8px;text-align:left;vertical-align:top}code,pre{overflow-wrap:anywhere}pre{white-space:pre-wrap}a{color:#1758a5}</style></head><body><main><h1>Independent Doppler / thunder comparison</h1>",
    );
    let heading = if comparison.status == SearchStatus::Completed {
        "Completed independent mode comparison"
    } else {
        "PARTIAL — comparison cancelled; some requested work is unfinished"
    };
    // The top-level review can retain enabled but unrequested modalities.
    // Only requested modes supply assumptions to this comparison.
    let mut assessment_document = document.clone();
    if !comparison
        .request
        .modes
        .iter()
        .any(|mode| matches!(mode, SearchMode::DopplerOnly | SearchMode::Combined))
    {
        assessment_document.mix.doppler_frequency = false;
        assessment_document.mix.doppler_timing = false;
    }
    if !comparison
        .request
        .modes
        .iter()
        .any(|mode| matches!(mode, SearchMode::ThunderOnly | SearchMode::Combined))
    {
        assessment_document.mix.thunder_timing = false;
    }
    html.push_str(&source_assessments_html(&assessment_document));
    write!(html,"<p class=\"status\"><strong>{heading}</strong><br>Location verified: <strong>NO</strong>. This compares independently searched evidence modes with identical regional/time priors. Scores are uncalibrated and differently normalized across modes.</p><p>Case: <strong>{}</strong>. Completed modes: {}/{}. Event/source/receiver/time evaluations: {}/{}.</p><section><h2>Mode-specific results</h2><div class=\"scroll\"><table><thead><tr><th>Mode</th><th>Status / work</th><th>Best supported location</th><th>Score / clock</th><th>Details</th></tr></thead><tbody>",escaped(&document.case_id),comparison.progress.completed_modes,comparison.progress.total_modes,comparison.progress.completed_evaluations,comparison.progress.total_evaluations).unwrap();
    for entry in &comparison.modes {
        let (status, work) = match entry.status {
            ModeStatus::NotStarted => ("NOT STARTED".to_owned(), "0 evaluations".to_owned()),
            ModeStatus::Completed => (
                "completed".to_owned(),
                entry
                    .report
                    .as_ref()
                    .map(|r| {
                        format!(
                            "{}/{} cells",
                            r.progress.completed_cells, r.progress.total_cells
                        )
                    })
                    .unwrap_or_default(),
            ),
            ModeStatus::Cancelled => (
                "PARTIAL; best so far only".to_owned(),
                entry
                    .report
                    .as_ref()
                    .map(|r| {
                        format!(
                            "{}/{} cells",
                            r.progress.completed_cells, r.progress.total_cells
                        )
                    })
                    .unwrap_or_default(),
            ),
        };
        let best = entry.report.as_ref().and_then(|r| {
            r.location_summaries
                .iter()
                .filter(|row| row.best_score.is_some())
                .min_by(|a, b| {
                    a.best_score
                        .unwrap()
                        .total_cmp(&b.best_score.unwrap())
                        .then_with(|| a.observer_index.cmp(&b.observer_index))
                })
        });
        let location = best
            .map(|row| {
                format!(
                    "{:.7}, {:.7}",
                    row.observer.latitude_deg, row.observer.longitude_deg
                )
            })
            .unwrap_or_else(|| {
                if entry.status == ModeStatus::NotStarted {
                    "not evaluated".into()
                } else {
                    "no geographic summary / no supported receiver".into()
                }
            });
        let score = best
            .map(|row| {
                format!(
                    "{:.8e}<br>{:+.6} s",
                    row.best_score.unwrap(),
                    row.best_clock_offset_s.unwrap()
                )
            })
            .unwrap_or_else(|| "unavailable".into());
        let details = if entry.report.is_some() {
            format!(
                "<a href=\"{}/index.html\">Full {} dossier</a>",
                mode_directory(entry.mode),
                mode_name(entry.mode)
            )
        } else {
            "No report: this mode was not started".into()
        };
        write!(
            html,
            "<tr><td>{}</td><td>{}<br>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            mode_name(entry.mode),
            status,
            work,
            location,
            score,
            details
        )
        .unwrap();
    }
    html.push_str("</tbody></table></div><p>Each mode searches its own approved associations and shared clock independently. Geographic rows cover every declared receiver, even when the global joint-hypothesis list is crowded by many alternatives at one location. A better score in one mode is not a likelihood ratio against another mode.</p>");
    let best_for = |mode| {
        comparison
            .modes
            .iter()
            .find(|entry| entry.mode == mode && entry.status == ModeStatus::Completed)
            .and_then(|entry| entry.report.as_ref())
            .and_then(|r| {
                r.location_summaries
                    .iter()
                    .filter(|row| row.best_score.is_some())
                    .min_by(|a, b| {
                        a.best_score
                            .unwrap()
                            .total_cmp(&b.best_score.unwrap())
                            .then_with(|| a.observer_index.cmp(&b.observer_index))
                    })
            })
    };
    if let (Some(d), Some(t)) = (
        best_for(SearchMode::DopplerOnly),
        best_for(SearchMode::ThunderOnly),
    ) {
        let distance =
            crate::modality_diagnostics::horizontal_separation_m(&d.observer, &t.observer);
        write!(html,"<p><strong>Independent Doppler–thunder peak separation: {distance:.3} m.</strong> The preferred clocks differ by {:.6} s. Different best cells can still have overlapping broad low-score areas. Spatial overlap after independent clock fitting does not establish a common recording time. Only best clocks are summarized, so other shared low-score clocks cannot be ruled out from this display. Inspect the combined shared-clock residuals. Peak separation alone is not an acceptance or rejection rule.</p>",(d.best_clock_offset_s.unwrap()-t.best_clock_offset_s.unwrap()).abs()).unwrap();
    } else {
        html.push_str("<p>Doppler–thunder peak agreement is not established: both modes must finish with supported geographic summaries.</p>");
    }
    html.push_str("</section><section><h2>Shared scope and limitations</h2><ul>");
    for limitation in &comparison.limitations {
        write!(html, "<li>{}</li>", escaped(limitation)).unwrap();
    }
    write!(html,"</ul><p>Original review SHA-256: <code>{}</code><br>Loaded input buffers SHA-256: <code>{}</code></p><p>The same observed traces, receiver list, clock bounds and approved sources within each modality were used. Only irrelevant modality terms and plan rows were disabled. Original weights and scales are retained. Mode-specific dossiers preserve the exact derived review and report; the original combined review and full comparison are at the top level.</p><h3>Original analyst notes</h3><pre>{}</pre><p>This local export contains case notes and possibly private paths. It includes no external assets, raw recordings or provider datasets. Unstarted modes remain unstarted in comparison.json; completed subreports do not convert an incomplete overall comparison into a completed one.</p></section></main></body></html>",escaped(&comparison.document_sha256),escaped(&comparison.inputs_sha256),escaped(&document.notes)).unwrap();
    Ok(html)
}

pub fn export_modalities(
    directory: &Path,
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
) -> Result<ExportReceipt, String> {
    export_modalities_internal(directory, document, comparison, None)
}
pub fn export_modalities_with_replay_inputs(
    directory: &Path,
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
    replay: &crate::investigation_archive::ReplayInputs,
) -> Result<ExportReceipt, String> {
    export_modalities_internal(directory, document, comparison, Some(replay))
}
fn export_modalities_internal(
    directory: &Path,
    document: &ReviewWorkspace,
    comparison: &ModalityReport,
    replay: Option<&crate::investigation_archive::ReplayInputs>,
) -> Result<ExportReceipt, String> {
    let html = render_modalities_html(document, comparison)?;
    let replay = replay
        .map(|inputs| {
            crate::investigation_archive::validate_replay_inputs(
                document,
                inputs,
                &comparison.inputs_sha256,
            )?;
            crate::investigation_archive::replay_bytes(inputs)
        })
        .transpose()?;
    let workspace_bytes = serde_json::to_vec_pretty(document).map_err(|e| e.to_string())?;
    let comparison_bytes = serde_json::to_vec_pretty(comparison).map_err(|e| e.to_string())?;
    let mut root_files = vec![
        ("index.html", html.as_bytes()),
        ("workspace.json", workspace_bytes.as_slice()),
        ("comparison.json", comparison_bytes.as_slice()),
    ];
    if let Some(bytes) = &replay {
        root_files.push(("replay-inputs.json", bytes.as_slice()));
    }
    check_export_sizes(&root_files)?;
    let destination = fresh_directory(directory)?;
    let marker = destination.join("INCOMPLETE.txt");
    write_new(
        &marker,
        b"Incomplete comparison export. Do not treat partial files as a complete dossier.\n",
    )?;
    for (name, bytes) in root_files {
        write_new(&destination.join(name), bytes)?;
    }
    for entry in &comparison.modes {
        if let Some(report) = &entry.report {
            let derived =
                regional_search::mode_document(document, &comparison.request.regional, entry.mode)?;
            export(
                &destination.join(mode_directory(entry.mode)),
                &derived,
                report,
            )?;
        }
    }
    let mut paths = Vec::new();
    let mut directories = vec![destination.clone()];
    while let Some(folder) = directories.pop() {
        for item in fs::read_dir(folder).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if linked(&metadata) {
                return Err("Comparison export unexpectedly contains a reparse point".into());
            }
            if metadata.is_dir() {
                directories.push(path);
            } else if path != marker {
                paths.push(path);
            }
        }
    }
    paths.sort();
    let mut artifacts = Vec::new();
    let mut total_bytes = 0u64;
    for path in paths {
        let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
        total_bytes = total_bytes
            .checked_add(size)
            .ok_or("Dossier byte count overflow")?;
        let cap = if path
            .file_name()
            .is_some_and(|name| name == "replay-inputs.json")
        {
            crate::investigation_archive::MAX_REPLAY_BYTES
        } else {
            crate::investigation_archive::MAX_REPORT_BYTES
        };
        if size > cap || total_bytes > crate::investigation_archive::MAX_ARCHIVE_BYTES {
            return Err("Comparison export exceeds its bounded aggregate size".into());
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        artifacts.push(ExportArtifact {
            path: path
                .strip_prefix(&destination)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/"),
            bytes: bytes.len() as u64,
            sha256: fingerprint(&bytes),
        });
    }
    let receipt = ExportReceipt {
        schema_version: 1,
        status: if comparison.status == SearchStatus::Completed {
            "completed_comparison_exported"
        } else {
            "partial_comparison_exported"
        }
        .into(),
        location_verified: false,
        document_sha256: comparison.document_sha256.clone(),
        inputs_sha256: comparison.inputs_sha256.clone(),
        artifacts,
    };
    write_new(
        &destination.join("receipt.json"),
        &serde_json::to_vec_pretty(&receipt).map_err(|e| e.to_string())?,
    )?;
    fs::remove_file(marker).map_err(|e| e.to_string())?;
    Ok(receipt)
}
