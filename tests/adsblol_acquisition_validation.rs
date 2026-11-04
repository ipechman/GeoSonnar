use geosonnar::adsblol_acquisition::{self as acquisition, AcquisitionRequest, MetadataReceipt};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join(format!(
                "adsblol-acquisition-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let allowed = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .canonicalize()
            .unwrap();
        if let Ok(path) = self.0.canonicalize() {
            assert!(path.starts_with(&allowed) && path != allowed);
            // Windows can briefly retain a filesystem handle during teardown.
            // Retry only sharing/lock conflicts, within a fixed small bound;
            // persistent conflicts and all other errors still fail the test.
            for attempt in 0..5 {
                match fs::remove_dir_all(&path) {
                    Ok(()) => return,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                    Err(error)
                        if cfg!(windows)
                            && attempt < 4
                            && matches!(error.raw_os_error(), Some(32 | 33)) =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(error) => panic!("Cannot remove owned acquisition fixture: {error}"),
                }
            }
        }
    }
}

fn request() -> AcquisitionRequest {
    AcquisitionRequest {
        date_utc: "2026-09-08".into(),
        instance: Some("prod-0".into()),
        max_download_bytes: 6_000_000_000,
    }
}
fn tag() -> &'static str {
    "v2026.09.08-planes-readsb-prod-0"
}
fn release(parts: &[(&str, u64)]) -> Value {
    json!({"tag_name":tag(),"draft":false,"prerelease":false,
        "html_url":format!("https://github.com/adsblol/globe_history_2026/releases/tag/{}",tag()),
        "assets":parts.iter().map(|(suffix,size)| {let name=format!("{}.tar{}",tag(),suffix);json!({"name":name,"state":"uploaded","size":size,"digest":format!("sha256:{}","a".repeat(64)),"browser_download_url":format!("https://github.com/adsblol/globe_history_2026/releases/download/{}/{}",tag(),name)})}).collect::<Vec<_>>()})
}

#[test]
fn provider_preference_is_date_and_instance_bound() {
    let mut r = request();
    r.instance = None;
    let url = format!(
        "https://github.com/adsblol/globe_history_2026/releases/download/{}/{}.tar.aa",
        tag(),
        tag()
    );
    assert_eq!(acquisition::resolve_instance(&r, &url).unwrap(), "prod-0");
    assert!(
        acquisition::resolve_instance(&r, &url.replace("github.com", "attacker.test")).is_err()
    );
    assert!(acquisition::resolve_instance(&r, &format!("{url}\n{url}")).is_err());
    assert!(acquisition::resolve_instance(&r, "").is_err());
    assert_eq!(
        acquisition::resolve_instance(&request(), "").unwrap(),
        "prod-0"
    );
}

#[test]
fn calendar_instance_and_budget_are_validated_before_network() {
    for date in [
        "2026-02-29",
        "2026-13-01",
        "2026-09-00",
        "2022-01-01",
        "2026/09/08",
        "../../evil",
    ] {
        let mut r = request();
        r.date_utc = date.into();
        assert!(acquisition::resolve_instance(&r, "").is_err(), "{date}");
    }
    let mut r = request();
    r.date_utc = "2024-02-29".into();
    assert!(acquisition::resolve_instance(&r, "").is_ok());
    r = request();
    r.instance = Some("https://attacker.test".into());
    assert!(acquisition::resolve_instance(&r, "").is_err());
    r = request();
    r.max_download_bytes = 0;
    assert!(acquisition::resolve_instance(&r, "").is_err());
}

#[test]
fn parts_are_sorted_and_exact_provider_sizes_hashes_retained() {
    let p = acquisition::parse_release_plan(
        &request(),
        "prod-0",
        &release(&[(".ab", 22), (".aa", 2_000_000_000)]),
    )
    .unwrap();
    assert_eq!(p.assets[0].name, format!("{}.tar.aa", tag()));
    assert_eq!(p.assets[1].bytes, 22);
    assert_eq!(p.planned_bytes, 2_000_000_022);
    assert!(!p.coverage_complete);
    assert_eq!(p.license, "ODbL-1.0");
    assert!(acquisition::parse_release_plan(&request(), "prod-0", &release(&[("", 100)])).is_ok());
}

#[test]
fn missing_duplicate_or_foreign_archive_parts_are_rejected() {
    for parts in [
        vec![(".ab", 22)],
        vec![(".aa", 2_000_000_000), (".ac", 22)],
        vec![(".aa", 1), (".aa", 1)],
        vec![(".aa", 1), (".ab", 1)],
        vec![("", 1), (".aa", 1)],
        vec![(".../../x", 1)],
    ] {
        assert!(acquisition::parse_release_plan(&request(), "prod-0", &release(&parts)).is_err());
    }
}

#[test]
fn unsafe_missing_or_unfinished_provider_metadata_is_rejected() {
    let good = release(&[("", 100)]);
    let mutations = [
        ("name", json!("../bad.tar")),
        ("size", json!(0)),
        ("size", json!(2_000_000_001u64)),
        ("digest", json!("sha256:bad")),
        ("digest", Value::Null),
        ("state", json!("new")),
        ("browser_download_url", json!("https://attacker.test/a.tar")),
    ];
    for (key, value) in mutations {
        let mut bad = good.clone();
        bad["assets"][0][key] = value;
        assert!(
            acquisition::parse_release_plan(&request(), "prod-0", &bad).is_err(),
            "{key}"
        );
    }
    for (key, value) in [
        ("tag_name", json!("v2026.09.09-planes-readsb-prod-0")),
        ("draft", json!(true)),
        ("prerelease", json!(true)),
        ("html_url", json!("https://attacker.test/release")),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(
            acquisition::parse_release_plan(&request(), "prod-0", &bad).is_err(),
            "{key}"
        );
    }
    let mut r = request();
    r.max_download_bytes = 99;
    assert!(acquisition::parse_release_plan(&r, "prod-0", &good).is_err());
}

#[test]
fn http_range_requires_exact_final_status_interval_total_and_length() {
    let good = "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\nHTTP/2 206\r\nContent-Range: bytes 5-9/100\r\nContent-Length: 5\r\n\r\n";
    assert!(acquisition::validate_range_response(good, 5, 9, 100, 5).is_ok());
    for bad in [
        good.replace("HTTP/2 206", "HTTP/2 200"),
        good.replace("5-9/100", "0-4/100"),
        good.replace("5-9/100", "5-9/101"),
        good.replace("Content-Length: 5", "Content-Length: 4"),
        format!("{good}HTTP/1.1 500 Failed\r\n"),
    ] {
        assert!(acquisition::validate_range_response(&bad, 5, 9, 100, 5).is_err());
    }
    assert!(acquisition::validate_range_response(good, 5, 9, 100, 4).is_err());
    assert!(acquisition::validate_range_response(good, 5, 100, 100, 96).is_err());
}

#[test]
fn cached_bytes_are_checked_natively_not_trusted_by_filename_or_length() {
    let scratch = Scratch::new();
    let path = scratch.0.join("bytes.bin");
    fs::write(&path, b"abc").unwrap();
    let mut asset = acquisition::parse_release_plan(&request(), "prod-0", &release(&[("", 3)]))
        .unwrap()
        .assets
        .remove(0);
    asset.sha256 = acquisition::sha256_file(&path).unwrap();
    assert!(acquisition::verify_asset(&path, &asset).is_ok());
    fs::write(&path, b"abd").unwrap();
    assert!(acquisition::verify_asset(&path, &asset).is_err());
    fs::write(&path, b"abcd").unwrap();
    assert!(acquisition::verify_asset(&path, &asset).is_err());
    fs::remove_file(&path).unwrap();
    assert!(acquisition::verify_asset(&path, &asset).is_err());
}

fn offline_plan(directory: &Path) -> (PathBuf, acquisition::DownloadPlan) {
    fs::create_dir_all(directory).unwrap();
    let payload = directory.join("payload");
    fs::write(&payload, b"abc").unwrap();
    let mut source = release(&[("", 3)]);
    source["assets"][0]["digest"] = json!(format!(
        "sha256:{}",
        acquisition::sha256_file(&payload).unwrap()
    ));
    let metadata = directory.join("release.json");
    fs::write(&metadata, serde_json::to_vec_pretty(&source).unwrap()).unwrap();
    let mut plan = acquisition::parse_release_plan(&request(), "prod-0", &source).unwrap();
    plan.metadata.push(MetadataReceipt {
        path: "release.json".into(),
        bytes: fs::metadata(&metadata).unwrap().len(),
        sha256: acquisition::sha256_file(&metadata).unwrap(),
    });
    bind_request(directory, &mut plan);
    let path = directory.join("download-plan.json");
    fs::write(&path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    (path, plan)
}

fn bind_request(directory: &Path, plan: &mut acquisition::DownloadPlan) {
    let request_path = directory.join("request.json");
    fs::write(&request_path, serde_json::to_vec(&plan.request).unwrap()).unwrap();
    plan.metadata
        .retain(|item| item.path != Path::new("request.json"));
    plan.metadata.push(MetadataReceipt {
        path: "request.json".into(),
        bytes: fs::metadata(&request_path).unwrap().len(),
        sha256: acquisition::sha256_file(&request_path).unwrap(),
    });
}

#[test]
fn complete_cached_archive_is_adopted_offline_and_portably_reverified() {
    let scratch = Scratch::new();
    let (plan_path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(&plan.assets[0].name), b"abc").unwrap();
    let manifest = acquisition::fetch(&plan_path, &cache).unwrap();
    assert!(manifest.complete && manifest.full_sha_verified && !manifest.coverage_complete);
    assert_eq!(manifest.parts[0].path, PathBuf::from(&plan.assets[0].name));
    assert!(!manifest.parts[0].path.is_absolute());
    let moved = scratch.0.join("moved-cache");
    fs::create_dir(&moved).unwrap();
    for name in [&plan.assets[0].name, "archive-manifest.json"] {
        fs::copy(cache.join(name), moved.join(name)).unwrap();
    }
    assert!(acquisition::fetch(&plan_path, &moved).is_ok());
    fs::write(moved.join(&plan.assets[0].name), b"abd").unwrap();
    assert!(acquisition::fetch(&plan_path, &moved).is_err());
}

#[test]
fn changed_plan_or_raw_metadata_cannot_trigger_download() {
    let scratch = Scratch::new();
    let (path, mut plan) = offline_plan(&scratch.0.join("plan"));
    plan.assets[0].url = "https://attacker.test/a.tar".into();
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    assert!(acquisition::fetch(&path, &scratch.0.join("cache")).is_err());
    assert!(!scratch.0.join("cache").exists());
    let (path, _) = offline_plan(&scratch.0.join("second-plan"));
    fs::write(path.parent().unwrap().join("release.json"), b"{}").unwrap();
    assert!(acquisition::fetch(&path, &scratch.0.join("cache")).is_err());
}

#[test]
fn mismatched_cache_manifest_is_not_overwritten() {
    let scratch = Scratch::new();
    let (path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(&plan.assets[0].name), b"abc").unwrap();
    let mut manifest = acquisition::fetch(&path, &cache).unwrap();
    manifest.date_utc = "2026-09-09".into();
    let manifest_path = cache.join("archive-manifest.json");
    let before = serde_json::to_vec(&manifest).unwrap();
    fs::write(&manifest_path, &before).unwrap();
    assert!(acquisition::fetch(&path, &cache).is_err());
    assert_eq!(fs::read(manifest_path).unwrap(), before);
}

#[test]
fn unowned_partial_download_is_preserved_and_rejected() {
    let scratch = Scratch::new();
    let (path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    let partial = cache.join(format!("{}.part", plan.assets[0].name));
    fs::write(&partial, b"ab").unwrap();
    assert!(
        acquisition::fetch(&path, &cache)
            .unwrap_err()
            .contains("identity receipt")
    );
    assert_eq!(fs::read(partial).unwrap(), b"ab");
}

#[test]
fn complete_owned_partial_is_verified_and_promoted_without_network() {
    let scratch = Scratch::new();
    let (path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    let asset = &plan.assets[0];
    fs::write(cache.join(format!("{}.part", asset.name)), b"abc").unwrap();
    fs::write(
        cache.join(format!("{}.part-identity.json", asset.name)),
        serde_json::to_vec(asset).unwrap(),
    )
    .unwrap();
    assert!(acquisition::fetch(&path, &cache).is_ok());
    assert_eq!(fs::read(cache.join(&asset.name)).unwrap(), b"abc");
}

#[test]
fn interrupted_manifest_write_is_preserved_and_completion_retried() {
    let scratch = Scratch::new();
    let (path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(&plan.assets[0].name), b"abc").unwrap();
    let pending = cache.join("archive-manifest.json.pending");
    fs::write(&pending, b"{\"schema_version\":").unwrap();
    let manifest = acquisition::fetch(&path, &cache).unwrap();
    assert!(manifest.complete && manifest.full_sha_verified);
    assert!(cache.join("archive-manifest.json").exists());
    assert_eq!(fs::read(pending).unwrap(), b"{\"schema_version\":");
    assert!(acquisition::fetch(&path, &cache).is_ok());
}

#[test]
fn cached_manifest_requires_a_valid_original_plan_hash() {
    let scratch = Scratch::new();
    let (path, plan) = offline_plan(&scratch.0.join("plan"));
    let cache = scratch.0.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(&plan.assets[0].name), b"abc").unwrap();
    let mut manifest = acquisition::fetch(&path, &cache).unwrap();
    manifest.plan_sha256 = "unverified".into();
    let manifest_path = cache.join("archive-manifest.json");
    let before = serde_json::to_vec(&manifest).unwrap();
    fs::write(&manifest_path, &before).unwrap();
    assert!(acquisition::fetch(&path, &cache).is_err());
    assert_eq!(fs::read(manifest_path).unwrap(), before);
}

#[test]
fn preferred_list_binds_all_asset_urls_including_the_final_part() {
    let scratch = Scratch::new();
    let (path, mut plan) = offline_plan(&scratch.0.join("plan"));
    plan.request.instance = None;
    bind_request(path.parent().unwrap(), &mut plan);
    let preferred = path.parent().unwrap().join("preferred-releases.txt");
    let cache = scratch.0.join("cache");
    // The release says one unsplit file, but the provider list says a second
    // asset belongs to this archive. Do not download a knowingly incomplete set.
    fs::write(
        &preferred,
        format!("{},{}.ab", plan.assets[0].url, plan.assets[0].url),
    )
    .unwrap();
    plan.metadata.push(MetadataReceipt {
        path: "preferred-releases.txt".into(),
        bytes: fs::metadata(&preferred).unwrap().len(),
        sha256: acquisition::sha256_file(&preferred).unwrap(),
    });
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    assert!(
        acquisition::fetch(&path, &cache)
            .unwrap_err()
            .contains("part list differs")
    );
    assert!(!cache.exists());

    fs::write(&preferred, &plan.assets[0].url).unwrap();
    let receipt = plan.metadata.last_mut().unwrap();
    receipt.bytes = fs::metadata(&preferred).unwrap().len();
    receipt.sha256 = acquisition::sha256_file(&preferred).unwrap();
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(&plan.assets[0].name), b"abc").unwrap();
    assert!(acquisition::fetch(&path, &cache).is_ok());
}

#[test]
fn changed_request_inside_plan_cannot_override_bound_original_request() {
    let scratch = Scratch::new();
    let (path, mut plan) = offline_plan(&scratch.0.join("plan"));
    plan.request.max_download_bytes += 1;
    fs::write(&path, serde_json::to_vec(&plan).unwrap()).unwrap();
    let cache = scratch.0.join("cache");
    assert!(
        acquisition::fetch(&path, &cache)
            .unwrap_err()
            .contains("original request receipt")
    );
    assert!(!cache.exists());
}
