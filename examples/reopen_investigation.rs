//! Headless persistence regression for an explicitly supplied fixed hypothesis.
//! It does not search for a better fit or assert a correct location/source class.
use geosonnar::{
    casework,
    detailed_data::sha256,
    evidence_score::{self, LightningCatalog, ObserverLocation},
    investigation_archive::{self as archive, ReportKind},
    investigation_report,
    map_data::{self, MapData},
    prediction_overlay::{self, OverlaySettings, PredictionOverlay},
    regional_search::{self, ClockRange, EventAlternatives, RegionalPlan, RegionalReport},
    workspace::{self, EventKind, ReviewWorkspace},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    document: ReviewWorkspace,
    map: MapData,
    lightning: Option<LightningCatalog>,
    observer: ObserverLocation,
    settings: OverlaySettings,
}
fn write_json(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Replay result exceeds 64 MiB".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn resolve(manifest: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest.parent().unwrap_or(Path::new(".")).join(path)
    }
}
fn build(
    request: &Request,
    doc: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
) -> Result<PredictionOverlay, String> {
    prediction_overlay::build(
        doc,
        map,
        lightning,
        doc.strike_fallback_height_m,
        &request.observer,
        &request.settings,
    )
}
fn physical(overlay: &PredictionOverlay) -> Value {
    json!({"frequency":overlay.frequency,"timing":overlay.timing,"unavailable":overlay.unavailable,
        "frequency_sample_count":overlay.frequency_sample_count,"sampling_coarsened":overlay.sampling_coarsened,
        "observer":overlay.observer,"clock_adjustment_s":overlay.clock_adjustment_s,
        "adjusted_recording_start_utc_s":overlay.adjusted_recording_start_utc_s,"model_label":overlay.model_label,
        "candidates":overlay.score_report.candidates})
}
fn ensure_equal(a: &PredictionOverlay, b: &PredictionOverlay, stage: &str) -> Result<(), String> {
    if physical(a) != physical(b) {
        return Err(format!(
            "Physical predictions or measured residuals changed at {stage}"
        ));
    }
    Ok(())
}
fn fixed_plan(document: &ReviewWorkspace) -> RegionalPlan {
    RegionalPlan {
        schema_version: 1,
        clock: ClockRange {
            min_offset_s: document.map.clock_adjustment_s,
            max_offset_s: document.map.clock_adjustment_s,
            step_s: 1.0,
        },
        events: document
            .active_scoring_events()
            .map(|event| EventAlternatives {
                event_id: event.id.clone(),
                source_ids: vec![match event.kind {
                    EventKind::Doppler => event.flight_id.clone(),
                    EventKind::Thunder => event.strike_id.clone(),
                }],
                emitted_frequency_hz: (event.kind == EventKind::Doppler
                    && document.mix.doppler_frequency)
                    .then_some([event.emitted_frequency_hz; 2]),
            })
            .collect(),
        max_evaluations: 32,
        retain_hypotheses: 1,
    }
}
fn make_report(
    request: &Request,
    doc: &ReviewWorkspace,
    map: &MapData,
    lightning: &Option<LightningCatalog>,
) -> Result<RegionalReport, String> {
    regional_search::run(
        doc,
        map,
        lightning,
        std::slice::from_ref(&request.observer),
        &fixed_plan(doc),
        &AtomicBool::new(false),
        |_| {},
    )
}
fn export_replay(
    request: &Request,
    manifest: &Path,
    directory: &Path,
    document: &ReviewWorkspace,
    layers: &[MapData],
    lightning: &Option<LightningCatalog>,
) -> Result<(archive::LoadedInvestigation, PredictionOverlay), String> {
    let map = archive::merge_layers(layers)?;
    let replay = archive::capture_replay_inputs(document, layers, lightning)?;
    let report = make_report(request, document, &map, lightning)?;
    if report.hypotheses.len() != 1 {
        return Err(
            "Fixed supplied hypothesis is not rankable; no successful replay manufactured".into(),
        );
    }
    investigation_report::export_with_replay_inputs(directory, document, &report, &replay)?;
    let loaded = archive::load(directory)?;
    if &loaded.document != document {
        return Err("Exact archived document changed during reopening".into());
    }
    let restored = archive::load_replay_inputs(&loaded)?;
    if serde_json::to_value(&replay).map_err(|e| e.to_string())?
        != serde_json::to_value(&restored).map_err(|e| e.to_string())?
    {
        return Err("Normalized snapshot did not round-trip exactly".into());
    }
    let map = archive::merge_layers(&restored.layers)?;
    archive::verify_archived_inspection(manifest, &loaded, document, &map, &restored.lightning)?;
    let ReportKind::Regional(report) = &loaded.report else {
        return Err("Wrong saved report kind".into());
    };
    let temporary = regional_search::apply_hypothesis(document, report, &report.hypotheses[0].id)?;
    let overlay = build(request, &temporary, &map, &restored.lightning)?;
    archive::verify_archived_inspection(manifest, &loaded, document, &map, &restored.lightning)?;
    Ok((loaded, overlay))
}
fn run(input: &Path, manifest: &Path, output: &Path) -> Result<(), String> {
    if output.exists() {
        return Err("Use a new output directory".into());
    }
    let input_hash = sha256(input)?;
    let manifest_hash = sha256(manifest)?;
    let mut bytes = Vec::new();
    File::open(input)
        .map_err(|e| e.to_string())?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Request exceeds 64 MiB".into());
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    request.document.validate_acoustic_sources_for_scoring()?;
    workspace::verify(manifest, &request.document)?;
    let case = casework::read_case(manifest, true)?;
    if case.case_id != request.document.case_id {
        return Err(
            "Use a SavedCase with the original case ID; the adapter never silently rebinds it"
                .into(),
        );
    }
    let original = build(
        &request,
        &request.document,
        &request.map,
        &request.lightning,
    )?;
    let mut layers = Vec::new();
    let mut lightning = None;
    for binding in &request.document.datasets {
        let path = resolve(manifest, &binding.path);
        let options = request
            .document
            .dataset_options
            .iter()
            .find(|o| o.source_sha256 == binding.sha256)
            .ok_or("Source has no explicit saved interpretation")?;
        if let Some(catalog) = &request.lightning
            && catalog.input_sha256 == binding.sha256
        {
            let mut imported = evidence_score::import_lightning(&path, &catalog.provenance)?;
            imported.input_path = binding.path.clone();
            if serde_json::to_value(&imported).map_err(|e| e.to_string())?
                != serde_json::to_value(catalog).map_err(|e| e.to_string())?
            {
                return Err("Original lightning interpretation changed on native import".into());
            }
            lightning = Some(imported);
        } else {
            let mut layer = map_data::load_with_time_origin(&path, options.time_origin_utc_s)?;
            if layer.source_sha256 != binding.sha256 {
                return Err("Original source bytes changed".into());
            }
            // Native loader canonicalizes the opened path. Retain the original
            // literal binding as provenance after its bytes have been checked.
            layer.source_path = binding.path.clone();
            layers.push(layer);
        }
    }
    if request.lightning.is_some() != lightning.is_some() {
        return Err("Lightning source is absent from the declared bindings".into());
    }
    let map = archive::merge_layers(&layers)?;
    let native = build(&request, &request.document, &map, &lightning)?;
    ensure_equal(&original, &native, "native import of original source files")?;
    fs::create_dir(output).map_err(|e| e.to_string())?;
    let marker = output.join("INCOMPLETE.txt");
    fs::write(&marker, "Replay unfinished; no location accuracy claim.\n")
        .map_err(|e| e.to_string())?;
    let (loaded, reopened) = export_replay(
        &request,
        manifest,
        &output.join("dossier"),
        &request.document,
        &layers,
        &lightning,
    )?;
    ensure_equal(&native, &reopened, "archive round-trip")?;
    write_json(&output.join("original-prediction.json"), &original)?;
    write_json(&output.join("native-prediction.json"), &native)?;
    write_json(&output.join("reopened-prediction.json"), &reopened)?;

    // Missing-source control uses only newly created fixture copies. Original
    // source files and recording are never renamed, moved or deleted.
    let copies = output.join("owned-source-copies");
    fs::create_dir(&copies).map_err(|e| e.to_string())?;
    let copy_root = copies.canonicalize().map_err(|e| e.to_string())?;
    let mut copied_doc = request.document.clone();
    let mut copied_layers = layers.clone();
    let mut copied_lightning = lightning.clone();
    let mut owned = Vec::new();
    let mut total = 0u64;
    for (index, binding) in copied_doc.datasets.iter_mut().enumerate() {
        let source = resolve(manifest, &binding.path);
        let size = fs::metadata(&source).map_err(|e| e.to_string())?.len();
        total = total.checked_add(size).ok_or("Source copy size overflow")?;
        if total > 512 * 1024 * 1024 {
            return Err("Missing-source control exceeds 512 MiB".into());
        }
        let extension = source
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("json");
        if !extension.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err("Unsafe fixture extension".into());
        }
        let dest = copy_root.join(format!("source-{index}.{extension}"));
        let mut from = File::open(&source).map_err(|e| e.to_string())?;
        let mut to = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&dest)
            .map_err(|e| e.to_string())?;
        std::io::copy(&mut Read::by_ref(&mut from).take(size + 1), &mut to)
            .map_err(|e| e.to_string())?;
        to.sync_all().map_err(|e| e.to_string())?;
        drop(to);
        if sha256(&dest)? != binding.sha256 {
            return Err("Owned fixture copy differs from original hash".into());
        }
        for layer in &mut copied_layers {
            if layer.source_sha256 == binding.sha256 {
                layer.source_path = dest.clone();
            }
        }
        if let Some(catalog) = &mut copied_lightning
            && catalog.input_sha256 == binding.sha256
        {
            catalog.input_path = dest.clone();
        }
        binding.path = dest.clone();
        owned.push((dest, binding.sha256.clone()));
    }
    workspace::verify(manifest, &copied_doc)?;
    let copied_native = build(
        &request,
        &copied_doc,
        &archive::merge_layers(&copied_layers)?,
        &copied_lightning,
    )?;
    ensure_equal(&native, &copied_native, "owned source-copy control")?;
    let (copied_saved, _) = export_replay(
        &request,
        manifest,
        &output.join("missing-source-dossier"),
        &copied_doc,
        &copied_layers,
        &copied_lightning,
    )?;
    for (path, expected) in &owned {
        if path.parent() != Some(copy_root.as_path())
            || path.canonicalize().map_err(|e| e.to_string())? != *path
            || sha256(path)? != *expected
        {
            return Err("Owned fixture changed; refusing cleanup".into());
        }
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    let missing = archive::load(&copied_saved.directory)?;
    let restored = archive::load_replay_inputs(&missing)?;
    let restored_map = archive::merge_layers(&restored.layers)?;
    archive::verify_archived_inspection(
        manifest,
        &missing,
        &copied_doc,
        &restored_map,
        &restored.lightning,
    )?;
    let missing_overlay = build(&request, &copied_doc, &restored_map, &restored.lightning)?;
    ensure_equal(
        &native,
        &missing_overlay,
        "archived inspection with missing original fixture sources",
    )?;
    if workspace::verify(manifest, &copied_doc).is_ok() {
        return Err("Live source verification wrongly accepted missing source files".into());
    }
    write_json(
        &output.join("missing-source-prediction.json"),
        &missing_overlay,
    )?;
    if sha256(input)? != input_hash || sha256(manifest)? != manifest_hash {
        return Err("Original request or case changed during replay".into());
    }
    workspace::verify(manifest, &request.document)?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    write_json(
        &output.join("receipt.json"),
        &json!({"schema_version":1,"status":"passed","request_sha256":input_hash,"case_manifest_sha256":manifest_hash,
        "executable_sha256":sha256(&executable)?,"archive_receipt_sha256":loaded.receipt_sha256,"missing_source_archive_receipt_sha256":missing.receipt_sha256,
        "original_inputs_unchanged":true,"all_physical_predictions_and_measured_residuals_identical":true,"raw_source_files_not_rechecked_for_archived_inspection":true,
        "live_apply_requires_raw_sources":true,"copied_source_count":owned.len(),"loaded_layer_count":layers.len(),"observed_point_count":request.document.active_scoring_events().map(|e|e.points.len()).sum::<usize>(),
        "scope":"Persistence regression of the supplied hypothesis, not a new location or source-identification trial","location_verified":false,"location_accuracy_validated":false}),
    )?;
    fs::remove_file(marker).map_err(|e| e.to_string())?;
    println!(
        "Replay preserved predictions and missing-source boundaries; location remains unverified."
    );
    Ok(())
}
fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        eprintln!("Usage: reopen_investigation INPUT.json SAVED_CASE.json NEW_OUTPUT_DIRECTORY");
        std::process::exit(2);
    }
    if let Err(error) = run(
        Path::new(&args[0]),
        Path::new(&args[1]),
        Path::new(&args[2]),
    ) {
        eprintln!("Reopening replay failed: {error}");
        std::process::exit(2);
    }
}
