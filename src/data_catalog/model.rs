//! Stable catalog request, inventory, progress and reuse data contracts.
use crate::{
    adsblol::TraceImportConfig, adsblol_acquisition as acquisition, adsblol_history as history,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Adsb,
    Lightning,
    Terrain,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    pub label: String,
    pub kind: DataKind,
    pub date_utc: Option<String>,
    pub bounds: Option<[f64; 4]>,
    pub notes: String,
    pub max_bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    pub relative_path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    pub schema_version: u32,
    pub id: String,
    pub case_manifest_sha256: String,
    pub label: String,
    pub kind: DataKind,
    pub format: String,
    pub primary_file: PathBuf,
    pub created_unix_s: u64,
    pub source: Value,
    pub files: Vec<CatalogFile>,
    pub date_utc: Option<String>,
    pub bounds: Option<[f64; 4]>,
    pub notes: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct CatalogItem {
    pub entry: CatalogEntry,
    pub status: String,
    pub available_bytes: u64,
    pub problem: Option<String>,
    /// Listing checks paths, lengths and bounded provider status metadata;
    /// use verified_files before reading data or claiming source integrity.
    pub integrity_verified: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeleteReceipt {
    pub id: String,
    pub deleted_files: usize,
    pub deleted_bytes: u64,
    pub originals_preserved: bool,
}
#[derive(Clone, Debug)]
pub struct AdsbPlan {
    pub catalog_id: String,
    pub plan_path: PathBuf,
    pub plan: acquisition::DownloadPlan,
}
#[derive(Clone, Debug)]
pub struct HistoryItem {
    pub entry: CatalogEntry,
    pub plan: history::HistoryPlan,
    pub report: Option<history::HistoryReport>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DownloadProgress {
    pub planned_bytes: u64,
    pub downloaded_bytes: u64,
    pub complete: bool,
    pub notes: Vec<String>,
}

/// A metadata match, not a verified or ready source. Explicit reuse rechecks
/// both the current archive and this saved index before returning a binding.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedReuseMatch {
    pub catalog_id: String,
    pub label: String,
    pub index: crate::track_store::TrackStoreBinding,
    pub source_catalog_id: String,
    pub acquisition_manifest_sha256: String,
    pub config_sha256: String,
    /// Identity of the canonical serialized catalog entry, not its pathname.
    pub catalog_entry_sha256: String,
    pub registered_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct VerifiedIndexedReuse {
    pub matched: IndexedReuseMatch,
    pub binding: crate::track_store::TrackStoreBinding,
    pub source_passes: u64,
    pub source_points: u64,
    pub time_origin_utc_s: f64,
    /// Existing loader's conservative reservation, not measured process RAM.
    pub index_memory_bytes: u64,
    pub config: TraceImportConfig,
}
