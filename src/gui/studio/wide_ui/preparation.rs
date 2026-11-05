//! Wide-area search preparation.
use super::*;

pub(super) fn utc_input(seconds: f64) -> String {
    regional::utc_label(seconds).chars().take(19).collect()
}

pub(super) fn wide_checkpoint(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Wide-area work cancelled".into())
    } else {
        Ok(())
    }
}

pub(super) fn samples(bounds: [f64; 3], positive: bool) -> Result<Vec<f64>, String> {
    let [low, high, step] = bounds;
    if !bounds.iter().all(|v| v.is_finite()) || low > high || step <= 0. || positive && low <= 0. {
        return Err("Grid ranges need finite ordered bounds and a positive step".into());
    }
    let steps = ((high - low) / step).floor();
    if !steps.is_finite() || steps >= 1001. {
        return Err("A tone/shared-shift grid is limited to 1,001 values".into());
    }
    let count = steps as usize + 1;
    Ok((0..count).map(|i| low + i as f64 * step).collect())
}
pub(super) fn reviewed_pair(doc: &ReviewWorkspace) -> Result<[&EvidenceEvent; 2], String> {
    let mut selected: Vec<_> = doc
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Doppler && e.selected_for_scoring)
        .collect();
    if selected.len() != 2 {
        return Err("Select exactly two Doppler events in Audio review for this method".into());
    }
    selected.sort_by(|a, b| a.start_s.total_cmp(&b.start_s));
    for event in &selected {
        if event.review != ReviewState::Confirmed || event.points.len() < 3 {
            return Err(format!(
                "{} needs confirmed measurements and at least three trace points",
                event.label
            ));
        }
        if let Some(reason) = event.acoustic_source_block_reason() {
            return Err(reason);
        }
    }
    if selected[0].end_s >= selected[1].start_s {
        return Err("The two reviewed event windows must be ordered and nonoverlapping".into());
    }
    Ok([selected[0], selected[1]])
}
pub(super) fn copy_reviewed_pair(
    doc: &ReviewWorkspace,
    settings: &mut Settings,
) -> Result<(), String> {
    let pair = reviewed_pair(doc)?;
    for (i, event) in pair.into_iter().enumerate() {
        settings.event_ids[i] = event.id.clone();
        settings.cpa_s[i] = event.arrival_s;
        let low = event
            .points
            .iter()
            .map(|p| p.frequency_hz)
            .fold(f64::INFINITY, f64::min);
        let high = event
            .points
            .iter()
            .map(|p| p.frequency_hz)
            .fold(0., f64::max);
        settings.tones[i] = [(low * 0.9).max(0.1), high * 1.1, 1.];
    }
    // Copying measurements never changes the analyst's physical limits.
    Ok(())
}
pub(super) fn request(
    doc: &ReviewWorkspace,
    source: &IndexedSource,
    settings: &Settings,
) -> Result<WidePairRequest, String> {
    if !doc.datasets.iter().any(|b| {
        b.path == source.binding.manifest_path && b.sha256 == source.binding.manifest_sha256
    }) {
        return Err("Select the indexed source for this review again".into());
    }
    let events = reviewed_pair(doc)?;
    if settings.event_ids != [events[0].id.clone(), events[1].id.clone()] {
        return Err(
            "Copy the current reviewed pair into this plan, then inspect its timing and tone grids"
                .into(),
        );
    }
    for i in 0..2 {
        for (name, [low, high]) in [
            ("speed", settings.speed[i]),
            ("slant distance", settings.distance[i]),
        ] {
            if !low.is_finite() || !high.is_finite() || low < 0. || low > high {
                return Err(format!(
                    "Event {} needs finite, nonnegative, ordered {name} bounds",
                    i + 1
                ));
            }
        }
    }
    if settings
        .cpa_s
        .iter()
        .any(|v| !v.is_finite() || *v < 0. || *v > doc.duration_s)
        || settings.cpa_s[0] >= settings.cpa_s[1]
    {
        return Err("Review two ordered CPA anchors inside the recording".into());
    }
    let start = workspace::parse_utc(&settings.utc_start)? - source.origin;
    let end = workspace::parse_utc(&settings.utc_end)? - source.origin;
    if start >= end {
        return Err("Recording-start UTC bounds must be ordered".into());
    }
    let event = |i: usize| -> Result<geosonnar::physics::EventTrace, String> {
        Ok(geosonnar::physics::EventTrace {
            points: events[i]
                .points
                .iter()
                .map(|p| geosonnar::physics::TracePoint {
                    t_rec_s: p.time_s,
                    frequency_hz: p.frequency_hz,
                })
                .collect(),
            f0_hz: samples(settings.tones[i], true)?,
        })
    };
    Ok(WidePairRequest {
        schema_version: 1,
        document_sha256: geosonnar::regional_search::document_sha256(doc)?,
        recording_sha256: doc.recording_sha256.clone(),
        channel: doc.channel,
        grid: GridLayout::new(&doc.map)?,
        source: source.binding.clone(),
        events: [event(0)?, event(1)?],
        pair_config: geosonnar::physics::PairConfig {
            event1_cpa_recording_s: settings.cpa_s[0],
            shared_shifts_s: samples(settings.shift, false)?,
            clock_offset_bounds_s: Some([start, end]),
            model: geosonnar::physics::ModelConfig {
                sound_speed_mps: doc.sound_speed_mps,
                max_gap_s: source.max_gap_s,
            },
        },
        coarse_bounds: std::array::from_fn(|i| geosonnar::verification::CoarseBounds {
            speed_mps: settings.speed[i],
            slant_distance_m: settings.distance[i],
        }),
        measured_separation_s: settings.cpa_s[1] - settings.cpa_s[0],
        separation_tolerance_s: settings.separation_tolerance_s,
        budgets: WideBudgets {
            work_limit_enabled: settings.work_limit_enabled,
            max_work_units: settings.max_work,
            max_output_bytes: settings.max_output,
            max_duration_s: settings.max_seconds,
            max_index_memory_bytes: settings.max_index_memory_bytes,
        },
        retain: settings.retain,
    })
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewSnapshot {
    schema_version: u32,
    request_sha256: String,
    document: ReviewWorkspace,
}
pub(super) fn ordinary_result_parents(path: &std::path::Path) -> Result<(), String> {
    for parent in path.ancestors() {
        if parent.as_os_str().is_empty() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err("Wide-area output parents must be ordinary directories".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Wide-area output parents cannot be reparse points".into());
            }
        }
    }
    Ok(())
}
pub(super) fn save_review_snapshot(
    output: &std::path::Path,
    request: &WidePairRequest,
    document: &ReviewWorkspace,
) -> Result<(), String> {
    use std::io::Write;
    let snapshot = ReviewSnapshot {
        schema_version: 1,
        request_sha256: wide_area_pair::request_sha256(request)?,
        document: document.clone(),
    };
    let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Wide-area review snapshot exceeds 8 MiB; nothing was scored".into());
    }
    ordinary_result_parents(output.parent().ok_or("Missing output parent")?)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.with_extension("review.json"))
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
pub(super) fn load_saved_wide(path: &std::path::Path) -> Result<WideReply, String> {
    use std::io::Read;
    let report = wide_area_pair::load_report(path)?;
    let sidecar = path.with_extension("review.json");
    let document = if sidecar.try_exists().map_err(|e| e.to_string())? {
        let metadata = std::fs::symlink_metadata(&sidecar).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("Saved review snapshot is not an ordinary file".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Saved review snapshot is a reparse point".into());
            }
        }
        let mut bytes = Vec::new();
        std::fs::File::open(&sidecar)
            .map_err(|e| e.to_string())?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("Saved review snapshot exceeds its bound".into());
        }
        let snapshot: ReviewSnapshot = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let mut request_bytes = Vec::new();
        std::fs::File::open(path.join("request.json"))
            .map_err(|e| e.to_string())?
            .take(wide_area_pair::MAX_REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut request_bytes)
            .map_err(|e| e.to_string())?;
        if request_bytes.len() > wide_area_pair::MAX_REQUEST_BYTES {
            return Err("Saved request exceeds its bound".into());
        }
        let request: WidePairRequest =
            serde_json::from_slice(&request_bytes).map_err(|e| e.to_string())?;
        snapshot.document.validate()?;
        if snapshot.schema_version != 1
            || snapshot.request_sha256 != report.request_sha256
            || wide_area_pair::request_sha256(&request)? != report.request_sha256
            || geosonnar::regional_search::document_sha256(&snapshot.document)?
                != request.document_sha256
        {
            return Err("Saved review/request identity differs".into());
        }
        Some(Box::new(snapshot.document))
    } else {
        None
    };
    Ok(WideReply::ReopenedReport(Box::new(report), document))
}
