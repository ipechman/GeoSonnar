//! Case-local owned data copies. Sources and old case/job evidence are never moved.
//! Public compatibility facade; implementations have one responsibility each.
//! Destructive operations remove only individually registered regular files.
mod adsb_downloads;
mod catalog;
mod downloads;
mod history;
mod imports;
mod indexed_reuse;
mod model;
mod storage;

#[path = "fmi_catalog.rs"]
pub mod fmi;
#[path = "dataset_requests.rs"]
pub mod requests;

pub use adsb_downloads::{
    adsblol_progress, fetch_adsblol, fetch_adsblol_with_cancel, plan_adsblol,
};
pub use catalog::{
    delete_owned, list, verified_files, verified_files_with_cancel, verified_files_with_checkpoint,
};
pub use downloads::download_url_with_cancel;
pub use history::{
    fetch_history, fetch_history_with_transport, import_history, plan_history, read_history,
};
pub use imports::{
    import_adsblol_archive, import_adsblol_archive_with_cancel, import_adsblol_indexed_with_cancel,
    import_adsblol_indexed_with_limits, import_file, import_file_with_cancel,
};
pub use indexed_reuse::{
    find_completed_indexed_with_cancel, find_completed_indexed_with_checkpoint,
    verify_indexed_reuse_with_checkpoint, verify_indexed_reuse_with_limits,
};
pub use model::{
    AdsbPlan, CatalogEntry, CatalogFile, CatalogItem, DataKind, DeleteReceipt, DownloadProgress,
    HistoryItem, ImportRequest, IndexedReuseMatch, VerifiedIndexedReuse,
};

// Compatibility bridge for the existing FMI and generic provider adapters.
// New catalog internals import their dependencies explicitly from child modules.
use crate::detailed_data::sha256;
use downloads::download_url_as_with_cancel;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use storage::{
    checked, child, entry, identity, item_dir, lock, new_item, now, read, regular, unchanged_case,
    validate_bounds, write_new,
};
