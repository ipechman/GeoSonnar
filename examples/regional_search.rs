//! Headless adapter for the same regional engine used by the GUI.
//! No truth labels, event discovery, or automatic acceptance are performed here.
use geosonnar::{
    detailed_data::sha256,
    evidence_score::{LightningCatalog, ObserverLocation},
    map_data::MapData,
    regional_search::{self, RegionalPlan},
    workspace::ReviewWorkspace,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    schema_version: u32,
    cases: Vec<Case>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    document: ReviewWorkspace,
    map: MapData,
    lightning: Option<LightningCatalog>,
    observers: Vec<ObserverLocation>,
    plan: RegionalPlan,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn compiled_sources() -> BTreeMap<&'static str, String> {
    [
        ("Cargo.toml", include_bytes!("../Cargo.toml").as_slice()),
        ("Cargo.lock", include_bytes!("../Cargo.lock").as_slice()),
        (
            "src/regional_search.rs",
            include_bytes!("../src/regional_search.rs").as_slice(),
        ),
        (
            "src/workspace_scoring.rs",
            include_bytes!("../src/workspace_scoring.rs").as_slice(),
        ),
        (
            "src/workspace.rs",
            include_bytes!("../src/workspace.rs").as_slice(),
        ),
        (
            "src/physics.rs",
            include_bytes!("../src/physics.rs").as_slice(),
        ),
        (
            "src/evidence_score.rs",
            include_bytes!("../src/evidence_score.rs").as_slice(),
        ),
        (
            "src/modality_diagnostics.rs",
            include_bytes!("../src/modality_diagnostics.rs").as_slice(),
        ),
        (
            "src/atmosphere.rs",
            include_bytes!("../src/atmosphere.rs").as_slice(),
        ),
        (
            "src/receiver_heights.rs",
            include_bytes!("../src/receiver_heights.rs").as_slice(),
        ),
        (
            "examples/regional_search.rs",
            include_bytes!("regional_search.rs").as_slice(),
        ),
    ]
    .into_iter()
    .map(|(name, bytes)| (name, digest(bytes)))
    .collect()
}
fn write_new(path: &Path, value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Per-file report exceeds 64 MiB".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(digest(&bytes))
}
fn run(input: &Path, output: &Path) -> Result<(), String> {
    let mut bytes = Vec::new();
    fs::File::open(input)
        .map_err(|e| e.to_string())?
        .take(64 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("Input exceeds 64 MiB".into());
    }
    let input_sha = digest(&bytes);
    let batch: Batch = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if batch.schema_version != 1 || batch.cases.is_empty() || batch.cases.len() > 64 {
        return Err("Regional batch needs schema 1 and 1..64 cases".into());
    }
    let mut ids = BTreeSet::new();
    let mut total_retained = 0usize;
    for case in &batch.cases {
        if case.id.is_empty()
            || case.id.len() > 100
            || !case
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(&case.id)
        {
            return Err("Cases require distinct safe identifiers".into());
        }
        total_retained = total_retained
            .checked_add(case.plan.retain_hypotheses)
            .ok_or("Retained-result count overflowed")?;
    }
    if total_retained > 5000 {
        return Err("Batch exceeds 5000 retained hypotheses".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let executable_sha = sha256(&executable)?;
    fs::create_dir(output).map_err(|e| format!("Use a new regional output directory: {e}"))?;
    write_new(&output.join("inputs.json"), &batch)?;
    write_new(
        &output.join("build.json"),
        &serde_json::json!({
            "schema_version":1,"package_version":env!("CARGO_PKG_VERSION"),
            "input_sha256":input_sha,"executable_sha256":executable_sha,
            "compiled_source_sha256":compiled_sources(),"evaluation_labels_read":false,
            "location_verified":false,"gui_opened":false
        }),
    )?;
    let cancel = AtomicBool::new(false);
    let mut outcomes = Vec::new();
    for case in &batch.cases {
        let started = Instant::now();
        let result = regional_search::run(
            &case.document,
            &case.map,
            &case.lightning,
            &case.observers,
            &case.plan,
            &cancel,
            |_| {},
        );
        match result {
            Ok(report)=>{
                let name=format!("{}.json",case.id);
                let report_sha=write_new(&output.join(&name),&report)?;
                outcomes.push(serde_json::json!({"id":case.id,"status":"completed","report":name,"report_sha256":report_sha,"elapsed_s":started.elapsed().as_secs_f64(),"hypotheses":report.hypotheses.len()}));
            },
            Err(error)=>outcomes.push(serde_json::json!({"id":case.id,"status":"input_or_search_error","error":error,"elapsed_s":started.elapsed().as_secs_f64()})),
        }
    }
    if sha256(input)? != input_sha || sha256(&executable)? != executable_sha {
        return Err("Input or executable changed during the regional run".into());
    }
    write_new(
        &output.join("summary.json"),
        &serde_json::json!({
            "schema_version":1,"status":"completed","input_sha256":input_sha,
            "executable_sha256":executable_sha,"cases":outcomes,"location_verified":false,
            "input_and_binary_unchanged":true,"evaluation_labels_read":false
        }),
    )?;
    println!(
        "Wrote {} headless case outcomes to {}",
        batch.cases.len(),
        output.display()
    );
    Ok(())
}
fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.len() == 4 && args[0] == "--input" && args[2] == "--out" {
        run(&PathBuf::from(&args[1]), &PathBuf::from(&args[3]))
    } else {
        Err("Usage: regional_search --input BATCH.json --out NEW_DIRECTORY".into())
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
