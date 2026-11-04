//! Whole-recording acoustic proposals. The input is audio, never location labels,
//! flight data, hand-selected bands or event times. Ported in part from MIT
//! DOPPLERGEIST 70626ad; see THIRD_PARTY_NOTICES.md. Proposals need human review.
use crate::{acquisition, audio, discovery_groups as groups};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{BufReader, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

pub const SAMPLE_RATE: usize = 4000;
const MAX_CHUNK_CELLS: usize = 4_000_000;
const MAX_RIDGE_POINTS: usize = 10_000_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RidgePoint {
    pub time_s: f64,
    pub frequency_hz: f64,
    pub weight: f64,
}
pub type Ridge = Vec<RidgePoint>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverySettings {
    pub chunk_s: f64,
    pub overlap_s: f64,
    pub min_frequency_hz: f64,
    pub max_frequency_hz: f64,
    pub window_s: f64,
    pub hop_s: f64,
    pub min_contrast: f64,
    pub min_duration_s: f64,
    pub min_support: f64,
    pub min_drop_hz: f64,
    pub min_fractional_drop: f64,
    pub max_gap_s: f64,
    pub max_fragment_gap_s: f64,
    pub max_peaks_per_frame: usize,
}
impl Default for DiscoverySettings {
    fn default() -> Self {
        Self {
            chunk_s: 120.,
            overlap_s: 60.,
            min_frequency_hz: 40.,
            max_frequency_hz: 1600.,
            window_s: 2.048,
            hop_s: 0.5,
            min_contrast: 2.5,
            min_duration_s: 12.,
            min_support: 0.65,
            min_drop_hz: 4.,
            min_fractional_drop: 0.03,
            max_gap_s: 2.,
            max_fragment_gap_s: 20.,
            max_peaks_per_frame: 60,
        }
    }
}
fn sample_count(seconds: f64) -> Result<usize, String> {
    let n = seconds * SAMPLE_RATE as f64;
    if !n.is_finite() || !(1. ..=2_400_000.).contains(&n) || (n - n.round()).abs() > 1e-6 {
        return Err(
            "Durations must be exact sample multiples at 4000 Hz, within 600 seconds".into(),
        );
    }
    Ok(n.round() as usize)
}
impl DiscoverySettings {
    pub fn validate(&self) -> Result<(), String> {
        let values = [
            self.chunk_s,
            self.overlap_s,
            self.min_frequency_hz,
            self.max_frequency_hz,
            self.window_s,
            self.hop_s,
            self.min_contrast,
            self.min_duration_s,
            self.min_support,
            self.min_drop_hz,
            self.min_fractional_drop,
            self.max_gap_s,
            self.max_fragment_gap_s,
        ];
        if values.iter().any(|v| !v.is_finite() || *v <= 0.) {
            return Err("Discovery settings must be finite and positive".into());
        }
        if self.chunk_s > 600.
            || self.overlap_s >= self.chunk_s
            || self.overlap_s < self.min_duration_s + self.window_s
            || self.hop_s > self.window_s
            || self.window_s >= self.chunk_s
            || self.min_support > 1.
            || self.min_fractional_drop >= 1.
            || self.max_gap_s < self.hop_s
            || self.max_fragment_gap_s < self.max_gap_s
        {
            return Err("Invalid discovery window, overlap, support or gap settings".into());
        }
        if self.min_frequency_hz >= self.max_frequency_hz
            || self.max_frequency_hz > 1920.
            || self.max_peaks_per_frame == 0
            || self.max_peaks_per_frame > 256
        {
            return Err(
                "Use a frequency range within 0..1920 Hz and 1..256 peaks per frame".into(),
            );
        }
        let n = sample_count(self.window_s)?;
        let hop = sample_count(self.hop_s)?;
        let chunk = sample_count(self.chunk_s)?;
        let overlap = sample_count(self.overlap_s)?;
        if !n.is_power_of_two() || !(1024..=32768).contains(&n) {
            return Err("Window must contain a power of two samples, 1024..32768 at 4000 Hz (default 2.048 seconds)".into());
        }
        if !(chunk - overlap).is_multiple_of(hop) {
            return Err("Chunk stride must be an exact multiple of the spectral hop".into());
        }
        let bins =
            ((self.max_frequency_hz - self.min_frequency_hz) * self.window_s).ceil() as usize + 1;
        if bins < 3
            || ((chunk - n) / hop + 1).saturating_mul(bins) > MAX_CHUNK_CELLS
            || (self.max_gap_s / self.hop_s + 1.) * self.max_peaks_per_frame as f64 > 4096.
        {
            return Err(
                "Discovery settings exceed the bounded spectral or live-path budget".into(),
            );
        }
        Ok(())
    }
}

pub(crate) fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.;
    }
    let mut v = values.to_vec();
    let m = v.len() / 2;
    v.select_nth_unstable_by(m, f64::total_cmp);
    if v.len() % 2 == 1 {
        v[m]
    } else {
        (v[m] + v[..m].iter().copied().max_by(f64::total_cmp).unwrap()) / 2.
    }
}
struct Spectrum {
    time: Vec<f64>,
    frequency: Vec<f64>,
    db: Vec<Vec<f64>>,
    contrast: Vec<Vec<f64>>,
}
fn spectrum(data: &[f32], offset_samples: usize, s: &DiscoverySettings) -> Option<Spectrum> {
    let n = (s.window_s * SAMPLE_RATE as f64).round() as usize;
    let hop = (s.hop_s * SAMPLE_RATE as f64).round() as usize;
    if data.len() < n {
        return None;
    }
    let first = (s.min_frequency_hz * n as f64 / SAMPLE_RATE as f64).ceil() as usize;
    let last = (s.max_frequency_hz * n as f64 / SAMPLE_RATE as f64).floor() as usize;
    let frequency = (first..=last)
        .map(|bin| bin as f64 * SAMPLE_RATE as f64 / n as f64)
        .collect::<Vec<_>>();
    let mut engine = audio::SpectralEngine::configured(n, s.max_frequency_hz);
    let mut db = Vec::new();
    let mut time = Vec::new();
    let mut window = vec![0_f32; n];
    for start in (0..=data.len() - n).step_by(hop) {
        let source = &data[start..start + n];
        let mean = source.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
        for (v, x) in window.iter_mut().zip(source) {
            *v = (*x as f64 - mean) as f32;
        }
        db.push(
            engine.power_db(&window)[first..=last]
                .iter()
                .map(|v| *v as f64)
                .collect::<Vec<_>>(),
        );
        time.push((offset_samples + start + n / 2) as f64 / SAMPLE_RATE as f64);
    }
    let mut contrast = db.clone();
    for bin in 0..frequency.len() {
        let baseline = median(&db.iter().map(|col| col[bin]).collect::<Vec<_>>());
        for col in &mut contrast {
            col[bin] -= baseline;
        }
    }
    for col in &mut contrast {
        let center = median(col);
        let scale =
            (1.4826 * median(&col.iter().map(|v| (v - center).abs()).collect::<Vec<_>>())).max(1.);
        for v in col {
            *v = ((*v - center) / scale).clamp(-4., 12.);
        }
    }
    Some(Spectrum {
        time,
        frequency,
        db,
        contrast,
    })
}

fn peaks(column: &[f64], threshold: f64, limit: usize) -> Vec<usize> {
    let mut candidates = Vec::new();
    let mut i = 1;
    while i + 1 < column.len() {
        if column[i] > column[i - 1] {
            let mut end = i;
            while end + 1 < column.len() && column[end + 1] == column[i] {
                end += 1;
            }
            if end + 1 < column.len() && column[end] > column[end + 1] && column[i] >= threshold {
                let peak = (i + end) / 2;
                let height = column[peak];
                let mut left = height;
                let mut right = height;
                for &v in column[..peak].iter().rev() {
                    if v > height {
                        break;
                    }
                    left = left.min(v);
                }
                for &v in &column[peak + 1..] {
                    if v > height {
                        break;
                    }
                    right = right.min(v);
                }
                if height - left.max(right) >= 1.5 {
                    candidates.push(peak);
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    candidates.sort_by(|&a, &b| column[b].total_cmp(&column[a]).then(a.cmp(&b)));
    let mut accepted = Vec::<usize>::new();
    for candidate in candidates {
        if accepted.iter().all(|other| candidate.abs_diff(*other) >= 2) {
            accepted.push(candidate);
            if accepted.len() == limit {
                break;
            }
        }
    }
    // Ascending strength matches the upstream path-creation order.
    accepted.reverse();
    accepted
}

/// Rectangular minimum-cost one-to-one assignment, not greedy peak matching.
/// Invalid edges retain a large cost and are discarded by the caller.
pub(crate) fn assignment(cost: &[Vec<f64>]) -> Vec<(usize, usize)> {
    if cost.is_empty() || cost[0].is_empty() {
        return vec![];
    }
    let transpose = cost.len() > cost[0].len();
    let (n, m) = if transpose {
        (cost[0].len(), cost.len())
    } else {
        (cost.len(), cost[0].len())
    };
    let value = |i: usize, j: usize| if transpose { cost[j][i] } else { cost[i][j] };
    let (mut u, mut v, mut p, mut way) = (
        vec![0.; n + 1],
        vec![0.; m + 1],
        vec![0_usize; m + 1],
        vec![0_usize; m + 1],
    );
    for row in 1..=n {
        p[0] = row;
        let mut j0 = 0;
        let mut minv = vec![f64::INFINITY; m + 1];
        let mut used = vec![false; m + 1];
        loop {
            used[j0] = true;
            let i0 = p[j0];
            let mut delta = f64::INFINITY;
            let mut j1 = 0;
            for j in 1..=m {
                if !used[j] {
                    let current = value(i0 - 1, j - 1) - u[i0] - v[j];
                    if current < minv[j] {
                        minv[j] = current;
                        way[j] = j0;
                    }
                    if minv[j] < delta {
                        delta = minv[j];
                        j1 = j;
                    }
                }
            }
            for j in 0..=m {
                if used[j] {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
            if p[j0] == 0 {
                break;
            }
        }
        loop {
            let j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
            if j0 == 0 {
                break;
            }
        }
    }
    (1..=m)
        .filter(|&j| p[j] != 0)
        .map(|j| {
            if transpose {
                (j - 1, p[j] - 1)
            } else {
                (p[j] - 1, j - 1)
            }
        })
        .collect()
}

fn paths(spec: &Spectrum, s: &DiscoverySettings) -> Vec<Ridge> {
    let mut active = Vec::<Ridge>::new();
    let mut completed = Vec::new();
    for (col, &time) in spec.time.iter().enumerate() {
        let bins = peaks(&spec.contrast[col], s.min_contrast, s.max_peaks_per_frame);
        let points = bins
            .iter()
            .map(|&bin| {
                let (a, b, c) = (
                    spec.db[col][bin - 1],
                    spec.db[col][bin],
                    spec.db[col][bin + 1],
                );
                let denominator = a - 2. * b + c;
                let offset = if denominator.abs() > 1e-9 {
                    (0.5 * (a - c) / denominator).clamp(-0.5, 0.5)
                } else {
                    0.
                };
                RidgePoint {
                    time_s: time,
                    frequency_hz: spec.frequency[bin]
                        + offset * (spec.frequency[1] - spec.frequency[0]),
                    weight: spec.contrast[col][bin],
                }
            })
            .collect::<Vec<_>>();
        let mut live = Vec::new();
        for path in active {
            if time - path.last().unwrap().time_s <= s.max_gap_s + 1e-9 {
                live.push(path);
            } else {
                completed.push(path);
            }
        }
        active = live;
        let mut used = vec![false; points.len()];
        if !active.is_empty() && !points.is_empty() {
            let costs = active
                .iter()
                .map(|path| {
                    let last = path.last().unwrap();
                    let dt = time - last.time_s;
                    let tail = &path[path.len().saturating_sub(8)..];
                    let slope = if tail.len() >= 3 {
                        median(
                            &tail
                                .windows(2)
                                .map(|p| {
                                    (p[1].frequency_hz - p[0].frequency_hz)
                                        / (p[1].time_s - p[0].time_s)
                                })
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        0.
                    };
                    points
                        .iter()
                        .map(|point| {
                            let delta = point.frequency_hz - last.frequency_hz;
                            let speed = delta / dt;
                            if speed >= -(8_f64.max(last.frequency_hz * 0.08))
                                && speed <= 1_f64.max(last.frequency_hz * 0.008)
                            {
                                (delta - slope * dt).abs() / 1_f64.max(last.frequency_hz * 0.01)
                            } else {
                                1e6
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            for (row, col) in assignment(&costs) {
                if costs[row][col] < 1e6 {
                    active[row].push(points[col]);
                    used[col] = true;
                }
            }
        }
        for (i, point) in points.into_iter().enumerate() {
            if !used[i] {
                active.push(vec![point]);
            }
        }
    }
    completed.extend(active);
    completed
}

fn event(
    points: &[RidgePoint],
    group: usize,
    harmonic: usize,
    count: usize,
    duration: f64,
    s: &DiscoverySettings,
) -> Value {
    let frequency = points.iter().map(|p| p.frequency_hz).collect::<Vec<_>>();
    let mut quality = serde_json::to_value(
        groups::quality(points, s).expect("Only validated measured ridges are reported"),
    )
    .expect("Validated finite ridge quality");
    quality["harmonic_count"] = json!(count);
    let first = points[0].time_s;
    let last = points.last().unwrap().time_s;
    let id = if harmonic == 0 {
        format!("event-{:04}", group + 1)
    } else {
        format!("event-{:04}-harmonic-{}", group + 1, harmonic + 1)
    };
    json!({"event_id":id,"harmonic_group_id":format!("harmonics-{:04}",group+1),"start_s":(first-s.window_s/2.).max(0.),"end_s":(last+s.window_s/2.).min(duration),
        "band_hz":[(frequency.iter().copied().min_by(f64::total_cmp).unwrap()-2./s.window_s).max(0.01),frequency.iter().copied().max_by(f64::total_cmp).unwrap()+2./s.window_s],"ridge":points,
        "quality":quality})
}

fn write_new(path: &Path, data: &[u8]) -> Result<(), String> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())
}

fn review_html(
    report: &Value,
    original_json: &str,
    discovery_hash: &str,
) -> Result<String, String> {
    let finite = |value: &Value| value.as_f64().filter(|v| v.is_finite());
    fn safe_id(value: &Value) -> Option<&str> {
        value.as_str().filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
    }
    if report["schema_version"] != 1
        || report["location_verified"] != false
        || !matches!(
            report["status"].as_str(),
            Some("events_proposed" | "no_events")
        )
        || report["source"].as_str().is_none()
        || report["source_sha256"]
            .as_str()
            .is_none_or(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        || report["source_metadata"]["selected_channel_index"]
            .as_u64()
            .is_none()
    {
        return Err("Review requires an unverified schema-1 discovery report with source and channel identity".into());
    }
    let duration = finite(&report["source_metadata"]["duration_s"])
        .filter(|v| *v > 0.)
        .ok_or("Review needs a positive recording duration")?;
    let settings = &report["settings"];
    let low = finite(&settings["min_frequency_hz"]).ok_or("Missing review frequency bounds")?;
    let high = finite(&settings["max_frequency_hz"]).ok_or("Missing review frequency bounds")?;
    if low < 0. || low >= high || finite(&settings["max_gap_s"]).is_none_or(|v| v <= 0.) {
        return Err("Review frequency/gap bounds must be finite and ordered".into());
    }
    let groups = report["groups"].as_array().ok_or("Review needs groups")?;
    let events = report["events"].as_array().ok_or("Review needs events")?;
    if report["group_count"].as_u64() != Some(groups.len() as u64)
        || report["ridge_count"].as_u64() != Some(events.len() as u64)
        || (report["status"] == "no_events") != events.is_empty()
        || events.len() > 100_000
    {
        return Err("Review counts or status disagree with the discovery measurements".into());
    }
    let mut membership = HashMap::new();
    for group in groups {
        let id = safe_id(&group["harmonic_group_id"]).ok_or("Invalid review group identity")?;
        let start = finite(&group["start_s"]).ok_or("Invalid review group start")?;
        let end = finite(&group["end_s"]).ok_or("Invalid review group end")?;
        if membership.insert(id, (group, 0_usize)).is_some()
            || start < 0.
            || start >= end
            || end > duration + 1e-6
            || group["review_status"] != "unreviewed"
            || group["aircraft_identity_confirmed"] != false
        {
            return Err(
                "Review groups need unique identities, valid bounds and unreviewed status".into(),
            );
        }
    }
    let mut identities = HashSet::new();
    let mut point_count = 0;
    for event in events {
        let id = safe_id(&event["event_id"]).ok_or("Invalid review event identity")?;
        let group_id = safe_id(&event["harmonic_group_id"]).ok_or("Invalid review event group")?;
        let (group, count) = membership
            .get_mut(group_id)
            .ok_or("Review event references an unknown group")?;
        if !identities.insert(id) {
            return Err("Review event identities must be unique".into());
        }
        *count += 1;
        let band = event["band_hz"]
            .as_array()
            .filter(|v| v.len() == 2)
            .ok_or("Review event needs two frequency bounds")?;
        let band_low = finite(&band[0]).ok_or("Invalid review event band")?;
        let band_high = finite(&band[1]).ok_or("Invalid review event band")?;
        let points = event["ridge"]
            .as_array()
            .filter(|p| !p.is_empty())
            .ok_or("Review event needs measured ridge points")?;
        if band_low <= 0.
            || band_low >= band_high
            || finite(&event["quality"]["support_fraction"])
                .is_none_or(|v| !(0. ..=1.).contains(&v))
            || event["quality"]["fragment_count"]
                .as_u64()
                .is_none_or(|v| v == 0)
        {
            return Err("Invalid review ridge quality or frequency bounds".into());
        }
        let start = group["start_s"].as_f64().unwrap();
        let end = group["end_s"].as_f64().unwrap();
        let mut previous = -1.;
        for point in points {
            let time = finite(&point["time_s"]).ok_or("Invalid review sample time")?;
            let frequency =
                finite(&point["frequency_hz"]).ok_or("Invalid review sample frequency")?;
            if time <= previous
                || time < start
                || time > end
                || frequency < band_low
                || frequency > band_high
                || finite(&point["weight"]).is_none_or(|v| v <= 0.)
            {
                return Err(
                    "Review samples must be finite, positive, ordered and within their bounds"
                        .into(),
                );
            }
            previous = time;
        }
        point_count += points.len();
        if point_count > MAX_RIDGE_POINTS {
            return Err("Review exceeds the discovery measurement budget".into());
        }
    }
    if membership
        .values()
        .any(|(g, n)| *n == 0 || g["ridge_count"].as_u64() != Some(*n as u64))
    {
        return Err("Review group membership counts disagree".into());
    }
    // Embed the original decimal tokens, not a parse/reserialize round trip.
    // This preserves every measurement even with alternative JSON encodings.
    let serialized = original_json
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    Ok(include_str!("discovery_review.html")
        .replace("__DISCOVERY_SHA256__", discovery_hash)
        .replace("__REPORT_JSON__", &serialized))
}

/// Regenerate a review page without changing or rerunning discovery. Choices
/// start unreviewed and exports bind the exact original discovery-file bytes.
pub fn write_review(discovery_path: &Path, output_html: &Path) -> Result<(), String> {
    if output_html.exists() {
        return Err("Use a new review HTML path; existing reviews are never overwritten".into());
    }
    let before = acquisition::sha256_file(discovery_path)?;
    let original_json = fs::read_to_string(discovery_path).map_err(|e| e.to_string())?;
    let report =
        serde_json::from_str(&original_json).map_err(|e| format!("Invalid discovery JSON: {e}"))?;
    let html = review_html(&report, &original_json, &before)?;
    if acquisition::sha256_file(discovery_path)? != before {
        return Err("Discovery file changed while preparing its review".into());
    }
    if let Some(parent) = output_html.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_new(output_html, html.as_bytes())
}

/// Scan a complete audio stream, with bounded spectral memory and no audio hints.
/// Channel selection is explicit. The result is always unverified and reviewable.
pub fn discover(
    input: &Path,
    output: &Path,
    channel: usize,
    settings: &DiscoverySettings,
) -> Result<Value, String> {
    settings.validate()?;
    if !input.is_file() {
        return Err(format!("Recording not found: {}", input.display()));
    }
    if output.exists() {
        return Err(
            "Discovery requires a new output folder; existing results are never overwritten".into(),
        );
    }
    let available = audio::channel_count(input)?;
    if channel >= available {
        return Err(format!(
            "Requested channel {channel}, but only {available} channel(s) exist"
        ));
    }
    let source_hash = acquisition::sha256_file(input)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::create_dir(output).map_err(|e| e.to_string())?;
    let result = discover_inner(input, output, channel, available, settings, &source_hash);
    if let Err(error) = &result {
        // This invocation created a fresh directory. Remove only its named
        // completion artifacts, so a failed final write cannot leave a review
        // that appears to describe a completed run. Preserve diagnostic CSVs.
        let mut cleanup_errors = Vec::new();
        for name in ["discovery.json", "review.html"] {
            let path = output.join(name);
            if path.is_file()
                && let Err(e) = fs::remove_file(&path)
            {
                cleanup_errors.push(format!("{}: {e}", path.display()));
            }
        }
        let failure = json!({"schema_version":1,"status":"failed","location_verified":false,"error":error,"source_sha256":source_hash,"settings":settings,"completion_artifact_cleanup_errors":cleanup_errors});
        let _ = write_new(
            &output.join("failure.json"),
            serde_json::to_string_pretty(&failure).unwrap().as_bytes(),
        );
    }
    result
}
fn discover_inner(
    input: &Path,
    output: &Path,
    channel: usize,
    available: usize,
    s: &DiscoverySettings,
    source_hash: &str,
) -> Result<Value, String> {
    let mut command = Command::new(audio::resolve_media_tool("ffmpeg")?);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-xerror",
            "-err_detect",
            "explode",
            "-i",
        ])
        .arg(input)
        .args(["-map", "0:a:0", "-vn", "-af"])
        .arg(format!("pan=mono|c0=c{channel}"))
        .args(["-ar", "4000", "-c:a", "pcm_f32le", "-f", "f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let mut stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut out = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n.min(65536_usize.saturating_sub(out.len()))]);
        }
        out
    });
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut found = Vec::<Ridge>::new();
    let mut chunks = Vec::new();
    let mut decoded = 0;
    let calculation = (|| -> Result<(), String> {
        let chunk = sample_count(s.chunk_s)?;
        let overlap = sample_count(s.overlap_s)?;
        let stride = chunk - overlap;
        let mut data = vec![0_f32; chunk];
        let mut count = audio::read_samples(&mut reader, &mut data)?;
        decoded = count;
        let mut offset = 0;
        loop {
            let mut raw = 0;
            let mut kept = 0;
            if let Some(spec) = spectrum(&data[..count], offset, s) {
                let tracked = paths(&spec, s);
                raw = tracked.len();
                for path in tracked {
                    if groups::usable(&path, s) {
                        found.push(path);
                        kept += 1;
                    }
                }
            }
            chunks.push(json!({"start_s":offset as f64/SAMPLE_RATE as f64,"duration_s":count as f64/SAMPLE_RATE as f64,"tracked_paths":raw,"usable_paths":kept}));
            if found.len() > 100_000 || found.iter().map(Vec::len).sum::<usize>() > MAX_RIDGE_POINTS
            {
                return Err("Discovery output budget exceeded; split the recording explicitly rather than silently truncating detections".into());
            }
            if count < chunk {
                break;
            }
            data.copy_within(stride..chunk, 0);
            let read = audio::read_samples(&mut reader, &mut data[overlap..])?;
            decoded += read;
            if decoded > 86_400 * SAMPLE_RATE {
                return Err("Discovery is limited to 24 hours per recording".into());
            }
            if read == 0 {
                break;
            }
            count = overlap + read;
            offset += stride;
        }
        if decoded == 0 {
            return Err("Decoded audio is empty".into());
        }
        Ok(())
    })();
    drop(reader);
    if calculation.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let stderr = errors.join().map_err(|_| "Cannot collect decoder errors")?;
    calculation?;
    if !status.success() || !stderr.is_empty() {
        return Err(format!(
            "FFmpeg failed: {}",
            String::from_utf8_lossy(&stderr)
        ));
    }
    if acquisition::sha256_file(input)? != source_hash {
        return Err("Source recording changed while decoding".into());
    }
    let duration = decoded as f64 / SAMPLE_RATE as f64;
    let merged = groups::merge_duplicates(found);
    let (stitched, fragment_links) = groups::stitch_fragments(merged, s);
    let ridge_count = stitched.len();
    let grouped = groups::harmonic_groups(stitched);
    let mut events = Vec::new();
    for (i, group) in grouped.iter().enumerate() {
        for (j, points) in group.iter().enumerate() {
            events.push(event(points, i, j, group.len(), duration, s));
        }
    }
    let group_reports=grouped.iter().enumerate().map(|(i,group)|json!({"harmonic_group_id":format!("harmonics-{:04}",i+1),"start_s":group.iter().map(|p|(p[0].time_s-s.window_s/2.).max(0.)).fold(f64::INFINITY,f64::min),"end_s":group.iter().map(|p|(p.last().unwrap().time_s+s.window_s/2.).min(duration)).fold(0.,f64::max),"ridge_count":group.len(),"review_status":"unreviewed","aircraft_identity_confirmed":false})).collect::<Vec<_>>();
    let report = json!({"schema_version":1,"status":if events.is_empty(){"no_events"}else{"events_proposed"},"location_verified":false,"scope":"automatic acoustic moving-tone proposals; aircraft identity unconfirmed; human review required","upstream_commit":"70626ad44fb9c7f4e2f5b225956c1f9281f91c13","source":input.canonicalize().map_err(|e|e.to_string())?,"source_sha256":source_hash,"source_metadata":{"selected_audio_stream":"0:a:0","selected_channel_index":channel,"available_channels":available,"channels_averaged":false,"sample_rate_hz":SAMPLE_RATE,"decoded_samples":decoded,"duration_s":duration,"time_reference":"seconds from decoded input start; no UTC inferred"},"settings":s,"spectral_method":{"window":"periodic Hann","detrending":"per-frame mean removed before windowing","fft_size":(s.window_s*SAMPLE_RATE as f64).round() as usize,"padding":false,"upstream_difference":"Native radix-2 default window is 2.048s rather than 2.0s; all time/frequency uncertainty uses actual window"},"events":events,"groups":group_reports,"group_count":grouped.len(),"ridge_count":ridge_count,"chunks":chunks,"fragment_links":fragment_links,"limitations":["Downward moving tones are not proof of an aircraft; electronic sounds and other moving sources can resemble flyovers","Harmonic grouping and fragment links are acoustic hypotheses, not flight identities","Nontonal, faint or masked aircraft may be missed; a quiet interval is not proof no aircraft passed","No location, date or recording clock can be inferred from discovery alone"]});
    for e in report["events"].as_array().unwrap() {
        let mut csv = String::from("time_s,frequency_hz,weight\n");
        for p in e["ridge"].as_array().unwrap() {
            csv.push_str(&format!(
                "{},{},{}\n",
                p["time_s"], p["frequency_hz"], p["weight"]
            ));
        }
        write_new(
            &output.join(format!("{}.csv", e["event_id"].as_str().unwrap())),
            csv.as_bytes(),
        )?;
    }
    write_new(
        &output.join("discovery.json"),
        serde_json::to_string_pretty(&report)
            .map_err(|e| e.to_string())?
            .as_bytes(),
    )?;
    write_review(&output.join("discovery.json"), &output.join("review.html"))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assignment_is_global_one_to_one_and_handles_rectangles() {
        let cost = vec![vec![1., 2.], vec![1.1, 100.]];
        let result = assignment(&cost);
        assert_eq!(result.iter().map(|(r, c)| cost[*r][*c]).sum::<f64>(), 3.1);
        assert_eq!(assignment(&[vec![2., 1., 3.]]), vec![(0, 1)]);
        assert_eq!(assignment(&[vec![2.], vec![1.], vec![3.]]), vec![(1, 0)]);
        assert!(assignment(&[]).is_empty());
    }
    #[test]
    fn spectral_frame_centers_and_bins_are_physical() {
        let s = DiscoverySettings::default();
        let signal = (0..12000)
            .map(|i| {
                (2. * std::f64::consts::PI * 125. * i as f64 / SAMPLE_RATE as f64).sin() as f32
            })
            .collect::<Vec<_>>();
        let spec = spectrum(&signal, 40000, &s).unwrap();
        assert_eq!(spec.time, vec![11.024, 11.524]);
        let peak = spec.db[0]
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        assert_eq!(spec.frequency[peak], 125.);
    }
    #[test]
    fn invalid_settings_fail_before_allocating_or_decoding() {
        let base = DiscoverySettings::default();
        base.validate().unwrap();
        for i in 0..6 {
            let mut s = base.clone();
            match i {
                0 => s.window_s = 2.,
                1 => s.hop_s = 0.00025,
                2 => s.overlap_s = 120.,
                3 => s.max_frequency_hz = 2100.,
                4 => s.min_support = 1.1,
                _ => s.max_gap_s = 0.1,
            }
            assert!(s.validate().is_err());
        }
    }
}
