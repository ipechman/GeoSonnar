//! Version2 packs exact range records into shards and pages rich pass metadata.
use super::*;
use flate2::{Compression, write::GzEncoder};
use std::{
    fs::OpenOptions,
    io::{self, BufWriter, Seek, SeekFrom, Write},
    mem::size_of,
};

const PAGE_BYTES: u64 = 1024 * 1024;
const SCRATCH_BYTES: u64 = 4 * 1024 * 1024;
const PROVENANCE_BUFFER_BYTES: usize = 64 * 1024;
const ENCODING: &str =
    "le-f64-t_s-x_m-y_m-z_m;32bytes;no-resampling;packed-ranges;independent-gzip-provenance";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PackedManifest {
    schema_version: u32,
    status: String,
    config: TraceImportConfig,
    source: Value,
    stats: TraceImportStats,
    stored_bytes: u64,
    location_verified: bool,
    point_encoding: String,
    index_memory_budget_bytes: u64,
    pass_count: usize,
    block_count: usize,
    metadata_pages: Vec<StoreFile>,
    point_shards: Vec<StoreFile>,
    provenance_shards: Vec<StoreFile>,
}

pub(super) fn memory_check(needed: u64, limit: u64) -> Result<(), String> {
    if needed > limit {
        let (needed, limit) = crate::data_size::format_gib_pair(needed, limit);
        Err(format!(
            "{INDEX_MEMORY_LIMIT_PREFIX}: need at least {needed}; declared {limit}. Increase the source/index memory allowance and try again; no data was removed"
        ))
    } else {
        Ok(())
    }
}
fn allocation_error(error: impl std::fmt::Display) -> String {
    format!("{INDEX_MEMORY_LIMIT_PREFIX}: allocation refused: {error}")
}
fn json_memory(value: &Value) -> u64 {
    let nested = match value {
        Value::String(text) => text.capacity() as u64,
        Value::Array(items) => {
            items.capacity() as u64 * size_of::<Value>() as u64
                + items.iter().map(json_memory).sum::<u64>()
        }
        Value::Object(items) => items
            .iter()
            .map(|(key, value)| 128 + key.capacity() as u64 + json_memory(value))
            .sum(),
        _ => 0,
    };
    size_of::<Value>() as u64 + nested
}
fn file_memory(file: &StoreFile) -> u64 {
    file.path.capacity() as u64 * 2 + file.sha256.capacity() as u64
}
fn files_memory(files: &Vec<StoreFile>) -> u64 {
    files.capacity() as u64 * size_of::<StoreFile>() as u64
        + files.iter().map(file_memory).sum::<u64>()
}
pub(super) fn pass_nested_memory(pass: &PassMetadata) -> u64 {
    pass.id.capacity() as u64
        + pass.blocks.capacity() as u64 * size_of::<BlockRef>() as u64
        + pass
            .blocks
            .iter()
            .map(|block| {
                block.path.capacity() as u64 * 2
                    + block.sha256.capacity() as u64
                    + file_memory(&block.provenance)
            })
            .sum::<u64>()
}
fn config_memory(config: &TraceImportConfig) -> u64 {
    // Config contains only a few strings/scalars. Serialized bytes×8 also
    // conservatively covers owned string capacity for normal validated input.
    serde_json::to_vec(config).map_or(u64::MAX, |bytes| bytes.len() as u64 * 8 + 4096)
}
pub(super) fn manifest_memory(manifest: &Manifest) -> u64 {
    size_of::<Manifest>() as u64
        + config_memory(&manifest.config)
        + json_memory(&manifest.source)
        + manifest.status.capacity() as u64
        + manifest.point_encoding.capacity() as u64
        + manifest.passes.capacity() as u64 * size_of::<PassMetadata>() as u64
        + manifest.passes.iter().map(pass_nested_memory).sum::<u64>()
}
fn stop(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Indexed import cancelled before publication".into())
    } else {
        Ok(())
    }
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

struct ActiveShard {
    file: File,
    path: PathBuf,
    bytes: u64,
    hash: Sha256,
}
struct Shards {
    root: PathBuf,
    prefix: &'static str,
    maximum: u64,
    active: Option<ActiveShard>,
    files: Vec<StoreFile>,
    receipt_memory: u64,
}
impl Shards {
    fn new(root: &Path, prefix: &'static str, maximum: u64) -> Self {
        Self {
            root: root.into(),
            prefix,
            maximum,
            active: None,
            files: Vec::new(),
            receipt_memory: 0,
        }
    }
    fn close(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        stop(cancel)?;
        if let Some(mut active) = self.active.take() {
            active.file.flush().map_err(|error| error.to_string())?;
            active.file.sync_all().map_err(|error| error.to_string())?;
            let receipt = StoreFile {
                path: active.path,
                offset: 0,
                bytes: active.bytes,
                sha256: format!("{:x}", active.hash.finalize()),
            };
            self.receipt_memory += file_memory(&receipt);
            self.files.push(receipt);
        }
        stop(cancel)
    }
    fn append(&mut self, bytes: &[u8], cancel: &AtomicBool) -> Result<StoreFile, String> {
        stop(cancel)?;
        if bytes.is_empty() || bytes.len() as u64 > self.maximum {
            return Err("Packed record exceeds its declared shard limit".into());
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.bytes + bytes.len() as u64 > self.maximum)
        {
            self.close(cancel)?;
        }
        if self.active.is_none() {
            let path = PathBuf::from(format!("{}-{:08}.bin", self.prefix, self.files.len()));
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.root.join(&path))
                .map_err(|error| error.to_string())?;
            self.active = Some(ActiveShard {
                file,
                path,
                bytes: 0,
                hash: Sha256::new(),
            });
        }
        let active = self.active.as_mut().ok_or("Packed shard was not opened")?;
        let offset = active.bytes;
        for chunk in bytes.chunks(64 * 1024) {
            stop(cancel)?;
            active
                .file
                .write_all(chunk)
                .map_err(|error| error.to_string())?;
            active.hash.update(chunk);
            active.bytes += chunk.len() as u64;
        }
        stop(cancel)?;
        Ok(StoreFile {
            path: active.path.clone(),
            offset,
            bytes: bytes.len() as u64,
            sha256: digest(bytes),
        })
    }
    fn memory(&self) -> u64 {
        self.files.capacity() as u64 * size_of::<StoreFile>() as u64
            + self.receipt_memory
            + self.root.capacity() as u64 * 2
            + 4096
    }
}

struct LimitedVec<'a> {
    bytes: Vec<u8>,
    maximum: u64,
    cancel: &'a AtomicBool,
    limit_error: &'static str,
}
impl Write for LimitedVec<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::other("Indexed import cancelled"));
        }
        if bytes.len() as u64 > self.maximum.saturating_sub(self.bytes.len() as u64) {
            return Err(io::Error::other(self.limit_error));
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            let capacity = ((needed as u64).div_ceil(65536) * 65536).min(self.maximum) as usize;
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(|error| io::Error::other(allocation_error(error)))?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) struct TrackStoreWriter {
    root: PathBuf,
    config: TraceImportConfig,
    source: Value,
    stats: TraceImportStats,
    limit: u64,
    memory_limit: u64,
    written: u64,
    block_count: usize,
    pass_count: usize,
    ids: BTreeSet<String>,
    id_memory: u64,
    base_memory: u64,
    page: Vec<PassMetadata>,
    page_memory: u64,
    page_bytes: u64,
    page_maximum: u64,
    pages: Vec<StoreFile>,
    pages_memory: u64,
    points: Shards,
    provenance: Shards,
}
impl TrackStoreWriter {
    #[cfg(test)]
    pub(super) fn test_limits(&mut self, points: u64, provenance: u64, page: u64, memory: u64) {
        assert_eq!(self.pass_count, 0);
        assert!((32..=MAX_BLOCK_BYTES).contains(&points));
        assert!(provenance > 0 && provenance <= MAX_BLOCK_BYTES);
        assert!(page > 0 && page <= PAGE_BYTES);
        self.points.maximum = points;
        self.provenance.maximum = provenance;
        self.page_maximum = page;
        self.memory_limit = memory;
    }
    pub(crate) fn new(
        root: &Path,
        config: &TraceImportConfig,
        source: Value,
        limit: u64,
    ) -> Result<Self, String> {
        Self::new_with_memory_limit(root, config, source, limit, DEFAULT_INDEX_MEMORY_BYTES)
    }
    pub(crate) fn new_with_memory_limit(
        root: &Path,
        config: &TraceImportConfig,
        source: Value,
        limit: u64,
        memory_limit: u64,
    ) -> Result<Self, String> {
        crate::adsblol::validate_config(config)?;
        validate_index_memory_limit(memory_limit)?;
        if !(1024 * 1024..=1024u64.pow(4)).contains(&limit) {
            return Err(
                "Declare a track-store disk budget between 0.0009765625 GiB and 1024.000 GiB"
                    .into(),
            );
        }
        let base_memory =
            root.as_os_str().len() as u64 * 4 + config_memory(config) + json_memory(&source) + 4096;
        let result = Self {
            root: root.into(),
            config: config.clone(),
            source,
            stats: TraceImportStats::default(),
            limit,
            memory_limit,
            written: 0,
            block_count: 0,
            pass_count: 0,
            ids: BTreeSet::new(),
            id_memory: 4096,
            base_memory,
            page: Vec::new(),
            page_memory: 0,
            page_bytes: 2,
            page_maximum: PAGE_BYTES,
            pages: Vec::new(),
            pages_memory: 0,
            points: Shards::new(root, "points", MAX_BLOCK_BYTES),
            provenance: Shards::new(root, "provenance", MAX_BLOCK_BYTES),
        };
        memory_check(result.memory().saturating_add(SCRATCH_BYTES), memory_limit)?;
        Ok(result)
    }
    fn memory(&self) -> u64 {
        self.id_memory
            + self.base_memory
            + self.pages.capacity() as u64 * size_of::<StoreFile>() as u64
            + self.pages_memory
            + self.points.memory()
            + self.provenance.memory()
            + self.page.capacity() as u64 * size_of::<PassMetadata>() as u64
            + self.page_memory
    }
    fn disk(&self, bytes: u64) -> Result<(), String> {
        if bytes > self.limit.saturating_sub(self.written) {
            Err("Track store disk budget exceeded".into())
        } else {
            Ok(())
        }
    }
    fn flush_page(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        stop(cancel)?;
        if self.page.is_empty() {
            return Ok(());
        }
        memory_check(
            self.memory()
                .saturating_add(self.page_bytes)
                .saturating_add(SCRATCH_BYTES),
            self.memory_limit,
        )?;
        let bytes = serde_json::to_vec(&self.page).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > self.page_maximum {
            return Err("Packed metadata page exceeds byte limit".into());
        }
        self.disk(bytes.len() as u64)?;
        let relative = PathBuf::from(format!("passes-{:08}.json", self.pages.len()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.root.join(&relative))
            .map_err(|error| error.to_string())?;
        for chunk in bytes.chunks(64 * 1024) {
            stop(cancel)?;
            file.write_all(chunk).map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())?;
        stop(cancel)?;
        self.written += bytes.len() as u64;
        let receipt = StoreFile {
            path: relative,
            offset: 0,
            bytes: bytes.len() as u64,
            sha256: digest(&bytes),
        };
        self.pages_memory += file_memory(&receipt);
        self.pages.push(receipt);
        self.page.clear();
        self.page_memory = 0;
        self.page_bytes = 2;
        Ok(())
    }
    pub(crate) fn push(
        &mut self,
        report: TraceImportReport,
        member: Value,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        stop(cancel)?;
        if report.config != self.config || report.schema_version != 1 || report.location_verified {
            return Err("Track report differs from indexed import policy".into());
        }
        if report.stats.output_passes != report.passes.len()
            || report.stats.output_points
                != report
                    .passes
                    .iter()
                    .map(|pass| pass.track.points.len())
                    .sum::<usize>()
            || report.stats.source_traces != report.sources.len()
            || report.stats.selected_traces
                != report
                    .sources
                    .iter()
                    .filter(|source| source.selected_by_region)
                    .count()
        {
            return Err("Normalized report counts disagree with retained source data".into());
        }
        self.stats.add(&report.stats);
        for pass in report.passes {
            stop(cancel)?;
            let count = pass.track.points.len();
            if !(2..=MAX_PASS_POINTS).contains(&count)
                || pass.source_points.len() != count
                || pass.track.flight_id.is_empty()
                || pass.track.flight_id.len() > 256
            {
                return Err("Track-store pass has invalid count or identity".into());
            }
            let id_bytes = pass.track.flight_id.len() as u64 + 128;
            memory_check(
                self.memory()
                    .saturating_add(id_bytes)
                    .saturating_add(SCRATCH_BYTES),
                self.memory_limit,
            )?;
            if !self.ids.insert(pass.track.flight_id.clone()) {
                return Err("Duplicate track-store pass identity".into());
            }
            self.id_memory += id_bytes;
            let mut minimum = [f64::INFINITY; 3];
            let mut maximum = [f64::NEG_INFINITY; 3];
            let mut previous = f64::NEG_INFINITY;
            for (index, point) in pass.track.points.iter().enumerate() {
                if index.is_multiple_of(1024) {
                    stop(cancel)?;
                }
                if !point.t_s.is_finite()
                    || point.t_s <= previous
                    || point.xyz_m.iter().any(|value| !value.is_finite())
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
            let points_per_range = POINTS_PER_BLOCK.min((self.points.maximum / 32) as usize);
            for first in (0..count).step_by(points_per_range) {
                stop(cancel)?;
                let end = (first + points_per_range).min(count);
                let mut bytes = Vec::with_capacity((end - first) * 32);
                for (index, point) in pass.track.points[first..end].iter().enumerate() {
                    if index.is_multiple_of(1024) {
                        stop(cancel)?;
                    }
                    for value in [point.t_s, point.xyz_m[0], point.xyz_m[1], point.xyz_m[2]] {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                }
                self.disk(bytes.len() as u64)?;
                let physical = self.points.append(&bytes, cancel)?;
                self.written += physical.bytes;
                let trace_source = report
                    .sources
                    .iter()
                    .find(|source| source.icao24 == pass.icao24)
                    .ok_or("Normalized pass has no source metadata")?;
                let source = json!({"archive_member":member,"trace_source":trace_source,
                    "provider":report.provider,"coordinate_frame":report.coordinate_frame,
                    "timestamp_definition":report.timestamp_definition,"quality_notes":report.quality_notes,
                    "icao24":pass.icao24,"source_label":pass.source_label,"source_timestamp_utc_s":pass.source_timestamp_utc_s});
                let available = self.memory_limit.saturating_sub(
                    self.memory()
                        .saturating_add(SCRATCH_BYTES)
                        .saturating_add(PROVENANCE_BUFFER_BYTES as u64)
                        .saturating_add(json_memory(&source))
                        .saturating_add(pass_nested_memory(&metadata)),
                );
                let maximum = self.provenance.maximum.min(available);
                memory_check(
                    self.memory()
                        .saturating_add(SCRATCH_BYTES)
                        .saturating_add(PROVENANCE_BUFFER_BYTES as u64)
                        .saturating_add(1),
                    self.memory_limit,
                )?;
                let target = LimitedVec {
                    bytes: Vec::new(),
                    maximum,
                    cancel,
                    limit_error: if maximum < self.provenance.maximum {
                        INDEX_MEMORY_LIMIT_PREFIX
                    } else {
                        "Packed provenance range exceeds shard limit"
                    },
                };
                // Buffer JSON tokens before compression. Reserve this buffer separately
                // above; the compressed output remains bounded by LimitedVec.
                let encoder = GzEncoder::new(target, Compression::fast());
                let mut buffered = BufWriter::with_capacity(PROVENANCE_BUFFER_BYTES, encoder);
                serde_json::to_writer(
                    &mut buffered,
                    &StoredBlock {
                        schema_version: 1,
                        pass_id: &metadata.id,
                        first_sample: first,
                        source_points: &pass.source_points[first..end],
                        source: &source,
                    },
                )
                .map_err(|error| error.to_string())?;
                // into_inner drains pending JSON without requesting a gzip sync flush.
                // Both that write and gzip finalization retain LimitedVec's cancellation
                // and byte-limit errors before any provenance receipt is published.
                let encoder = buffered.into_inner().map_err(|error| error.to_string())?;
                let compressed = encoder.finish().map_err(|error| error.to_string())?.bytes;
                self.disk(compressed.len() as u64)?;
                let provenance = self.provenance.append(&compressed, cancel)?;
                self.written += provenance.bytes;
                metadata.blocks.push(BlockRef {
                    path: physical.path,
                    offset: physical.offset,
                    bytes: physical.bytes,
                    sha256: physical.sha256,
                    first_sample: first,
                    point_count: end - first,
                    provenance,
                });
                self.block_count += 1;
            }
            let size = serde_json::to_vec(&metadata)
                .map_err(|error| error.to_string())?
                .len() as u64
                + 1;
            if size + 2 > self.page_maximum {
                return Err("One pass exceeds metadata-page limit".into());
            }
            if self.page_bytes + size > self.page_maximum {
                self.flush_page(cancel)?;
            }
            let additional = if self.page.len() == self.page.capacity() {
                self.page.capacity().max(64)
            } else {
                0
            };
            memory_check(
                self.memory()
                    .saturating_add(pass_nested_memory(&metadata))
                    .saturating_add(size_of::<PassMetadata>() as u64 * additional as u64)
                    .saturating_add(SCRATCH_BYTES),
                self.memory_limit,
            )?;
            if additional != 0 {
                self.page
                    .try_reserve_exact(additional)
                    .map_err(allocation_error)?;
            }
            self.page_memory += pass_nested_memory(&metadata);
            self.page.push(metadata);
            self.page_bytes += size;
            self.pass_count += 1;
        }
        Ok(())
    }
    pub(crate) fn finish(mut self, cancel: &AtomicBool) -> Result<Value, String> {
        stop(cancel)?;
        self.flush_page(cancel)?;
        self.points.close(cancel)?;
        self.provenance.close(cancel)?;
        self.ids.clear();
        self.page = Vec::new();
        stop(cancel)?;
        let manifest = PackedManifest {
            schema_version: 2,
            status: "complete".into(),
            config: self.config,
            source: self.source,
            stats: self.stats,
            stored_bytes: self.written,
            location_verified: false,
            point_encoding: ENCODING.into(),
            index_memory_budget_bytes: self.memory_limit,
            pass_count: self.pass_count,
            block_count: self.block_count,
            metadata_pages: self.pages,
            point_shards: self.points.files,
            provenance_shards: self.provenance.files,
        };
        let header_memory = config_memory(&manifest.config)
            + json_memory(&manifest.source)
            + files_memory(&manifest.metadata_pages)
            + files_memory(&manifest.point_shards)
            + files_memory(&manifest.provenance_shards)
            + SCRATCH_BYTES;
        memory_check(header_memory.saturating_add(1), self.memory_limit)?;
        let maximum = MAX_MANIFEST_BYTES.min(self.memory_limit - header_memory);
        let mut target = LimitedVec {
            bytes: Vec::new(),
            maximum,
            cancel,
            limit_error: if maximum < MAX_MANIFEST_BYTES {
                INDEX_MEMORY_LIMIT_PREFIX
            } else {
                "Track-store manifest exceeds metadata byte limit"
            },
        };
        serde_json::to_writer(&mut target, &manifest).map_err(|error| error.to_string())?;
        let bytes = target.bytes;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES
            || bytes.len() as u64 > self.limit.saturating_sub(self.written)
        {
            return Err("Track-store manifest exceeds metadata/disk budget".into());
        }
        let path = self.root.join("index.json");
        let pending = self.root.join("index.pending.json");
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&pending)
            .map_err(|error| error.to_string())?;
        for chunk in bytes.chunks(64 * 1024) {
            stop(cancel)?;
            file.write_all(chunk).map_err(|error| error.to_string())?;
        }
        file.sync_all().map_err(|error| error.to_string())?;
        drop(file);
        stop(cancel)?;
        if path.exists() {
            return Err("Completed track index already exists".into());
        }
        fs::rename(&pending, &path).map_err(|error| error.to_string())?;
        Ok(
            json!({"schema_version":2,"status":"indexed_tracks_imported","archive_import_complete":true,
            "manifest":{"path":path,"bytes":bytes.len(),"sha256":digest(&bytes)},"stats":manifest.stats,
            "pass_count":manifest.pass_count,"stored_bytes":self.written+bytes.len()as u64,
            "index_memory_budget_bytes":self.memory_limit,"real_world_coverage_complete":false,"location_verified":false}),
        )
    }
}

pub(super) fn range_bytes(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>, String> {
    let size = ordinary(path)?.len();
    if size > MAX_BLOCK_BYTES
        || length > MAX_BLOCK_BYTES
        || offset.checked_add(length).is_none_or(|end| end > size)
    {
        return Err("Packed range is outside its bounded shard".into());
    }
    let mut file = File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(allocation_error)?;
    file.take(length)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != length {
        return Err("Packed range was truncated".into());
    }
    Ok(bytes)
}

pub(super) fn verify_file(
    path: &Path,
    expected: &StoreFile,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    checkpoint()?;
    if ordinary(path)?.len() != expected.bytes {
        return Err("Track store content changed".into());
    }
    let mut reader = File::open(path).map_err(|error| error.to_string())?;
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = expected.bytes;
    let mut hash = Sha256::new();
    while remaining != 0 {
        checkpoint()?;
        let count = remaining.min(buffer.len() as u64) as usize;
        reader
            .read_exact(&mut buffer[..count])
            .map_err(|error| error.to_string())?;
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    checkpoint()?;
    if ordinary(path)?.len() != expected.bytes
        || format!("{:x}", hash.finalize()) != expected.sha256
    {
        return Err("Track store content changed".into());
    }
    Ok(())
}

/// Stream each packed shard once in the declared append order. Requiring exact
/// offsets and complete coverage proves no overlaps, skipped bytes or hidden
/// records; independent range hashes also support later bounded random reads.
struct RangeVerifier<'a> {
    root: &'a Path,
    files: &'a [StoreFile],
    index: usize,
    offset: u64,
    reader: Option<File>,
    hash: Sha256,
}
impl<'a> RangeVerifier<'a> {
    fn new(root: &'a Path, files: &'a [StoreFile]) -> Self {
        Self {
            root,
            files,
            index: 0,
            offset: 0,
            reader: None,
            hash: Sha256::new(),
        }
    }
    fn read(
        &mut self,
        range: &StoreFile,
        retain: bool,
        checkpoint: &mut impl FnMut() -> Result<(), String>,
    ) -> Result<Vec<u8>, String> {
        checkpoint()?;
        let whole = self
            .files
            .get(self.index)
            .ok_or("Packed range refers to an unregistered shard")?;
        if range.path != whole.path
            || range.offset != self.offset
            || range.bytes == 0
            || !valid_hash(&range.sha256)
            || range
                .offset
                .checked_add(range.bytes)
                .is_none_or(|end| end > whole.bytes)
        {
            return Err("Packed ranges overlap, have missing bytes, or escape their shard".into());
        }
        if self.reader.is_none() {
            let path = child(self.root, &whole.path)?;
            if ordinary(&path)?.len() != whole.bytes {
                return Err("Packed shard size changed".into());
            }
            self.reader = Some(File::open(path).map_err(|error| error.to_string())?);
        }
        let reader = self
            .reader
            .as_mut()
            .ok_or("Packed shard reader unavailable")?;
        let mut bytes = Vec::new();
        if retain {
            bytes
                .try_reserve_exact(range.bytes as usize)
                .map_err(allocation_error)?;
        }
        let mut remaining = range.bytes;
        let mut buffer = [0u8; 64 * 1024];
        let mut hash = Sha256::new();
        while remaining != 0 {
            checkpoint()?;
            let count = remaining.min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..count])
                .map_err(|error| error.to_string())?;
            checkpoint()?;
            hash.update(&buffer[..count]);
            self.hash.update(&buffer[..count]);
            if retain {
                bytes.extend_from_slice(&buffer[..count]);
            }
            remaining -= count as u64;
        }
        if format!("{:x}", hash.finalize()) != range.sha256 {
            return Err("Packed range checksum changed".into());
        }
        self.offset += range.bytes;
        if self.offset == whole.bytes {
            if format!("{:x}", std::mem::take(&mut self.hash).finalize()) != whole.sha256 {
                return Err("Packed shard checksum changed".into());
            }
            if ordinary(&child(self.root, &whole.path)?)?.len() != whole.bytes {
                return Err("Packed shard size changed while reading".into());
            }
            self.reader = None;
            self.index += 1;
            self.offset = 0;
        }
        checkpoint()?;
        Ok(bytes)
    }
    fn finish(&self) -> Result<(), String> {
        if self.index != self.files.len() || self.reader.is_some() || self.offset != 0 {
            Err("Packed shard inventory has missing or unreferenced ranges".into())
        } else {
            Ok(())
        }
    }
}

pub(super) fn open(
    path: PathBuf,
    bytes: Vec<u8>,
    memory_limit: u64,
    checkpoint: &mut impl FnMut() -> Result<(), String>,
) -> Result<TrackStore, String> {
    checkpoint()?;
    let header: PackedManifest =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    checkpoint()?;
    crate::adsblol::validate_config(&header.config)?;
    validate_index_memory_limit(header.index_memory_budget_bytes)?;
    if header.schema_version != 2
        || header.status != "complete"
        || header.location_verified
        || header.point_encoding != ENCODING
        || header.pass_count != header.stats.output_passes
    {
        return Err("Only a complete valid packed track store can be opened".into());
    }
    let root = path
        .parent()
        .ok_or("Track store has no directory")?
        .to_path_buf();
    let mut names = BTreeSet::new();
    let mut stored = 0u64;
    for (files, prefix, suffix, maximum) in [
        (&header.metadata_pages, "passes", "json", PAGE_BYTES),
        (&header.point_shards, "points", "bin", MAX_BLOCK_BYTES),
        (
            &header.provenance_shards,
            "provenance",
            "bin",
            MAX_BLOCK_BYTES,
        ),
    ] {
        for (index, file) in files.iter().enumerate() {
            checkpoint()?;
            if file.offset != 0
                || file.bytes == 0
                || file.bytes > maximum
                || !valid_hash(&file.sha256)
                || file.path != Path::new(&format!("{prefix}-{index:08}.{suffix}"))
                || !names.insert(&file.path)
            {
                return Err("Invalid packed track-store file inventory".into());
            }
            let file_path = child(&root, &file.path)?;
            if ordinary(&file_path)?.len() != file.bytes {
                return Err("Packed file size changed".into());
            }
            stored = stored
                .checked_add(file.bytes)
                .ok_or("Packed byte count overflow")?;
        }
    }
    if stored != header.stored_bytes {
        return Err("Packed inventory byte total changed".into());
    }
    drop(names);
    let header_memory = config_memory(&header.config)
        + json_memory(&header.source)
        + files_memory(&header.metadata_pages)
        + files_memory(&header.point_shards)
        + files_memory(&header.provenance_shards)
        + path.capacity() as u64 * 4
        + 16384;
    let pass_array_bytes = (header.pass_count as u64)
        .checked_mul(size_of::<PassMetadata>() as u64)
        .ok_or_else(|| allocation_error("pass inventory overflow"))?;
    memory_check(
        header_memory
            .saturating_add(pass_array_bytes)
            .saturating_add(bytes.capacity() as u64)
            .saturating_add(SCRATCH_BYTES),
        memory_limit,
    )?;
    let mut passes = Vec::new();
    passes
        .try_reserve_exact(header.pass_count)
        .map_err(allocation_error)?;
    let mut retained = header_memory + passes.capacity() as u64 * size_of::<PassMetadata>() as u64;
    let mut point_verifier = RangeVerifier::new(&root, &header.point_shards);
    let mut source_verifier = RangeVerifier::new(&root, &header.provenance_shards);
    let mut point_count = 0usize;
    let mut block_count = 0usize;
    for page in &header.metadata_pages {
        checkpoint()?;
        memory_check(
            retained
                .saturating_add(page.bytes.saturating_mul(32))
                .saturating_add(bytes.capacity() as u64)
                .saturating_add(SCRATCH_BYTES),
            memory_limit,
        )?;
        let page_bytes =
            bounded_bytes_with_checkpoint(&child(&root, &page.path)?, page.bytes, checkpoint)?;
        if digest_with_checkpoint(&page_bytes, checkpoint)? != page.sha256 {
            return Err("Packed metadata page checksum changed".into());
        }
        checkpoint()?;
        let rows: Vec<PassMetadata> =
            serde_json::from_slice(&page_bytes).map_err(|error| error.to_string())?;
        checkpoint()?;
        if rows.is_empty() || rows.len() > header.pass_count.saturating_sub(passes.len()) {
            return Err("Packed metadata page count disagrees with the index".into());
        }
        let page_memory = rows.capacity() as u64 * size_of::<PassMetadata>() as u64
            + rows.iter().map(pass_nested_memory).sum::<u64>();
        memory_check(
            retained
                .saturating_add(page_memory)
                .saturating_add(page_bytes.capacity() as u64)
                .saturating_add(bytes.capacity() as u64)
                .saturating_add(SCRATCH_BYTES),
            memory_limit,
        )?;
        for pass in rows {
            checkpoint()?;
            if pass.id.is_empty()
                || pass.id.len() > 256
                || !(2..=MAX_PASS_POINTS).contains(&pass.point_count)
                || !pass.start_s.is_finite()
                || !pass.end_s.is_finite()
                || pass.start_s >= pass.end_s
                || (0..3).any(|axis| {
                    !pass.ecef_min[axis].is_finite()
                        || !pass.ecef_max[axis].is_finite()
                        || pass.ecef_min[axis] > pass.ecef_max[axis]
                })
            {
                return Err("Invalid indexed trajectory metadata".into());
            }
            let mut end = 0usize;
            let mut previous = f64::NEG_INFINITY;
            let mut actual_min = [f64::INFINITY; 3];
            let mut actual_max = [f64::NEG_INFINITY; 3];
            for block in &pass.blocks {
                checkpoint()?;
                if block.first_sample != end
                    || !(1..=POINTS_PER_BLOCK).contains(&block.point_count)
                    || block.bytes != block.point_count as u64 * 32
                    || block.provenance.bytes > MAX_BLOCK_BYTES
                {
                    return Err("Invalid packed point-range count or support".into());
                }
                let native = point_verifier.read(
                    &StoreFile {
                        path: block.path.clone(),
                        offset: block.offset,
                        bytes: block.bytes,
                        sha256: block.sha256.clone(),
                    },
                    true,
                    checkpoint,
                )?;
                for (sample, row) in native.chunks_exact(32).enumerate() {
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
                source_verifier.read(&block.provenance, false, checkpoint)?;
                end = end
                    .checked_add(block.point_count)
                    .ok_or("Packed point count overflow")?;
                block_count = block_count
                    .checked_add(1)
                    .ok_or("Packed block count overflow")?;
            }
            if end != pass.point_count
                || previous != pass.end_s
                || actual_min != pass.ecef_min
                || actual_max != pass.ecef_max
            {
                return Err("Track block support or conservative bounds are incomplete".into());
            }
            point_count = point_count
                .checked_add(end)
                .ok_or("Packed point total overflow")?;
            retained += pass_nested_memory(&pass);
            passes.push(pass);
        }
    }
    point_verifier.finish()?;
    source_verifier.finish()?;
    drop(point_verifier);
    drop(source_verifier);
    if passes.len() != header.pass_count
        || point_count != header.stats.output_points
        || block_count != header.block_count
    {
        return Err("Packed complete pass/point/range counts disagree".into());
    }
    checkpoint()?;
    // In-place sorting needs no second full metadata array. Source append order
    // remains in the hashed pages; the public search inventory stays lexical.
    passes.sort_unstable_by(|left, right| left.id.cmp(&right.id));
    checkpoint()?;
    if passes.windows(2).any(|pair| pair[0].id >= pair[1].id) {
        return Err("Duplicate packed trajectory identity".into());
    }
    let manifest_sha256 = digest_with_checkpoint(&bytes, checkpoint)?;
    let mut packed_files = header.metadata_pages;
    packed_files
        .try_reserve_exact(header.point_shards.len() + header.provenance_shards.len())
        .map_err(allocation_error)?;
    packed_files.extend(header.point_shards);
    packed_files.extend(header.provenance_shards);
    let manifest = Manifest {
        schema_version: 2,
        status: header.status,
        config: header.config,
        source: header.source,
        stats: header.stats,
        passes,
        stored_bytes: header.stored_bytes,
        location_verified: false,
        point_encoding: header.point_encoding,
    };
    let resident_memory_bytes = manifest_memory(&manifest)
        + files_memory(&packed_files)
        + path.capacity() as u64 * 4
        + bytes.capacity() as u64
        + SCRATCH_BYTES
        + 16384;
    memory_check(resident_memory_bytes, memory_limit)?;
    checkpoint()?;
    Ok(TrackStore {
        binding: TrackStoreBinding {
            manifest_path: path,
            manifest_sha256,
        },
        root,
        manifest,
        manifest_bytes: bytes.len() as u64,
        packed_files,
        resident_memory_bytes,
    })
}
