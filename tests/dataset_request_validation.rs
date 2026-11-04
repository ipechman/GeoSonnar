use geosonnar::{
    data_catalog::{
        self, DataKind, ImportRequest,
        requests::{self, LightningQuery, TerrainResolution},
    },
    detailed_data::sha256,
    workspace,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    manifest: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/dataset-request-controls")
            .join(format!(
                "{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("fabricated.json");
        fs::write(&source, b"{}\n").unwrap();
        let manifest = root.join("case.json");
        fs::write(&manifest,serde_json::to_vec(&json!({"schema_version":1,"case_id":"fabricated","title":"Dataset request control","created_unix_s":1,"local_only":true,"recording":{"path":"fabricated.json","sha256":sha256(&source).unwrap(),"bytes":3,"media":{"duration_s":1.,"sample_rate_hz":48000,"channels":1,"audio_codec":"fabricated"}}})).unwrap()).unwrap();
        Self { root, manifest }
    }
    fn export(&self) -> data_catalog::CatalogEntry {
        let start = workspace::parse_utc("2026-05-12 12:00:00").unwrap();
        let path = self.root.join("strikes.csv");
        fs::write(&path,format!("id,time_utc_s,latitude_deg,longitude_deg,altitude_m,intensity\nstart,{start},45,-73,100,2\nend,{},46,-72,,\noutside_time,{},45,-73,,\noutside_area,{},20,-73,,\n",start+60.,start+61.,start+20.)).unwrap();
        data_catalog::import_file(
            &self.manifest,
            &path,
            &ImportRequest {
                label: "Fabricated lightning".into(),
                kind: DataKind::Lightning,
                date_utc: None,
                bounds: None,
                notes: "Fabricated control".into(),
                max_bytes: 4096,
            },
        )
        .unwrap()
    }
}
fn query() -> LightningQuery {
    LightningQuery {
        provider: "Fabricated provider".into(),
        start_utc: "2026-05-12 12:00:00".into(),
        end_utc: "2026-05-12 12:01:00".into(),
        bounds: [45., -73., 46., -72.],
    }
}

#[test]
fn terrain_tiles_use_floor_coordinates_exclude_touching_edges_and_handle_date_line() {
    let plan =
        requests::plan_terrain([45.1, -73.9, 46., -73.], TerrainResolution::Glo90, 1000).unwrap();
    assert_eq!(plan.tiles.len(), 1);
    assert_eq!(
        plan.tiles[0].name,
        "Copernicus_DSM_COG_30_N45_00_W074_00_DEM"
    );
    assert_eq!(
        plan.tiles[0].url,
        "https://copernicus-dem-90m.s3.amazonaws.com/Copernicus_DSM_COG_30_N45_00_W074_00_DEM/Copernicus_DSM_COG_30_N45_00_W074_00_DEM.tif"
    );
    let southern =
        requests::plan_terrain([-0.2, 0.1, 0., 1.], TerrainResolution::Glo30, 1000).unwrap();
    assert!(southern.tiles[0].name.contains("10_S01_00_E000"));
    let crossing =
        requests::plan_terrain([0., 179.5, 1., -179.5], TerrainResolution::Glo90, 1000).unwrap();
    assert_eq!(crossing.tiles.len(), 2);
    assert_eq!(crossing.tiles[0].bounds, [0., 179., 1., 180.]);
    assert_eq!(crossing.tiles[1].bounds, [0., -180., 1., -179.]);
}

#[test]
fn terrain_plan_limits_and_mutations_fail_before_network_or_catalog_creation() {
    let f = Fixture::new();
    for bounds in [
        [-90., -180., 90., 180.],
        [f64::NAN, 0., 1., 1.],
        [0., 0., 0., 1.],
        [0., 0., 1., 181.],
    ] {
        assert!(requests::plan_terrain(bounds, TerrainResolution::Glo90, 1000).is_err());
    }
    assert!(requests::plan_terrain([0., 0., 1., 1.], TerrainResolution::Glo90, 0).is_err());
    let mut plan =
        requests::plan_terrain([0., 0., 1., 1.], TerrainResolution::Glo90, 1000).unwrap();
    plan.tiles[0].url = "https://unrelated.invalid/payload".into();
    assert!(
        requests::download_terrain_with_cancel(&f.manifest, &plan, &AtomicBool::new(false))
            .unwrap_err()
            .contains("plan changed")
    );
    let plan = requests::plan_terrain([0., 0., 1., 1.], TerrainResolution::Glo90, 1000).unwrap();
    assert!(
        requests::download_terrain_with_cancel(&f.manifest, &plan, &AtomicBool::new(true))
            .unwrap_err()
            .contains("canceled")
    );
    assert!(!f.root.join("managed-data").exists());
}

#[test]
fn lightning_selection_preserves_source_and_actual_boundary_values_with_provenance() {
    let f = Fixture::new();
    let raw = f.export();
    let source = data_catalog::verified_files(&f.manifest, &raw.id)
        .unwrap()
        .remove(0);
    let before = sha256(&source).unwrap();
    let subset = requests::filter_lightning_export(
        &f.manifest,
        &raw.id,
        &query(),
        "Licensed fabricated control",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(subset.source["selected_strikes"], 2);
    assert_eq!(subset.source["original_strikes"], 4);
    assert_eq!(subset.source["provider_coverage_verified"], false);
    assert_eq!(subset.source["filters_applied_locally"], true);
    assert_eq!(subset.source["source_sha256"], before);
    let paths = data_catalog::verified_files(&f.manifest, &subset.id).unwrap();
    let selected: Value = serde_json::from_slice(
        &fs::read(
            paths
                .iter()
                .find(|p| p.ends_with("lightning.json"))
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(selected["strikes"][0]["id"], "start");
    assert_eq!(selected["strikes"][1]["id"], "end");
    assert_eq!(selected["strikes"][0]["altitude_m"], 100.);
    assert_eq!(selected["strikes"][0]["intensity"], 2.);
    assert_eq!(sha256(&source).unwrap(), before);
    data_catalog::delete_owned(&f.manifest, &subset.id).unwrap();
    assert_eq!(sha256(&source).unwrap(), before);
}

#[test]
fn lightning_empty_invalid_and_canceled_selections_do_not_become_evidence() {
    let f = Fixture::new();
    let raw = f.export();
    let mut empty = query();
    empty.bounds = [0., 0., 1., 1.];
    let error = requests::filter_lightning_export(
        &f.manifest,
        &raw.id,
        &empty,
        "",
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(error.contains("no strikes"));
    let mut invalid = query();
    invalid.end_utc = invalid.start_utc.clone();
    assert!(invalid.validate().is_err());
    invalid = query();
    invalid.start_utc = "2026-02-30 00:00:00".into();
    assert!(invalid.validate().is_err());
    invalid = query();
    invalid.provider.clear();
    assert!(invalid.validate().is_err());
    assert!(
        requests::filter_lightning_export(
            &f.manifest,
            &raw.id,
            &query(),
            "",
            &AtomicBool::new(true)
        )
        .is_err()
    );
    assert_eq!(data_catalog::list(&f.manifest).unwrap().len(), 1);
    assert!(query().request_text().unwrap().contains("12:01:00"));
}

#[test]
fn lightning_explicit_export_format_and_limits_are_checked_before_download() {
    let f = Fixture::new();
    assert!(
        requests::download_lightning_with_cancel(
            &f.manifest,
            "https://example.invalid/export",
            requests::LightningFormat::Csv,
            &query(),
            "",
            requests::MAX_LIGHTNING_BYTES + 1,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(
        requests::download_lightning_with_cancel(
            &f.manifest,
            "http://example.invalid/export",
            requests::LightningFormat::Json,
            &query(),
            "",
            1024,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(!f.root.join("managed-data").exists());
}
