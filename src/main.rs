use geosonnar::{
    acquisition, audio, media,
    verification::{self, read_json, write_json},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const HELP: &str = concat!("GeoSonnar ", env!("CARGO_PKG_VERSION"), " - aircraft-audio replication workbench (command-line companion)

geosonnar inspect [--project DIR | --input VIDEO] [--out REPORT.json]
geosonnar case-create --input RECORDING --title TITLE --out NEW_CASE_DIR [--ffprobe PATH]
geosonnar case-status --case CASE_DIR/case.json [--out REPORT.json]
geosonnar case-add-hint --case CASE_DIR/case.json --hint HINT.json
geosonnar job-create --plan PLAN.json --out NEW_JOB_DIR
geosonnar job-run --job JOB_DIR [--resume true]
geosonnar job-status --job JOB_DIR [--out NEW_REPORT.json]
geosonnar job-cancel --job JOB_DIR
geosonnar job-recover --job JOB_DIR
geosonnar preflight --case CASE.json [--out REPORT.json]
geosonnar analyze --input VIDEO --out DIR [--channel 0]
geosonnar discover --input AUDIO_OR_VIDEO --out NEW_DIR [--channel 0] [--settings SETTINGS.json]
geosonnar review-discovery --input DISCOVERY.json --out NEW_REVIEW.html
geosonnar prepare-reviewed-search --config BRIDGE.json --out NEW_DIR
geosonnar derive-audio-priors --config AUDIO_REQUEST.json --out NEW_DIR
geosonnar prepare-audio-search --priors PRIORS.json --geometry GEOMETRY.json --out NEW_DIR
geosonnar fetch --request REQUEST.json --out DIR
geosonnar plan-adsblol --request REQUEST.json --out NEW_DIR
geosonnar fetch-adsblol --plan PLAN.json --cache DIR
geosonnar plan-adsblol-history --request REQUEST.json --out NEW_PLAN.json
geosonnar fetch-adsblol-history --plan PLAN.json --cache DIR
geosonnar import-adsblol-history --cache DIR --report REPORT.json --config CONFIG.json --out NEW_REPORT.json [--allow-partial true]
geosonnar plan-fmi-lightning --case CASE_DIR/case.json --request REQUEST.json
geosonnar fetch-fmi-lightning --case CASE_DIR/case.json --id CATALOG_ID
geosonnar fmi-lightning-status --case CASE_DIR/case.json --id CATALOG_ID
geosonnar import-fmi-lightning --case CASE_DIR/case.json --id CATALOG_ID
geosonnar measure-thunder-envelope --config MEASUREMENT.json --out NEW_DIR
geosonnar preview-thunder-envelope --config RUN.json [--out NEW_PREVIEW.json]
geosonnar search-thunder-envelope --config RUN.json --out NEW_DIR [--cancel-file PATH]
geosonnar preview-joint-evidence --request JOINT_REQUEST.json [--out NEW_PREVIEW.json]
geosonnar search-joint-evidence --request JOINT_REQUEST.json --recording MEDIA --out NEW_DIR [--cancel-file PATH]
geosonnar normalize --input FLIGHTS.json --config NORMALIZATION.json --out REPORT.json
geosonnar import-adsblol-trace --input TRACE.json --config CONFIG.json --out REPORT.json
geosonnar prepare-adsblol-import --manifest MANIFEST.json --config CONFIG.json --out NEW_REQUEST.json
geosonnar import-adsblol --request IMPORT.json --out NEW_DIR
geosonnar import-adsblol-indexed --request IMPORT.json --out NEW_DIR --max-disk-bytes BYTES [--max-index-memory-bytes BYTES] [--cancel-file PATH]
geosonnar terrain-grid --config TERRAIN.json --out NEW_DIR
geosonnar prepare-search --config SEARCH_CONFIG.json --out SEARCH_INPUT.json
geosonnar rank --input SEARCH_INPUT.json --out RESULTS.json
geosonnar wide-pair-estimate --request REQUEST.json --out ESTIMATE.json [--cancel-file FILE]
geosonnar wide-pair-search --request REQUEST.json --out NEW_DIR [--cancel-file PATH]
geosonnar rank-single --input SEARCH_INPUT.json --out RESULTS.json
geosonnar decide --input EVIDENCE.json --out DECISION.json
geosonnar overlays --input SEARCH_INPUT.json --results RESULTS.json --out CURVES.json [--top 3]
geosonnar demo --out DIR

All file paths may contain spaces. Case/config-relative inputs resolve relative to
their config file. Explicit command paths resolve relative to the current folder.
FFmpeg/FFprobe: next to the executable, in its tools folder, or on PATH.
Fetch: requires curl.exe on Windows. Cached files are reused by default.
Search output is a ranked hypothesis, not an automatic verification verdict.
Exit 2: command/input error. Exit 3: preflight blocked, no candidate, or
incomplete regional history acquisition (partial, cancelled or budget exhausted).
Exit 4: saved job cancelled. Resume requires --resume true.
");

struct Args {
    command: String,
    flags: BTreeMap<String, String>,
}

struct SignalWhenDropped<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for SignalWhenDropped<'_> {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
fn with_cancel_file<T>(
    path: Option<PathBuf>,
    task: impl FnOnce(&std::sync::atomic::AtomicBool) -> Result<T, String>,
) -> Result<T, String> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let cancelled = AtomicBool::new(path.as_ref().is_some_and(|p| p.exists()));
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let _finish = SignalWhenDropped(&finished);
        if let Some(path) = path {
            let cancel = &cancelled;
            let done = &finished;
            scope.spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    if path.exists() {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
        }
        task(&cancelled)
    })
}
impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args().skip(1);
        let command = args.next().unwrap_or_else(|| "help".into());
        let mut flags = BTreeMap::new();
        while let Some(key) = args.next() {
            if !key.starts_with("--") {
                return Err(format!("Expected --option, got {key}"));
            }
            let value = args
                .next()
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("{key} needs a value"))?;
            if flags.insert(key.clone(), value).is_some() {
                return Err(format!("Duplicate option {key}"));
            }
        }
        Ok(Self { command, flags })
    }
    fn required(&self, key: &str) -> Result<&str, String> {
        self.flags
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("{key} is required"))
    }
    fn path(&self, key: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(self.required(key)?))
    }
    fn allowed(&self, keys: &[&str]) -> Result<(), String> {
        for k in self.flags.keys() {
            if !keys.contains(&k.as_str()) {
                return Err(format!("Unknown option {k}"));
            }
        }
        Ok(())
    }
}

fn emit(report: &Value, out: Option<&String>) -> Result<(), String> {
    if let Some(path) = out {
        write_json(Path::new(path), report)?;
        println!("Report: {path}");
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(report).map_err(|e| e.to_string())?
        );
    }
    Ok(())
}

fn emit_fresh(report: &Value, out: Option<&String>) -> Result<(), String> {
    if let Some(path) = out {
        use std::io::Write;
        let mut bytes = serde_json::to_vec_pretty(report).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("Use a new status report file: {e}"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        println!("Report: {path}");
        Ok(())
    } else {
        emit(report, None)
    }
}

fn run() -> Result<i32, String> {
    let args = Args::parse()?;
    match args.command.as_str() {
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            Ok(0)
        }
        "measure-thunder-envelope" => {
            args.allowed(&["--config", "--out"])?;
            let report = geosonnar::envelope_workflow::measure(
                &args.path("--config")?,
                &args.path("--out")?,
            )?;
            println!(
                "{}; observation saved in {}; location_verified=false",
                report["status"],
                args.required("--out")?
            );
            Ok(0)
        }
        "preview-thunder-envelope" => {
            args.allowed(&["--config", "--out"])?;
            let report = geosonnar::envelope_workflow::preview(&args.path("--config")?)?;
            emit_fresh(&report, args.flags.get("--out"))?;
            Ok(0)
        }
        "search-thunder-envelope" => {
            args.allowed(&["--config", "--out", "--cancel-file"])?;
            let config = args.path("--config")?;
            let output = args.path("--out")?;
            let cancel_file = args.flags.get("--cancel-file").map(PathBuf::from);
            let cancelled = std::sync::atomic::AtomicBool::new(
                cancel_file.as_ref().is_some_and(|path| path.exists()),
            );
            let finished = std::sync::atomic::AtomicBool::new(false);
            let report = std::thread::scope(|scope| {
                let _finish = SignalWhenDropped(&finished);
                if let Some(path) = cancel_file {
                    let cancel = &cancelled;
                    let done = &finished;
                    scope.spawn(move || {
                        while !done.load(std::sync::atomic::Ordering::Relaxed) {
                            if path.exists() {
                                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                                break;
                            }
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    });
                }
                let result = geosonnar::envelope_workflow::run(&config, &output, &cancelled, |p| {
                    eprintln!(
                        "Envelope: {} / {} receivers; {} / {} offsets",
                        p.completed_receivers,
                        p.total_receivers,
                        p.evaluated_offsets,
                        p.total_offsets
                    );
                });
                finished.store(true, std::sync::atomic::Ordering::Relaxed);
                result
            })?;
            println!(
                "{}; results saved in {}; location_verified=false",
                report["status"],
                output.display()
            );
            Ok(if report["cancelled"] == true {
                4
            } else if report["complete"] != true || report["retained_hypotheses"] == 0 {
                3
            } else {
                0
            })
        }
        "preview-joint-evidence" => {
            args.allowed(&["--request", "--out"])?;
            let report = geosonnar::joint_workflow::preview(&args.path("--request")?)?;
            emit_fresh(&report, args.flags.get("--out"))?;
            Ok(0)
        }
        "search-joint-evidence" => {
            args.allowed(&["--request", "--recording", "--out", "--cancel-file"])?;
            let request = args.path("--request")?;
            let recording = args.path("--recording")?;
            let output = args.path("--out")?;
            let report = with_cancel_file(
                args.flags.get("--cancel-file").map(PathBuf::from),
                |cancel| {
                    geosonnar::joint_workflow::run(
                        &request,
                        &recording,
                        &output,
                        cancel,
                        |progress| {
                            eprintln!(
                                "Joint {:?}: Doppler {} / {}, envelope {} / {} cells",
                                progress.stage,
                                progress.completed_doppler_cells,
                                progress.total_cells,
                                progress.completed_envelope_cells,
                                progress.total_cells
                            );
                        },
                    )
                },
            )?;
            println!(
                "{}; shared-cell results saved in {}; location_verified=false",
                report["status"],
                output.display()
            );
            Ok(if report["cancelled"] == true {
                4
            } else if report["ranking_available"] != true {
                3
            } else {
                0
            })
        }
        "inspect" => {
            args.allowed(&["--project", "--input", "--out"])?;
            if args.flags.contains_key("--project") && args.flags.contains_key("--input") {
                return Err("Choose --project or --input".into());
            }
            let input = if let Some(p) = args.flags.get("--input") {
                PathBuf::from(p)
            } else {
                media::discover_raw(Path::new(
                    args.flags
                        .get("--project")
                        .map(String::as_str)
                        .unwrap_or("."),
                ))?
            };
            let info = media::inspect(&input)?;
            let report = json!({"status":"input_found","location_verified":false,"media":info});
            emit(&report, args.flags.get("--out"))?;
            Ok(0)
        }
        "case-create" => {
            args.allowed(&["--input", "--title", "--out", "--ffprobe"])?;
            let report = geosonnar::casework::create_with_probe(
                &args.path("--input")?,
                &args.path("--out")?,
                args.required("--title")?,
                args.flags.get("--ffprobe").map(Path::new),
            )?;
            println!(
                "Saved local case: {}; recording_matches={}; location_verified=false",
                args.path("--out")?.join("case.json").display(),
                report["recording_matches"]
            );
            Ok(0)
        }
        "case-status" => {
            args.allowed(&["--case", "--out"])?;
            emit_fresh(
                &geosonnar::casework::status(&args.path("--case")?)?,
                args.flags.get("--out"),
            )?;
            Ok(0)
        }
        "case-add-hint" => {
            args.allowed(&["--case", "--hint"])?;
            let path = geosonnar::casework::add_hint(&args.path("--case")?, &args.path("--hint")?)?;
            println!(
                "Recorded clue: {}; no search bounds or location verdict inferred",
                path.display()
            );
            Ok(0)
        }
        "job-create" => {
            args.allowed(&["--plan", "--out"])?;
            let state = geosonnar::jobs::init_job(&args.path("--plan")?, &args.path("--out")?)?;
            println!(
                "Job prepared: {}; phase={}; location_verified=false",
                args.required("--out")?,
                state.phase
            );
            Ok(0)
        }
        "job-run" => {
            args.allowed(&["--job", "--resume"])?;
            let resume = args
                .flags
                .get("--resume")
                .map(|v| v.parse::<bool>())
                .transpose()
                .map_err(|_| "--resume must be true or false")?
                .unwrap_or(false);
            let state = geosonnar::jobs::run_job(&args.path("--job")?, resume)?;
            println!("Job phase: {}; location_verified=false", state.phase);
            if let Some(message) = &state.message {
                println!("{message}");
            }
            Ok(match state.phase.as_str() {
                "completed" => 0,
                "cancelled" | "canceled" => 4,
                _ => 2,
            })
        }
        "job-status" => {
            args.allowed(&["--job", "--out"])?;
            let state = geosonnar::jobs::status(&args.path("--job")?)?;
            emit_fresh(
                &serde_json::to_value(state).map_err(|e| e.to_string())?,
                args.flags.get("--out"),
            )?;
            Ok(0)
        }
        "job-cancel" => {
            args.allowed(&["--job"])?;
            let state = geosonnar::jobs::cancel_job(&args.path("--job")?)?;
            println!("Cancellation requested; phase={}", state.phase);
            Ok(0)
        }
        "job-recover" => {
            args.allowed(&["--job"])?;
            let state = geosonnar::jobs::recover_job(&args.path("--job")?)?;
            println!(
                "Dead-runner lock recovered; phase={}; resume explicitly with --resume true",
                state.phase
            );
            Ok(0)
        }
        "preflight" => {
            args.allowed(&["--case", "--out"])?;
            let report = verification::preflight(&args.path("--case")?)?;
            let blocked = report["status"] == "blocked";
            emit(&report, args.flags.get("--out"))?;
            if blocked {
                eprintln!("Replication is not ready: see blockers in the report.");
            }
            Ok(if blocked { 3 } else { 0 })
        }
        "analyze" => {
            args.allowed(&["--input", "--out", "--channel"])?;
            let channel = args
                .flags
                .get("--channel")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|_| "Channel must be a nonnegative integer")?
                .unwrap_or(0);
            let report = audio::analyze(&args.path("--input")?, &args.path("--out")?, channel)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "discover" => {
            args.allowed(&["--input", "--out", "--channel", "--settings"])?;
            let channel = args
                .flags
                .get("--channel")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|_| "Channel must be a nonnegative integer")?
                .unwrap_or(0);
            let settings = match args.flags.get("--settings") {
                Some(path) => read_json(Path::new(path))?,
                None => geosonnar::discovery::DiscoverySettings::default(),
            };
            let report = geosonnar::discovery::discover(
                &args.path("--input")?,
                &args.path("--out")?,
                channel,
                &settings,
            )?;
            println!(
                "{}: {} possible flyovers, {} measured ridges; location_verified=false",
                report["status"], report["group_count"], report["ridge_count"]
            );
            println!(
                "Review: {}",
                args.path("--out")?.join("review.html").display()
            );
            Ok(0)
        }
        "review-discovery" => {
            args.allowed(&["--input", "--out"])?;
            geosonnar::discovery::write_review(&args.path("--input")?, &args.path("--out")?)?;
            println!("Review: {}", args.required("--out")?);
            Ok(0)
        }
        "prepare-reviewed-search" => {
            args.allowed(&["--config", "--out"])?;
            let report =
                geosonnar::reviewed_search::prepare(&args.path("--config")?, &args.path("--out")?)?;
            println!(
                "Reviewed search prepared: {} observers, {} passes; location_verified=false",
                report["observer_count"], report["track_count"]
            );
            println!(
                "Input: {}",
                args.path("--out")?.join("search_input.json").display()
            );
            Ok(0)
        }
        "derive-audio-priors" => {
            args.allowed(&["--config", "--out"])?;
            let report =
                geosonnar::audio_priors::derive(&args.path("--config")?, &args.path("--out")?)?;
            println!(
                "{}; location_verified=false; UTC clock not inferred",
                report["status"]
            );
            println!(
                "Priors: {}",
                args.path("--out")?.join("priors.json").display()
            );
            Ok(if report["usable_for_search"] == true {
                0
            } else {
                3
            })
        }
        "prepare-audio-search" => {
            args.allowed(&["--priors", "--geometry", "--out"])?;
            let report = geosonnar::audio_priors::prepare(
                &args.path("--priors")?,
                &args.path("--geometry")?,
                &args.path("--out")?,
            )?;
            println!(
                "{}: {} observers, {} passes; location_verified=false",
                report["status"], report["observer_count"], report["track_count"]
            );
            Ok(0)
        }
        "fetch" => {
            args.allowed(&["--request", "--out"])?;
            let request = read_json(&args.path("--request")?)?;
            let report = acquisition::fetch_adsb_aero(&request, &args.path("--out")?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "plan-adsblol" => {
            args.allowed(&["--request", "--out"])?;
            let request = read_json(&args.path("--request")?)?;
            let plan = geosonnar::adsblol_acquisition::plan(&request, &args.path("--out")?)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "fetch-adsblol" => {
            args.allowed(&["--plan", "--cache"])?;
            let manifest = geosonnar::adsblol_acquisition::fetch(
                &args.path("--plan")?,
                &args.path("--cache")?,
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "plan-adsblol-history" => {
            args.allowed(&["--request", "--out"])?;
            let request = read_json(&args.path("--request")?)?;
            let plan = geosonnar::adsblol_history::plan(&request)?;
            let output = args.path("--out")?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|e| e.to_string())?;
            serde_json::to_writer_pretty(&mut file, &plan).map_err(|e| e.to_string())?;
            println!(
                "Historical plan: {} replay files; at most {} traces; cumulative budget {}. No network requested; coverage_complete=false.",
                plan.replay_chunks.len(),
                plan.request.max_traces,
                geosonnar::data_size::format_gib(plan.request.max_download_bytes)
            );
            Ok(0)
        }
        "plan-fmi-lightning" => {
            args.allowed(&["--case", "--request"])?;
            let request = read_json(&args.path("--request")?)?;
            let item = geosonnar::data_catalog::fmi::plan(&args.path("--case")?, &request)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"entry":item.entry,"plan":item.plan,"report":item.report})
                )
                .map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "fetch-fmi-lightning" | "fmi-lightning-status" => {
            args.allowed(&["--case", "--id"])?;
            let item = if args.command == "fetch-fmi-lightning" {
                geosonnar::data_catalog::fmi::fetch(
                    &args.path("--case")?,
                    args.required("--id")?,
                    &std::sync::atomic::AtomicBool::new(false),
                    |_| {},
                )?
            } else {
                geosonnar::data_catalog::fmi::load(&args.path("--case")?, args.required("--id")?)?
            };
            let incomplete = item
                .report
                .as_ref()
                .is_none_or(|r| r.status != geosonnar::data_catalog::fmi::FmiStatus::Complete);
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"entry":item.entry,"plan":item.plan,"report":item.report})
                )
                .map_err(|e| e.to_string())?
            );
            Ok(if args.command == "fetch-fmi-lightning" && incomplete {
                3
            } else {
                0
            })
        }
        "import-fmi-lightning" => {
            args.allowed(&["--case", "--id"])?;
            let entry = geosonnar::data_catalog::fmi::import(
                &args.path("--case")?,
                args.required("--id")?,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&entry).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "fetch-adsblol-history" => {
            args.allowed(&["--plan", "--cache"])?;
            let plan = read_json(&args.path("--plan")?)?;
            let report = geosonnar::adsblol_history::fetch(
                &plan,
                &args.path("--cache")?,
                &std::sync::atomic::AtomicBool::new(false),
                |_| {},
            )?;
            println!(
                "{}",
                serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?
            );
            Ok(
                if report.status == geosonnar::adsblol_history::HistoryStatus::Complete {
                    0
                } else {
                    3
                },
            )
        }
        "import-adsblol-history" => {
            args.allowed(&[
                "--cache",
                "--report",
                "--config",
                "--out",
                "--allow-partial",
            ])?;
            let allow_partial = match args.flags.get("--allow-partial").map(String::as_str) {
                None | Some("false") => false,
                Some("true") => true,
                _ => return Err("--allow-partial must be true or false".into()),
            };
            let report = read_json(&args.path("--report")?)?;
            let config = read_json(&args.path("--config")?)?;
            let imported = geosonnar::adsblol_history::import_traces(
                &args.path("--cache")?,
                &report,
                &config,
                allow_partial,
            )?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(args.path("--out")?)
                .map_err(|e| e.to_string())?;
            serde_json::to_writer_pretty(&mut file, &imported).map_err(|e| e.to_string())?;
            println!(
                "Historical native import: {} passes, {} points; coverage_complete=false; location_verified=false",
                imported.stats.output_passes, imported.stats.output_points
            );
            Ok(0)
        }
        "normalize" => {
            args.allowed(&["--input", "--config", "--out"])?;
            let raw = std::fs::read_to_string(args.path("--input")?).map_err(|e| e.to_string())?;
            let config = read_json(&args.path("--config")?)?;
            let report = geosonnar::normalization::normalize_adsb_aero(&raw, &config)?;
            write_json(&args.path("--out")?, &report)?;
            println!("Normalized tracks: {}", args.required("--out")?);
            Ok(0)
        }
        "import-adsblol-trace" => {
            args.allowed(&["--input", "--config", "--out"])?;
            let input = std::fs::read(args.path("--input")?).map_err(|e| e.to_string())?;
            let text = geosonnar::detailed_data::decode_member(&input)?;
            let config = read_json(&args.path("--config")?)?;
            let report = geosonnar::adsblol::normalize_trace(&text, &config)?;
            write_json(&args.path("--out")?, &report)?;
            println!(
                "Detailed trace: {} passes, {} points; location_verified=false",
                report.stats.output_passes, report.stats.output_points
            );
            Ok(0)
        }
        "prepare-adsblol-import" => {
            args.allowed(&["--manifest", "--config", "--out"])?;
            geosonnar::detailed_data::prepare_import(
                &args.path("--manifest")?,
                &args.path("--config")?,
                &args.path("--out")?,
            )?;
            println!("Import request: {}", args.required("--out")?);
            Ok(0)
        }
        "import-adsblol" => {
            args.allowed(&["--request", "--out"])?;
            let report = geosonnar::detailed_data::import_archive(
                &args.path("--request")?,
                &args.path("--out")?,
            )?;
            println!(
                "Detailed archive: {} traces scanned, {} passes; location_verified=false",
                report["traces_scanned"], report["stats"]["output_passes"]
            );
            Ok(0)
        }
        "import-adsblol-indexed" => {
            args.allowed(&[
                "--request",
                "--out",
                "--max-disk-bytes",
                "--max-index-memory-bytes",
                "--cancel-file",
            ])?;
            let limit = args
                .required("--max-disk-bytes")?
                .parse::<u64>()
                .map_err(|_| "--max-disk-bytes must be an integer byte budget")?;
            let memory = args
                .flags
                .get("--max-index-memory-bytes")
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| "--max-index-memory-bytes must be an integer byte budget")
                })
                .transpose()?
                .unwrap_or(geosonnar::track_store::DEFAULT_INDEX_MEMORY_BYTES);
            let report = with_cancel_file(
                args.flags.get("--cancel-file").map(PathBuf::from),
                |cancel| {
                    geosonnar::detailed_data::import_archive_indexed_with_limits(
                        &args.path("--request")?,
                        &args.path("--out")?,
                        limit,
                        memory,
                        cancel,
                    )
                },
            )?;
            println!(
                "Indexed archive: {}",
                serde_json::to_string(&report).map_err(|e| e.to_string())?
            );
            Ok(0)
        }
        "terrain-grid" => {
            args.allowed(&["--config", "--out"])?;
            let report = geosonnar::detailed_data::terrain_grid(
                &args.path("--config")?,
                &args.path("--out")?,
            )?;
            println!(
                "Terrain grid: {} observers; location_verified=false",
                report["observer_count"]
            );
            Ok(0)
        }
        "prepare-search" => {
            args.allowed(&["--config", "--out"])?;
            let input = geosonnar::search_setup::prepare(&args.path("--config")?)?;
            write_json(&args.path("--out")?, &input)?;
            println!(
                "Search: {} observers, {} passes",
                input.observers.len(),
                input.tracks.len()
            );
            Ok(0)
        }
        "wide-pair-estimate" | "wide-pair-search" => {
            args.allowed(&["--request", "--out", "--cancel-file"])?;
            let request_path = args.path("--request")?;
            use std::io::Read;
            let mut raw = Vec::new();
            std::fs::File::open(&request_path)
                .map_err(|e| e.to_string())?
                .take(geosonnar::wide_area_pair::MAX_REQUEST_BYTES as u64 + 1)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            if raw.len() > geosonnar::wide_area_pair::MAX_REQUEST_BYTES {
                return Err("Wide-area request exceeds its metadata size bound".into());
            }
            let mut request: geosonnar::wide_area_pair::WidePairRequest =
                serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
            if request.source.manifest_path.is_relative() {
                request.source.manifest_path = request_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join(&request.source.manifest_path);
            }
            if args.command == "wide-pair-estimate" {
                let estimate = with_cancel_file(
                    args.flags.get("--cancel-file").map(PathBuf::from),
                    |cancel| {
                        geosonnar::wide_area_pair::estimate_with_cancel(&request, cancel, |_| {})
                    },
                )?;
                write_json(&args.path("--out")?, &estimate)?;
                println!(
                    "Wide-area estimate: {} receivers, {} source passes",
                    estimate.receiver_count, estimate.source_passes
                );
                Ok(0)
            } else {
                let report = with_cancel_file(
                    args.flags.get("--cancel-file").map(PathBuf::from),
                    |cancel| {
                        geosonnar::wide_area_pair::run(
                            &request,
                            &std::path::absolute(args.path("--out")?).map_err(|e| e.to_string())?,
                            cancel,
                            |_| {},
                        )
                    },
                )?;
                println!(
                    "Wide-area {:?}: {} / {} receivers; location_verified=false",
                    report.status,
                    report.progress.completed_receivers,
                    report.progress.total_receivers
                );
                Ok(
                    if report.status == geosonnar::wide_area_pair::WideStatus::Completed {
                        0
                    } else {
                        3
                    },
                )
            }
        }
        "rank" => {
            args.allowed(&["--input", "--out"])?;
            let input = read_json(&args.path("--input")?)?;
            let report = verification::rank(&input)?;
            write_json(&args.path("--out")?, &report)?;
            println!(
                "{}: {} ranked hypotheses; location_verified=false",
                report["status"], report["ranked_count"]
            );
            Ok(if report["status"] == "no_candidate" {
                3
            } else {
                0
            })
        }
        "rank-single" => {
            args.allowed(&["--input", "--out"])?;
            let input = read_json(&args.path("--input")?)?;
            let report = geosonnar::single::rank(&input)?;
            write_json(&args.path("--out")?, &report)?;
            println!(
                "{}: {} ranked hypotheses",
                report["status"], report["ranked_count"]
            );
            Ok(if report["status"] == "no_candidate" {
                3
            } else {
                0
            })
        }
        "decide" => {
            args.allowed(&["--input", "--out"])?;
            let input = read_json(&args.path("--input")?)?;
            let report = geosonnar::decision::decide(&input);
            write_json(&args.path("--out")?, &report)?;
            println!("{}: {}", report["status"], report["reason_code"]);
            Ok(if report["status"] == "failed" { 2 } else { 0 })
        }
        "overlays" => {
            args.allowed(&["--input", "--results", "--out", "--top"])?;
            let input = read_json(&args.path("--input")?)?;
            let results = read_json(&args.path("--results")?)?;
            let top = args
                .flags
                .get("--top")
                .map(|s| s.parse::<usize>())
                .transpose()
                .map_err(|_| "--top must be a number")?
                .unwrap_or(3);
            let curves = geosonnar::overlay::export(&input, &results, top)?;
            write_json(&args.path("--out")?, &curves)?;
            println!("Model curves: {}", args.required("--out")?);
            Ok(0)
        }
        "demo" => {
            args.allowed(&["--out"])?;
            let dir = args.path("--out")?;
            let input = verification::demo_input()?;
            write_json(&dir.join("synthetic_input.json"), &input)?;
            let report = verification::rank(&input)?;
            write_json(&dir.join("synthetic_results.json"), &report)?;
            println!(
                "Synthetic model test completed. This does not verify the downloaded recording."
            );
            Ok(0)
        }
        other => Err(format!("Unknown command {other}. Run geosonnar --help.")),
    }
}

fn main() {
    if let Err(error) = geosonnar::jobs::await_worker_gate() {
        eprintln!("GeoSonnar: {error}");
        std::process::exit(2);
    }
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("GeoSonnar: {e}");
            std::process::exit(2);
        }
    }
}
