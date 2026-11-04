//! Headless ablation adapter for the production manual-workspace scorer.
//! This program accepts no evaluation labels, learns no weights, performs no
//! parameter/association fitting and never declares a verified location.
//! Run: hybrid_ablation --input inputs.json --out NEW_predictions.json
use geosonnar::{
    evidence_score::{
        CandidateScore, EvidenceTerm, LightningCatalog, ObserverLocation, ScoreConfig, ScoreReport,
    },
    map_data::MapData,
    workspace::{EvidenceMix, ReviewWorkspace},
    workspace_scoring,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, BufWriter, Read, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_CASES: usize = 128;
const MAX_TOTAL_OBSERVERS: usize = 100_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Batch {
    schema_version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    document: ReviewWorkspace,
    map: MapData,
    lightning: Option<LightningCatalog>,
    observers: Vec<ObserverLocation>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Mode {
    DopplerOnly,
    ThunderOnly,
    Combined,
}

impl Mode {
    fn apply(self, mix: &mut EvidenceMix) {
        mix.doppler_frequency = matches!(self, Self::DopplerOnly | Self::Combined);
        mix.doppler_timing = matches!(self, Self::DopplerOnly | Self::Combined);
        mix.thunder_timing = matches!(self, Self::ThunderOnly | Self::Combined);
    }
}

#[derive(Serialize)]
struct InputIdentity {
    bytes: u64,
    sha256: String,
}

#[derive(Serialize)]
struct Header {
    schema_version: u32,
    scorer: &'static str,
    package_version: &'static str,
    input: InputIdentity,
    executable: InputIdentity,
    compiled_source_sha256: BTreeMap<&'static str, String>,
    mode_contract: &'static str,
    evaluation_labels_read: bool,
    location_verified: bool,
}

#[derive(Serialize)]
struct CaseResult {
    id: String,
    case_input_sha256: String,
    observer_sha256: String,
    document_invariant_sha256: String,
    map_sha256: String,
    lightning_sha256: String,
    observer_count: usize,
    modes: Vec<ModeResult>,
}

#[derive(Serialize)]
struct ModeResult {
    mode: Mode,
    status: &'static str,
    elapsed_s: f64,
    document_sha256: String,
    document_invariant_sha256: String,
    error: Option<String>,
    report: Option<CompactReport>,
}

#[derive(Serialize)]
struct CompactReport {
    config: ScoreConfig,
    selected_terms: Vec<EvidenceTerm>,
    evidence_sha256: String,
    ranked_candidate_ids: Vec<String>,
    candidates: Vec<CompactCandidate>,
    calibration: String,
    location_verified: bool,
    limitations: Vec<String>,
}

#[derive(Serialize)]
struct CompactCandidate {
    id: String,
    observer: ObserverLocation,
    rank: Option<usize>,
    score: Option<f64>,
    terms: Vec<CompactTerm>,
    unavailable_reasons: Vec<String>,
    location_verified: bool,
}

#[derive(Serialize)]
struct CompactTerm {
    term: EvidenceTerm,
    weight: f64,
    residual_scale: f64,
    available: bool,
    unavailable_reason: Option<String>,
    observation_count: usize,
    event_mean_squared_residuals: BTreeMap<String, f64>,
    mean_normalized_squared_residual: Option<f64>,
    weighted_contribution: Option<f64>,
}

impl From<CandidateScore> for CompactCandidate {
    fn from(value: CandidateScore) -> Self {
        Self {
            id: value.candidate.id,
            observer: value.candidate.observer,
            rank: value.rank,
            score: value.score,
            terms: value
                .terms
                .into_iter()
                .map(|term| CompactTerm {
                    term: term.term,
                    weight: term.weight,
                    residual_scale: term.residual_scale,
                    available: term.available,
                    unavailable_reason: term.unavailable_reason,
                    observation_count: term.residuals.len(),
                    event_mean_squared_residuals: term.event_mean_squared_residuals,
                    mean_normalized_squared_residual: term.mean_normalized_squared_residual,
                    weighted_contribution: term.weighted_contribution,
                })
                .collect(),
            unavailable_reasons: value.unavailable_reasons,
            location_verified: value.location_verified,
        }
    }
}

impl From<ScoreReport> for CompactReport {
    fn from(value: ScoreReport) -> Self {
        Self {
            config: value.config,
            selected_terms: value.selected_terms,
            evidence_sha256: value.evidence_sha256,
            ranked_candidate_ids: value.ranked_candidate_ids,
            candidates: value.candidates.into_iter().map(Into::into).collect(),
            calibration: value.calibration,
            location_verified: value.location_verified,
            limitations: value.limitations,
        }
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn json_hash(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_vec(value)
        .map(|bytes| hash(&bytes))
        .map_err(|error| error.to_string())
}

fn invariant_hash(document: &ReviewWorkspace) -> Result<String, String> {
    let mut copy = document.clone();
    copy.mix.doppler_frequency = false;
    copy.mix.doppler_timing = false;
    copy.mix.thunder_timing = false;
    json_hash(&copy)
}

fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > MAX_INPUT_BYTES {
        return Err("Input exceeds 64 MiB".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("Input grew beyond 64 MiB during reading".into());
    }
    Ok(bytes)
}

fn validate_batch(batch: &Batch) -> Result<(), String> {
    if batch.schema_version != 1 || batch.cases.is_empty() || batch.cases.len() > MAX_CASES {
        return Err(format!("Schema 1 requires 1..{MAX_CASES} cases"));
    }
    let mut names = BTreeSet::new();
    let mut total = 0usize;
    for case in &batch.cases {
        if case.id.is_empty()
            || case.id.len() > 128
            || !case
                .id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || !names.insert(&case.id)
        {
            return Err("Case IDs must be unique opaque ASCII names of 1..128 characters".into());
        }
        if case.observers.is_empty() || case.observers.len() > workspace_scoring::MAX_OBSERVERS {
            return Err("Each case needs 1..10000 observers".into());
        }
        total += case.observers.len();
        if total > MAX_TOTAL_OBSERVERS {
            return Err(format!(
                "Batch exceeds {MAX_TOTAL_OBSERVERS} total observers"
            ));
        }
        let mut observers = BTreeSet::new();
        for point in &case.observers {
            if !point.latitude_deg.is_finite()
                || !point.longitude_deg.is_finite()
                || !point.ellipsoid_height_m.is_finite()
                || !(-85.0..=85.0).contains(&point.latitude_deg)
                || !(-180.0..=180.0).contains(&point.longitude_deg)
                || !(-1000.0..=100_000.0).contains(&point.ellipsoid_height_m)
            {
                return Err("Invalid explicit observer coordinate or height".into());
            }
            let bits = |n: f64| if n == 0.0 { 0 } else { n.to_bits() };
            if !observers.insert([
                bits(point.latitude_deg),
                bits(point.longitude_deg),
                bits(point.ellipsoid_height_m),
            ]) {
                return Err(
                    "Duplicate receiver coordinates would bias candidate comparisons".into(),
                );
            }
        }
        if !case.map.candidates.is_empty()
            || !case.map.observers.is_empty()
            || case.map.grid.is_some()
            || case.map.terrain.is_some()
            || !case.map.lightning.is_empty()
            || case.map.location_verified
        {
            return Err("Map input must contain model tracks, not prior answers, grids or duplicate lightning layers".into());
        }
    }
    Ok(())
}

fn score_case(case: &Case) -> Result<CaseResult, String> {
    let invariant = invariant_hash(&case.document)?;
    let mut modes = Vec::new();
    for mode in [Mode::DopplerOnly, Mode::ThunderOnly, Mode::Combined] {
        let mut document = case.document.clone();
        mode.apply(&mut document.mix);
        let mode_invariant = invariant_hash(&document)?;
        if mode_invariant != invariant {
            return Err(
                "A mode changed evidence or assumptions beyond the three enable flags".into(),
            );
        }
        let started = Instant::now();
        let result = workspace_scoring::score_observers(
            &document,
            &case.map,
            &case.lightning,
            document.strike_fallback_height_m,
            &case.observers,
        );
        let elapsed_s = started.elapsed().as_secs_f64();
        let (status, error, report) = match result {
            Ok(report) => ("scored", None, Some(report.into())),
            Err(error) => ("unavailable", Some(error), None),
        };
        modes.push(ModeResult {
            mode,
            status,
            elapsed_s,
            document_sha256: json_hash(&document)?,
            document_invariant_sha256: mode_invariant,
            error,
            report,
        });
    }
    Ok(CaseResult {
        id: case.id.clone(),
        case_input_sha256: json_hash(case)?,
        observer_sha256: json_hash(&case.observers)?,
        document_invariant_sha256: invariant,
        map_sha256: json_hash(&case.map)?,
        lightning_sha256: json_hash(&case.lightning)?,
        observer_count: case.observers.len(),
        modes,
    })
}

fn compiled_sources() -> BTreeMap<&'static str, String> {
    [
        (
            "src/atmosphere.rs",
            include_bytes!("../src/atmosphere.rs").as_slice(),
        ),
        (
            "src/modality_diagnostics.rs",
            include_bytes!("../src/modality_diagnostics.rs").as_slice(),
        ),
        (
            "examples/hybrid_ablation.rs",
            include_bytes!("hybrid_ablation.rs").as_slice(),
        ),
        (
            "src/workspace_scoring.rs",
            include_bytes!("../src/workspace_scoring.rs").as_slice(),
        ),
        (
            "src/evidence_score.rs",
            include_bytes!("../src/evidence_score.rs").as_slice(),
        ),
        (
            "src/physics.rs",
            include_bytes!("../src/physics.rs").as_slice(),
        ),
        (
            "src/workspace.rs",
            include_bytes!("../src/workspace.rs").as_slice(),
        ),
        (
            "src/thunder.rs",
            include_bytes!("../src/thunder.rs").as_slice(),
        ),
        (
            "src/map_data.rs",
            include_bytes!("../src/map_data.rs").as_slice(),
        ),
        ("Cargo.toml", include_bytes!("../Cargo.toml").as_slice()),
        ("Cargo.lock", include_bytes!("../Cargo.lock").as_slice()),
    ]
    .into_iter()
    .map(|(name, bytes)| (name, hash(bytes)))
    .collect()
}

struct BoundedWriter {
    file: BufWriter<File>,
    bytes: u64,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.bytes + bytes.len() as u64 > MAX_OUTPUT_BYTES {
            return Err(io::Error::other("Prediction report exceeds 256 MiB"));
        }
        let written = self.file.write(bytes)?;
        self.bytes += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

struct TemporaryOutput(PathBuf);
impl Drop for TemporaryOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn ensure_new(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err("Output exists; choose a new prediction file".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn execute(input: &Path, output: &Path) -> Result<(), String> {
    ensure_new(output)?;
    let bytes = read_bounded(input)?;
    let batch: Batch = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    validate_batch(&batch)?;
    let input_hash = hash(&bytes);
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let binary = fs::read(executable).map_err(|error| error.to_string())?;
    let header = Header {
        schema_version: 1,
        scorer: "geosonnar::workspace_scoring::score_observers",
        package_version: env!("CARGO_PKG_VERSION"),
        input: InputIdentity {
            bytes: bytes.len() as u64,
            sha256: input_hash.clone(),
        },
        executable: InputIdentity {
            bytes: binary.len() as u64,
            sha256: hash(&binary),
        },
        compiled_source_sha256: compiled_sources(),
        mode_contract: "Only doppler_frequency, doppler_timing and thunder_timing enable flags change; all candidates, evidence, weights, floors, associations, emitted frequencies, heights and clock priors stay fixed. Missing required support never falls back to another mode.",
        evaluation_labels_read: false,
        location_verified: false,
    };
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err("Create the output parent directory before running".into());
    }
    let serial = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let temporary = parent.join(format!(
        ".hybrid-ablation-{}-{serial}.tmp",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    let temporary = TemporaryOutput(temporary);
    let mut writer = BoundedWriter {
        file: BufWriter::with_capacity(256 * 1024, file),
        bytes: 0,
    };
    let mut prefix = serde_json::to_vec(&header).map_err(|e| e.to_string())?;
    if prefix.pop() != Some(b'}') {
        return Err("Invalid serialized report header".into());
    }
    writer.write_all(&prefix).map_err(|e| e.to_string())?;
    writer
        .write_all(b",\"cases\":[")
        .map_err(|e| e.to_string())?;
    for (index, case) in batch.cases.iter().enumerate() {
        if index > 0 {
            writer.write_all(b",").map_err(|e| e.to_string())?;
        }
        let result = score_case(case)?;
        serde_json::to_writer(&mut writer, &result).map_err(|e| e.to_string())?;
        eprintln!("Scored case {} of {}", index + 1, batch.cases.len());
    }
    writer.write_all(b"]}\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    writer
        .file
        .get_ref()
        .sync_all()
        .map_err(|e| e.to_string())?;
    drop(writer);
    if hash(&read_bounded(input)?) != input_hash {
        return Err("Scoring input changed during execution; report not published".into());
    }
    // Claim the destination exclusively, then atomically replace our empty
    // placeholder with the complete, synced report. Never truncate an old run.
    drop(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)
            .map_err(|e| e.to_string())?,
    );
    fs::rename(&temporary.0, output)
        .map_err(|e| format!("Could not publish complete report: {e}"))?;
    Ok(())
}

fn run() -> Result<(), String> {
    let mut input = None;
    let mut output = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--input" && input.is_none() {
            input = args.next().map(PathBuf::from);
        } else if argument == "--out" && output.is_none() {
            output = args.next().map(PathBuf::from);
        } else {
            return Err(
                "Usage: hybrid_ablation --input inputs.json --out NEW_predictions.json".into(),
            );
        }
    }
    execute(
        &input.ok_or("Missing --input")?,
        &output.ok_or("Missing --out")?,
    )
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Hybrid ablation failed: {error}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluation_truth_is_rejected_at_batch_and_case_boundaries() {
        for input in [
            r#"{"schema_version":1,"cases":[],"truth":{"latitude_deg":1}}"#,
            r#"{"schema_version":1,"cases":[{"truth":{"latitude_deg":1}}]}"#,
        ] {
            let error = serde_json::from_str::<Batch>(input).unwrap_err();
            assert!(error.to_string().contains("unknown field `truth`"));
        }
    }

    #[test]
    fn ablation_changes_only_enable_flags_preserving_declared_weights_and_floors() {
        let original = EvidenceMix {
            frequency_weight: 2.0,
            doppler_timing_weight: 3.0,
            thunder_weight: 5.0,
            frequency_scale_hz: 0.25,
            doppler_timing_scale_s: 1.5,
            thunder_scale_s: 7.0,
            ..Default::default()
        };
        for (mode, flags) in [
            (Mode::DopplerOnly, [true, true, false]),
            (Mode::ThunderOnly, [false, false, true]),
            (Mode::Combined, [true, true, true]),
        ] {
            let mut mix = original.clone();
            mode.apply(&mut mix);
            assert_eq!(
                [
                    mix.doppler_frequency,
                    mix.doppler_timing,
                    mix.thunder_timing
                ],
                flags
            );
            mix.doppler_frequency = false;
            mix.doppler_timing = false;
            mix.thunder_timing = false;
            assert_eq!(mix, original);
        }
    }

    #[test]
    fn existing_prediction_output_is_refused_without_reading_missing_input() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let temporary = root.join("tmp");
        fs::create_dir_all(&temporary).unwrap();
        let name = format!(
            "hybrid-existing-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = temporary.join(name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(b"Existing predictions must survive")
            .unwrap();
        drop(file);
        let outcome = execute(&temporary.join("nonexistent-input.json"), &path);
        let preserved = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(outcome.unwrap_err().contains("Output exists"));
        assert_eq!(preserved, b"Existing predictions must survive");
    }
}
