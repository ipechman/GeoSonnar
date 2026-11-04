//! Fresh-only bounded paged results. A missing terminal receipt means interrupted,
//! never completed. This first journal deliberately does not advertise resume.
use crate::wide_area_pair::{
    MAX_REPORT_BYTES, MAX_REQUEST_BYTES, ReceiverResult, WidePairReport, WidePairRequest,
    request_sha256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const RESERVED_FINAL_BYTES: u64 = 16 * 1024 * 1024;
const PAGE_ROWS: u64 = 256;
const PAGE_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageReceipt {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub first_receiver: u64,
    pub receiver_count: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Terminal {
    schema_version: u32,
    request_sha256: String,
    report_sha256: String,
    report_bytes: u64,
}
struct Page {
    file: File,
    name: String,
    hash: Sha256,
    bytes: u64,
    first: u64,
    rows: u64,
}
pub(crate) struct Journal {
    pub directory: PathBuf,
    budget: u64,
    used: u64,
    page: Option<Page>,
    pub pages: Vec<PageReceipt>,
    next_receiver: u64,
}
fn ordinary_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("Wide-area results cannot use linked paths".into());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err("Wide-area results cannot use reparse paths".into());
            }
        }
    }
    Ok(())
}
fn fresh(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}
fn limited(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    ordinary_ancestors(path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > maximum {
        return Err("Wide-area journal artifact exceeds its bound".into());
    }
    Ok(bytes)
}
impl Journal {
    pub fn create(directory: &Path, request: &WidePairRequest) -> Result<Self, String> {
        if !directory.is_absolute() {
            return Err("Choose an absolute fresh result directory".into());
        }
        ordinary_ancestors(directory.parent().ok_or("Missing result parent")?)?;
        fs::create_dir(directory).map_err(|e| format!("Results require a fresh directory: {e}"))?;
        let bytes = serde_json::to_vec(request).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("Oversized wide-area request".into());
        }
        fresh(&directory.join("request.json"), &bytes)?;
        Ok(Self {
            directory: directory.to_owned(),
            budget: request.budgets.max_output_bytes,
            used: bytes.len() as u64,
            page: None,
            pages: Vec::new(),
            next_receiver: 0,
        })
    }
    pub fn can_write(&self, bytes: u64) -> bool {
        self.used
            .checked_add(bytes)
            .and_then(|n| n.checked_add(RESERVED_FINAL_BYTES))
            .is_some_and(|n| n <= self.budget)
    }
    pub fn write_receiver(&mut self, row: &ReceiverResult) -> Result<(), String> {
        if row.receiver_index != self.next_receiver {
            return Err("Receiver journal order changed".into());
        }
        let mut bytes = serde_json::to_vec(row).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > 16 * 1024 {
            return Err("Receiver summary exceeds 16 KiB".into());
        }
        if !self.can_write(bytes.len() as u64 + 4096) {
            return Err("Wide-area output budget exhausted".into());
        }
        if self
            .page
            .as_ref()
            .is_some_and(|p| p.rows >= PAGE_ROWS || p.bytes + bytes.len() as u64 > PAGE_BYTES)
        {
            self.seal_page()?;
        }
        if self.page.is_none() {
            let name = format!("receivers-{:08}.jsonl", self.pages.len());
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.directory.join(&name))
                .map_err(|e| e.to_string())?;
            self.page = Some(Page {
                file,
                name,
                hash: Sha256::new(),
                bytes: 0,
                first: self.next_receiver,
                rows: 0,
            });
        }
        let page = self.page.as_mut().unwrap();
        page.file.write_all(&bytes).map_err(|e| e.to_string())?;
        page.hash.update(&bytes);
        page.bytes += bytes.len() as u64;
        page.rows += 1;
        self.used += bytes.len() as u64;
        self.next_receiver += 1;
        Ok(())
    }
    pub fn seal_page(&mut self) -> Result<(), String> {
        if let Some(page) = self.page.take() {
            page.file.sync_all().map_err(|e| e.to_string())?;
            let receipt = PageReceipt {
                name: page.name,
                bytes: page.bytes,
                sha256: format!("{:x}", page.hash.finalize()),
                first_receiver: page.first,
                receiver_count: page.rows,
            };
            let mut bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            if !self.can_write(bytes.len() as u64) {
                return Err("Wide-area checkpoint budget exhausted".into());
            }
            let mut index = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.directory.join("checkpoints.jsonl"))
                .map_err(|e| e.to_string())?;
            index
                .write_all(&bytes)
                .and_then(|_| index.sync_all())
                .map_err(|e| e.to_string())?;
            self.used += bytes.len() as u64;
            self.pages.push(receipt);
        }
        Ok(())
    }
    pub fn finish(&mut self, report: &mut WidePairReport) -> Result<(), String> {
        self.seal_page()?;
        report.pages = self.pages.clone();
        if report.progress.completed_receivers != self.next_receiver {
            return Err("Report and durable receiver inventory differ".into());
        }
        let bytes = serde_json::to_vec(report).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_REPORT_BYTES || self.used + bytes.len() as u64 + 4096 > self.budget {
            return Err("Wide-area final report exceeds its reserved output budget".into());
        }
        fresh(&self.directory.join("report.json"), &bytes)?;
        let terminal = Terminal {
            schema_version: 1,
            request_sha256: report.request_sha256.clone(),
            report_sha256: format!("{:x}", Sha256::digest(&bytes)),
            report_bytes: bytes.len() as u64,
        };
        fresh(
            &self.directory.join("terminal.json"),
            &serde_json::to_vec(&terminal).map_err(|e| e.to_string())?,
        )
    }
}
/// Reopen a completed or explicitly stopped historical result without opening
/// original media or aircraft. Missing terminal files remain interrupted.
pub fn load_report(directory: &Path) -> Result<WidePairReport, String> {
    let terminal: Terminal =
        serde_json::from_slice(&limited(&directory.join("terminal.json"), 4096)?)
            .map_err(|e| e.to_string())?;
    let request: WidePairRequest = serde_json::from_slice(&limited(
        &directory.join("request.json"),
        MAX_REQUEST_BYTES,
    )?)
    .map_err(|e| e.to_string())?;
    crate::wide_area_pair::validate_request(&request)?;
    let bytes = limited(&directory.join("report.json"), MAX_REPORT_BYTES)?;
    if terminal.schema_version != 1
        || terminal.request_sha256 != request_sha256(&request)?
        || terminal.report_bytes != bytes.len() as u64
        || terminal.report_sha256 != format!("{:x}", Sha256::digest(&bytes))
    {
        return Err("Wide-area saved result identity changed".into());
    }
    let report: WidePairReport = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if report.request_sha256 != terminal.request_sha256
        || report.progress.total_receivers != request.grid.count
        || report.progress.completed_receivers > report.progress.total_receivers
        || report.location_verified
        || report.best_hypotheses.len() > request.retain
        || report.best_flight_pairs.len() > request.retain
        || report.progress.work_units > request.budgets.max_work_units
        || report.source != request.source
        || report
            .source_time_origin_utc_s
            .is_some_and(|origin| !origin.is_finite() || !(0.0..10_000_000_000.0).contains(&origin))
        || (report.source_time_origin_utc_s.is_none()
            && (report.progress.completed_receivers != 0
                || !report.best_hypotheses.is_empty()
                || !report.best_flight_pairs.is_empty()
                || report.source_verified_at_end))
        || (report.status == crate::wide_area_pair::WideStatus::Completed
            && (report.progress.completed_receivers != request.grid.count
                || !report.source_verified_at_end
                || report.source_time_origin_utc_s.is_none()
                || report.error.is_some()))
    {
        return Err("Saved wide-area report has inconsistent completion or retention".into());
    }
    let mut next = 0;
    for (index, page) in report.pages.iter().enumerate() {
        if page.name != format!("receivers-{index:08}.jsonl")
            || page.first_receiver != next
            || page.receiver_count == 0
            || page.receiver_count > PAGE_ROWS
            || page.bytes > PAGE_BYTES
        {
            return Err("Wide-area page inventory differs".into());
        }
        let bytes = limited(&directory.join(&page.name), PAGE_BYTES as usize)?;
        if page.bytes != bytes.len() as u64
            || page.sha256 != format!("{:x}", Sha256::digest(&bytes))
        {
            return Err("Wide-area receiver page changed".into());
        }
        let mut count = 0;
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            let row: ReceiverResult = serde_json::from_slice(line).map_err(|e| e.to_string())?;
            if row.receiver_index != next
                || serde_json::to_value(&row.observer).map_err(|e| e.to_string())?
                    != serde_json::to_value(request.grid.observer(next)?)
                        .map_err(|e| e.to_string())?
            {
                return Err("Wide-area receiver coordinate/order changed".into());
            }
            count += 1;
            next += 1;
        }
        if count != page.receiver_count {
            return Err("Wide-area page row count differs".into());
        }
    }
    if next != report.progress.completed_receivers {
        return Err("Wide-area completed receiver inventory is incomplete".into());
    }
    Ok(report)
}
