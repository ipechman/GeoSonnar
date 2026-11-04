//! Immutable, disk-backed native trajectories. Display sampling never changes
//! these original f64 points or the normalized pass/row provenance.
use crate::{
    adsblol::{TraceImportConfig, TraceImportReport, TraceImportStats, TracePointSource},
    physics::{Track, TrackPoint},
};
#[cfg(test)]
use flate2::{Compression, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(test)]
use std::{
    fs::OpenOptions,
    io::{self, Write},
};

const MAX_BLOCK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;
const POINTS_PER_BLOCK: usize = 65536;
const MAX_PASS_POINTS: usize = 1_000_000;
pub const DEFAULT_INDEX_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
pub const INDEX_MEMORY_LIMIT_PREFIX: &str = "Track-store index memory budget exceeded";

pub fn is_memory_limit_error(message: &str) -> bool {
    message.starts_with(INDEX_MEMORY_LIMIT_PREFIX)
}

pub fn validate_index_memory_limit(bytes: u64) -> Result<(), String> {
    if !(64 * 1024 * 1024..=4 * 1024 * 1024 * 1024).contains(&bytes) {
        return Err(format!("{INDEX_MEMORY_LIMIT_PREFIX}: declare64MiB..4GiB"));
    }
    Ok(())
}

#[path = "track_store_packed.rs"]
mod packed;
pub(crate) use packed::TrackStoreWriter;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrackStoreBinding {
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRef {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
    pub bytes: u64,
    pub sha256: String,
    pub first_sample: usize,
    pub point_count: usize,
    pub provenance: StoreFile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreFile {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub offset: u64,
    pub bytes: u64,
    pub sha256: String,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassMetadata {
    pub id: String,
    pub point_count: usize,
    pub start_s: f64,
    pub end_s: f64,
    /// Conservative bounds of every linear segment, including block seams.
    pub ecef_min: [f64; 3],
    pub ecef_max: [f64; 3],
    pub blocks: Vec<BlockRef>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    status: String,
    config: TraceImportConfig,
    source: Value,
    stats: TraceImportStats,
    passes: Vec<PassMetadata>,
    stored_bytes: u64,
    location_verified: bool,
    point_encoding: String,
}

#[derive(Serialize)]
struct StoredBlock<'a> {
    schema_version: u32,
    pass_id: &'a str,
    first_sample: usize,
    source_points: &'a [TracePointSource],
    source: &'a Value,
}

fn ordinary(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Track store refuses reparse points".into());
        }
    }
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Track store needs an ordinary file".into());
    }
    Ok(metadata)
}

fn child(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    if relative.components().count() != 1
        || !matches!(relative.components().next(), Some(Component::Normal(_)))
        || relative.to_string_lossy().contains(':')
    {
        return Err("Track block must be a direct owned relative file".into());
    }
    let path = root.join(relative);
    ordinary(&path)?;
    Ok(path)
}

fn bounded_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    bounded_bytes_with_checkpoint(path, limit, &mut || Ok(()))
}

fn bounded_bytes_with_checkpoint(
    path: &Path,
    limit: u64,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<Vec<u8>, String> {
    checkpoint()?;
    let declared = ordinary(path)?.len();
    if declared > limit {
        return Err("Track store file exceeds byte limit".into());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(declared as usize)
        .map_err(|error| format!("{INDEX_MEMORY_LIMIT_PREFIX}: {error}"))?;
    let mut reader = File::open(path)
        .map_err(|e| e.to_string())?
        .take(declared + 1);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        checkpoint()?;
        let count = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        checkpoint()?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > declared {
            return Err("Track store file grew beyond its byte limit".into());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() as u64 != declared {
        return Err("Track store file changed length during read".into());
    }
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn digest_with_checkpoint(
    bytes: &[u8],
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    for chunk in bytes.chunks(1024 * 1024) {
        checkpoint()?;
        hash.update(chunk);
    }
    checkpoint()?;
    Ok(format!("{:x}", hash.finalize()))
}

fn decode_point(row: &[u8]) -> Result<TrackPoint, String> {
    if row.len() != 32 {
        return Err("Invalid native point record length".into());
    }
    let mut values = [0.; 4];
    for (index, value) in values.iter_mut().enumerate() {
        *value = f64::from_le_bytes(
            row[index * 8..index * 8 + 8]
                .try_into()
                .map_err(|_| "Invalid native point record")?,
        );
    }
    if values.iter().any(|x| !x.is_finite()) {
        return Err("Nonfinite native point record".into());
    }
    Ok(TrackPoint {
        t_s: values[0],
        xyz_m: [values[1], values[2], values[3]],
    })
}

#[cfg(test)]
struct BoundedWriter {
    file: File,
    bytes: u64,
    maximum: u64,
    hash: Sha256,
}
#[cfg(test)]
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() as u64 > self.maximum.saturating_sub(self.bytes) {
            return Err(io::Error::other("Track store disk/block budget exceeded"));
        }
        let n = self.file.write(bytes)?;
        self.bytes += n as u64;
        self.hash.update(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
struct LegacyTrackStoreWriter {
    root: PathBuf,
    limit: u64,
    written: u64,
    block_count: usize,
    config: TraceImportConfig,
    source: Value,
    stats: TraceImportStats,
    passes: Vec<PassMetadata>,
    ids: BTreeSet<String>,
    metadata_bytes: u64,
}

#[cfg(test)]
impl LegacyTrackStoreWriter {
    pub(crate) fn new(
        root: &Path,
        config: &TraceImportConfig,
        source: Value,
        limit: u64,
    ) -> Result<Self, String> {
        crate::adsblol::validate_config(config)?;
        if !(1024 * 1024..=1_000_000_000_000).contains(&limit) {
            return Err("Declare a track-store disk budget between1MiB and1TB".into());
        }
        Ok(Self {
            root: root.into(),
            limit,
            written: 0,
            block_count: 0,
            config: config.clone(),
            source,
            stats: TraceImportStats::default(),
            passes: Vec::new(),
            ids: BTreeSet::new(),
            metadata_bytes: 0,
        })
    }

    pub(crate) fn push(
        &mut self,
        report: TraceImportReport,
        member: Value,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        if report.config != self.config || report.schema_version != 1 || report.location_verified {
            return Err("Track report differs from indexed import policy".into());
        }
        if report.stats.output_passes != report.passes.len()
            || report.stats.output_points
                != report
                    .passes
                    .iter()
                    .map(|p| p.track.points.len())
                    .sum::<usize>()
            || report.stats.source_traces != report.sources.len()
            || report.stats.selected_traces
                != report
                    .sources
                    .iter()
                    .filter(|s| s.selected_by_region)
                    .count()
        {
            return Err("Normalized report counts disagree with retained source data".into());
        }
        self.stats.add(&report.stats);
        for pass in report.passes {
            let count = pass.track.points.len();
            if !(2..=MAX_PASS_POINTS).contains(&count)
                || pass.source_points.len() != count
                || !self.ids.insert(pass.track.flight_id.clone())
            {
                return Err("Track-store pass has invalid count or duplicate identity".into());
            }
            let mut minimum = [f64::INFINITY; 3];
            let mut maximum = [f64::NEG_INFINITY; 3];
            let mut previous = f64::NEG_INFINITY;
            for point in &pass.track.points {
                if !point.t_s.is_finite()
                    || point.t_s <= previous
                    || point.xyz_m.iter().any(|v| !v.is_finite())
                {
                    return Err(
                        "Track-store points must preserve finite increasing native support".into(),
                    );
                }
                previous = point.t_s;
                for (axis, coordinate) in point.xyz_m.iter().copied().enumerate() {
                    minimum[axis] = minimum[axis].min(coordinate);
                    maximum[axis] = maximum[axis].max(coordinate);
                }
            }
            let mut metadata = PassMetadata {
                id: pass.track.flight_id.clone(),
                point_count: count,
                start_s: pass.track.points[0].t_s,
                end_s: pass.track.points[count - 1].t_s,
                ecef_min: minimum,
                ecef_max: maximum,
                blocks: Vec::new(),
            };
            for first in (0..count).step_by(POINTS_PER_BLOCK) {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Indexed import cancelled before block publication".into());
                }
                let end = (first + POINTS_PER_BLOCK).min(count);
                let relative = PathBuf::from(format!("block-{:08}.bin", self.block_count));
                let mut writer = BoundedWriter {
                    file: OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(self.root.join(&relative))
                        .map_err(|e| e.to_string())?,
                    bytes: 0,
                    maximum: MAX_BLOCK_BYTES.min(self.limit.saturating_sub(self.written)),
                    hash: Sha256::new(),
                };
                let mut point_bytes = Vec::with_capacity((end - first) * 32);
                for point in &pass.track.points[first..end] {
                    for value in [point.t_s, point.xyz_m[0], point.xyz_m[1], point.xyz_m[2]] {
                        point_bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
                writer.write_all(&point_bytes).map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                writer.file.sync_all().map_err(|e| e.to_string())?;
                self.written += writer.bytes;
                let physical_bytes = writer.bytes;
                let physical_hash = format!("{:x}", writer.hash.finalize());
                let provenance_path =
                    PathBuf::from(format!("block-{:08}.provenance.json.gz", self.block_count));
                let provenance_writer = BoundedWriter {
                    file: OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(self.root.join(&provenance_path))
                        .map_err(|e| e.to_string())?,
                    bytes: 0,
                    maximum: MAX_BLOCK_BYTES.min(self.limit.saturating_sub(self.written)),
                    hash: Sha256::new(),
                };
                let mut encoder = GzEncoder::new(provenance_writer, Compression::fast());
                let trace_source = report
                    .sources
                    .iter()
                    .find(|source| source.icao24 == pass.icao24)
                    .ok_or("Normalized pass has no source metadata")?;
                let source = json!({"archive_member":member,"trace_source":trace_source,
                    "provider":report.provider,"coordinate_frame":report.coordinate_frame,
                    "timestamp_definition":report.timestamp_definition,"quality_notes":report.quality_notes,
                    "icao24":pass.icao24,"source_label":pass.source_label,"source_timestamp_utc_s":pass.source_timestamp_utc_s});
                serde_json::to_writer(
                    &mut encoder,
                    &StoredBlock {
                        schema_version: 1,
                        pass_id: &metadata.id,
                        first_sample: first,
                        source_points: &pass.source_points[first..end],
                        source: &source,
                    },
                )
                .map_err(|e| e.to_string())?;
                let mut provenance = encoder.finish().map_err(|e| e.to_string())?;
                provenance.flush().map_err(|e| e.to_string())?;
                provenance.file.sync_all().map_err(|e| e.to_string())?;
                self.written += provenance.bytes;
                metadata.blocks.push(BlockRef {
                    path: relative,
                    offset: 0,
                    bytes: physical_bytes,
                    sha256: physical_hash,
                    first_sample: first,
                    point_count: end - first,
                    provenance: StoreFile {
                        path: provenance_path,
                        offset: 0,
                        bytes: provenance.bytes,
                        sha256: format!("{:x}", provenance.hash.finalize()),
                    },
                });
                self.block_count += 1;
            }
            self.metadata_bytes += serde_json::to_vec(&metadata)
                .map_err(|e| e.to_string())?
                .len() as u64;
            if self.metadata_bytes > MAX_MANIFEST_BYTES - 1024 * 1024 {
                return Err("Track-store index metadata budget exceeded".into());
            }
            self.passes.push(metadata);
        }
        Ok(())
    }

    pub(crate) fn finish(mut self, cancel: &AtomicBool) -> Result<Value, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Indexed import cancelled before manifest publication".into());
        }
        self.passes.sort_by(|a, b| a.id.cmp(&b.id));
        let manifest = Manifest {
            schema_version: 1,
            status: "complete".into(),
            config: self.config,
            source: self.source,
            stats: self.stats,
            passes: self.passes,
            stored_bytes: self.written,
            location_verified: false,
            point_encoding:
                "le-f64-t_s-x_m-y_m-z_m;32bytes;no-resampling;gzip-json-source-provenance".into(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES || self.written + bytes.len() as u64 > self.limit
        {
            return Err("Track-store manifest exceeds its metadata/disk budget".into());
        }
        let path = self.root.join("index.json");
        let pending = self.root.join("index.pending.json");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        if cancel.load(Ordering::Relaxed) {
            return Err("Indexed import cancelled before manifest publication".into());
        }
        if path.exists() {
            return Err("Completed track index already exists".into());
        }
        fs::rename(&pending, &path).map_err(|e| e.to_string())?;
        Ok(
            json!({"schema_version":1,"status":"indexed_tracks_imported","archive_import_complete":true,
            "manifest":{"path":path,"bytes":bytes.len(),"sha256":digest(&bytes)},"stats":manifest.stats,
            "pass_count":manifest.passes.len(),"stored_bytes":self.written+bytes.len()as u64,
            "real_world_coverage_complete":false,"location_verified":false}),
        )
    }
}

pub struct TrackStore {
    binding: TrackStoreBinding,
    root: PathBuf,
    manifest: Manifest,
    manifest_bytes: u64,
    packed_files: Vec<StoreFile>,
    resident_memory_bytes: u64,
}
impl TrackStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_checkpoint(path, || Ok(()))
    }

    /// Verify the complete index and every source block with cooperative stop
    /// checks. A callback error is returned unchanged; no partial store escapes.
    pub fn open_with_checkpoint(
        path: &Path,
        checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        Self::open_with_memory_limit(path, DEFAULT_INDEX_MEMORY_BYTES, checkpoint)
    }

    pub fn open_with_memory_limit(
        path: &Path,
        memory_limit: u64,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        validate_index_memory_limit(memory_limit)?;
        checkpoint()?;
        let size = ordinary(path)?.len();
        if size > MAX_MANIFEST_BYTES {
            return Err("Track store file exceeds byte limit".into());
        }
        packed::memory_check(
            size.saturating_mul(32).saturating_add(1024 * 1024),
            memory_limit,
        )?;
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        let bytes = bounded_bytes_with_checkpoint(&path, size, &mut checkpoint)?;
        checkpoint()?;
        #[derive(Deserialize)]
        struct Version {
            schema_version: u32,
        }
        let version: Version = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        checkpoint()?;
        if version.schema_version == 2 {
            return packed::open(path, bytes, memory_limit, &mut checkpoint);
        }
        Self::open_v1(path, bytes, memory_limit, &mut checkpoint)
    }

    fn open_v1(
        path: PathBuf,
        bytes: Vec<u8>,
        memory_limit: u64,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<Self, String> {
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        checkpoint()?;
        let retained_memory =
            packed::manifest_memory(&manifest).saturating_add(path.capacity() as u64 * 4 + 4096);
        let largest_pair = manifest
            .passes
            .iter()
            .flat_map(|pass| &pass.blocks)
            .map(|block| block.bytes.saturating_add(block.provenance.bytes))
            .max()
            .unwrap_or(0);
        let resident_memory_bytes = retained_memory
            .saturating_add(bytes.capacity() as u64)
            .saturating_add(largest_pair)
            .saturating_add(1024 * 1024);
        let set_entries = manifest.passes.len() as u64
            + manifest
                .passes
                .iter()
                .map(|pass| pass.blocks.len() as u64 * 2)
                .sum::<u64>();
        packed::memory_check(
            resident_memory_bytes.saturating_add(set_entries.saturating_mul(128)),
            memory_limit,
        )?;
        crate::adsblol::validate_config(&manifest.config)?;
        if manifest.schema_version != 1
            || manifest.status != "complete"
            || manifest.location_verified
            || manifest.point_encoding
                != "le-f64-t_s-x_m-y_m-z_m;32bytes;no-resampling;gzip-json-source-provenance"
        {
            return Err("Only a completed native track store can be opened".into());
        }
        let root = path
            .parent()
            .ok_or("Track store has no directory")?
            .to_path_buf();
        let mut ids = BTreeSet::new();
        let mut blocks = BTreeSet::new();
        let mut points = 0usize;
        let mut stored = 0u64;
        for pass in &manifest.passes {
            checkpoint()?;
            if pass.id.is_empty()
                || pass.id.len() > 256
                || !ids.insert(&pass.id)
                || !(2..=MAX_PASS_POINTS).contains(&pass.point_count)
                || !pass.start_s.is_finite()
                || !pass.end_s.is_finite()
                || pass.start_s >= pass.end_s
                || (0..3).any(|i| {
                    !pass.ecef_min[i].is_finite()
                        || !pass.ecef_max[i].is_finite()
                        || pass.ecef_min[i] > pass.ecef_max[i]
                })
            {
                return Err("Invalid indexed trajectory metadata".into());
            }
            let mut end = 0;
            let mut previous = f64::NEG_INFINITY;
            let mut actual_min = [f64::INFINITY; 3];
            let mut actual_max = [f64::NEG_INFINITY; 3];
            for block in &pass.blocks {
                checkpoint()?;
                if !blocks.insert(&block.path)
                    || block.offset != 0
                    || block.provenance.offset != 0
                    || block.first_sample != end
                    || !(1..=POINTS_PER_BLOCK).contains(&block.point_count)
                    || block.bytes != (block.point_count as u64) * 32
                    || block.bytes > MAX_BLOCK_BYTES
                    || block.sha256.len() != 64
                    || !block.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err("Invalid track block inventory".into());
                }
                let block_path = child(&root, &block.path)?;
                if ordinary(&block_path)?.len() != block.bytes {
                    return Err("Track block size changed".into());
                }
                packed::memory_check(
                    retained_memory
                        .saturating_add(bytes.capacity() as u64)
                        .saturating_add(block.bytes)
                        .saturating_add(block.provenance.bytes),
                    memory_limit,
                )?;
                let native_bytes =
                    bounded_bytes_with_checkpoint(&block_path, block.bytes, &mut checkpoint)?;
                if digest_with_checkpoint(&native_bytes, &mut checkpoint)? != block.sha256 {
                    return Err("Track block checksum changed".into());
                }
                for (sample, row) in native_bytes.chunks_exact(32).enumerate() {
                    if sample.is_multiple_of(1024) {
                        checkpoint()?;
                    }
                    let point = decode_point(row)?;
                    if point.t_s <= previous || (end + sample == 0 && point.t_s != pass.start_s) {
                        return Err("Indexed pass time support changed".into());
                    }
                    previous = point.t_s;
                    for (axis, coordinate) in point.xyz_m.iter().copied().enumerate() {
                        actual_min[axis] = actual_min[axis].min(coordinate);
                        actual_max[axis] = actual_max[axis].max(coordinate);
                    }
                }
                checkpoint()?;
                let provenance = &block.provenance;
                if !blocks.insert(&provenance.path)
                    || provenance.bytes == 0
                    || provenance.bytes > MAX_BLOCK_BYTES
                {
                    return Err("Invalid track provenance block".into());
                }
                let provenance_path = child(&root, &provenance.path)?;
                let provenance_bytes = bounded_bytes_with_checkpoint(
                    &provenance_path,
                    provenance.bytes,
                    &mut checkpoint,
                )?;
                if provenance_bytes.len() as u64 != provenance.bytes
                    || digest_with_checkpoint(&provenance_bytes, &mut checkpoint)?
                        != provenance.sha256
                {
                    return Err("Track source provenance checksum changed".into());
                }
                end += block.point_count;
                stored = stored
                    .checked_add(block.bytes)
                    .ok_or("Track block bytes overflow")?;
                stored = stored
                    .checked_add(provenance.bytes)
                    .ok_or("Track provenance bytes overflow")?;
            }
            if end != pass.point_count
                || previous != pass.end_s
                || actual_min != pass.ecef_min
                || actual_max != pass.ecef_max
            {
                return Err("Track block support or conservative bounds are incomplete".into());
            }
            points = points.checked_add(end).ok_or("Track count overflow")?;
        }
        if points != manifest.stats.output_points
            || manifest.passes.len() != manifest.stats.output_passes
            || stored != manifest.stored_bytes
            || !manifest.passes.windows(2).all(|p| p[0].id < p[1].id)
        {
            return Err("Track-store complete counts/order disagree".into());
        }
        let manifest_sha256 = digest_with_checkpoint(&bytes, &mut checkpoint)?;
        checkpoint()?;
        Ok(Self {
            binding: TrackStoreBinding {
                manifest_path: path,
                manifest_sha256,
            },
            root,
            manifest,
            manifest_bytes: bytes.len() as u64,
            packed_files: Vec::new(),
            resident_memory_bytes,
        })
    }
    pub fn resident_memory_bytes(&self) -> u64 {
        self.resident_memory_bytes
    }
    pub fn binding(&self) -> &TrackStoreBinding {
        &self.binding
    }
    pub fn time_origin_utc_s(&self) -> f64 {
        self.manifest.config.time_origin_utc_s
    }
    pub fn import_config(&self) -> &TraceImportConfig {
        &self.manifest.config
    }
    pub fn passes(&self) -> &[PassMetadata] {
        &self.manifest.passes
    }
    pub fn total_points(&self) -> u64 {
        self.manifest.stats.output_points as u64
    }
    pub fn verify_unchanged(&self) -> Result<(), String> {
        self.verify_unchanged_with_checkpoint(|| Ok(()))
    }

    /// Recheck all frozen content while allowing the caller to stop a long scan.
    pub fn verify_unchanged_with_checkpoint(
        &self,
        mut checkpoint: impl FnMut() -> Result<(), String>,
    ) -> Result<(), String> {
        checkpoint()?;
        let bytes = bounded_bytes_with_checkpoint(
            &self.binding.manifest_path,
            self.manifest_bytes,
            &mut checkpoint,
        )?;
        if digest_with_checkpoint(&bytes, &mut checkpoint)? != self.binding.manifest_sha256 {
            return Err("Track-store manifest changed".into());
        }
        if !self.packed_files.is_empty() {
            for file in &self.packed_files {
                checkpoint()?;
                let path = child(&self.root, &file.path)?;
                packed::verify_file(&path, file, &mut checkpoint)?;
            }
            checkpoint()?;
            return Ok(());
        }
        for pass in &self.manifest.passes {
            checkpoint()?;
            for block in &pass.blocks {
                checkpoint()?;
                for (path, bytes, hash) in [
                    (&block.path, block.bytes, &block.sha256),
                    (
                        &block.provenance.path,
                        block.provenance.bytes,
                        &block.provenance.sha256,
                    ),
                ] {
                    let data = bounded_bytes_with_checkpoint(
                        &child(&self.root, path)?,
                        bytes,
                        &mut checkpoint,
                    )?;
                    if data.len() as u64 != bytes
                        || digest_with_checkpoint(&data, &mut checkpoint)? != *hash
                    {
                        return Err("Track store content changed".into());
                    }
                }
            }
        }
        checkpoint()?;
        Ok(())
    }
    pub fn load_track(&self, index: usize) -> Result<Track, String> {
        let metadata = self
            .manifest
            .passes
            .get(index)
            .ok_or("Track-store pass index unavailable")?;
        let mut points = Vec::with_capacity(metadata.point_count);
        for reference in &metadata.blocks {
            let path = child(&self.root, &reference.path)?;
            let bytes = if self.packed_files.is_empty() {
                bounded_bytes(&path, MAX_BLOCK_BYTES)?
            } else {
                packed::range_bytes(&path, reference.offset, reference.bytes)?
            };
            if bytes.len() as u64 != reference.bytes || digest(&bytes) != reference.sha256 {
                return Err("Track block checksum changed".into());
            }
            if reference.first_sample != points.len() || bytes.len() != reference.point_count * 32 {
                return Err("Track block identity/support changed".into());
            }
            for row in bytes.chunks_exact(32) {
                points.push(decode_point(row)?);
            }
        }
        if points.first().is_none_or(|p| p.t_s != metadata.start_s)
            || points.last().is_none_or(|p| p.t_s != metadata.end_s)
            || points.iter().any(|p| {
                !p.t_s.is_finite()
                    || (0..3).any(|i| {
                        !p.xyz_m[i].is_finite()
                            || p.xyz_m[i] < metadata.ecef_min[i]
                            || p.xyz_m[i] > metadata.ecef_max[i]
                    })
            })
            || !points.windows(2).all(|p| p[0].t_s < p[1].t_s)
        {
            return Err("Track points disagree with indexed continuous support".into());
        }
        Ok(Track {
            flight_id: metadata.id.clone(),
            points,
        })
    }
}

/// Convert an already-normalized interchange without resampling or reading a
/// source archive again. Caller-supplied source binding remains attribution;
/// the complete generated store has independently verified byte identities.
pub fn write_normalized(
    report: TraceImportReport,
    output: &Path,
    max_disk_bytes: u64,
    source_binding: Value,
) -> Result<TrackStoreBinding, String> {
    if output.exists() {
        return Err("Use a new track-store output directory".into());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let cancel = AtomicBool::new(false);
    let mut writer = TrackStoreWriter::new(
        output,
        &report.config,
        source_binding.clone(),
        max_disk_bytes,
    )?;
    writer.push(report, json!({"normalized_source":source_binding}), &cancel)?;
    writer.finish(&cancel)?;
    Ok(TrackStore::open(&output.join("index.json"))?
        .binding()
        .clone())
}

#[cfg(test)]
#[path = "track_store_tests.rs"]
mod tests;
