//! Small acquisition-only runner; useful while the rest of the pipeline evolves.
use geosonnar::acquisition::{AcquisitionRequest, fetch_adsb_aero};
use std::path::Path;

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("Usage: fetch_public <request.json> <output-directory>".into());
    }
    let request: AcquisitionRequest =
        serde_json::from_slice(&std::fs::read(&args[1]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let report = fetch_adsb_aero(&request, Path::new(&args[2]))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
