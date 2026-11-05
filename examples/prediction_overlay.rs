//! Headless export of the same immutable candidate inspection used by the GUI.
//! Inputs contain explicit assumptions, never evaluation-location labels.
use geosonnar::{
    detailed_data::sha256,
    evidence_score::{LightningCatalog, ObserverLocation},
    map_data::MapData,
    prediction_overlay::{self, OverlaySettings},
    workspace::ReviewWorkspace,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
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

fn run(input: &Path, output: &Path) -> Result<(), String> {
    let before = sha256(input)?;
    let mut bytes = Vec::new();
    std::fs::File::open(input)
        .map_err(|e| e.to_string())?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Inspection input exceeds 0.0625 GiB".into());
    }
    let request: Request = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let overlay = prediction_overlay::build(
        &request.document,
        &request.map,
        &request.lightning,
        request.document.strike_fallback_height_m,
        &request.observer,
        &request.settings,
    )?;
    if sha256(input)? != before {
        return Err("Inspection input changed while computing predictions".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let document = json!({
        "schema_version": 1,
        "status": "completed",
        "input_sha256": before,
        "executable_sha256": sha256(&executable)?,
        "source_binding": "Use the accompanying full validation source snapshot for this build",
        "location_verified": false,
        "overlay": overlay
    });
    let bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Inspection output exceeds 0.0625 GiB".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    println!("Inspection exported; no evidence changed or location accepted.");
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        eprintln!("Usage: prediction_overlay INPUT.json NEW_OUTPUT.json");
        std::process::exit(2);
    }
    if let Err(error) = run(Path::new(&args[0]), Path::new(&args[1])) {
        eprintln!("Inspection failed: {error}");
        std::process::exit(2);
    }
}
