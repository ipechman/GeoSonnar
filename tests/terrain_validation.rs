use geosonnar::terrain::{GeographicRaster, observer_grid};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn height_binding(file: &Fixture) -> geosonnar::workspace::SourceBinding {
    geosonnar::workspace::SourceBinding {
        path: file.path.canonicalize().unwrap(),
        sha256: geosonnar::detailed_data::sha256(&file.path).unwrap(),
    }
}

fn receiver(latitude_deg: f64, longitude_deg: f64) -> geosonnar::evidence_score::ObserverLocation {
    geosonnar::evidence_score::ObserverLocation {
        latitude_deg,
        longitude_deg,
        ellipsoid_height_m: 777.0,
    }
}

#[test]
fn explicit_receiver_terrain_converts_compatible_orthometric_height() {
    use geosonnar::receiver_heights::*;
    let surface = fixture(Spec {
        constant: Some(40.0),
        ..Spec::default()
    });
    let geoid = fixture(Spec {
        constant: Some(-30.0),
        ..Spec::default()
    });
    let bindings = vec![height_binding(&surface), height_binding(&geoid)];
    let settings = ReceiverTerrain {
        surface_sha256s: vec![bindings[0].sha256.clone()],
        surface_datum: SurfaceDatum::Egm2008,
        geoid: Some(GeoidSource {
            source_sha256: bindings[1].sha256.clone(),
            reference: SurfaceDatum::Egm2008,
        }),
        above_surface_m: 3.0,
        notes: "Synthetic datum declaration".into(),
    };
    let coordinates = vec![receiver(40.625, -74.625), receiver(40.5, -74.5)];
    let report = resolve(Some(&settings), &bindings, &coordinates).unwrap();
    assert_eq!(report.sources.len(), 2);
    assert!(report.unavailable.is_empty());
    for p in report.complete_observers().unwrap() {
        assert_eq!(p.ellipsoid_height_m, 13.0);
    }
    assert_eq!(coordinates[0].ellipsoid_height_m, 777.0);
    assert_eq!(report.samples[0].geoid_undulation_m, -30.0);
    let restored: ReceiverHeightReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(restored.settings, Some(settings));
}

#[test]
fn receiver_height_datums_cannot_be_silently_mixed_or_corrected_twice() {
    use geosonnar::receiver_heights::*;
    let mut settings = ReceiverTerrain {
        surface_sha256s: vec!["1".repeat(64)],
        surface_datum: SurfaceDatum::Egm2008,
        geoid: Some(GeoidSource {
            source_sha256: "2".repeat(64),
            reference: SurfaceDatum::Egm96,
        }),
        above_surface_m: 1.0,
        notes: String::new(),
    };
    assert!(settings.validate().is_err());
    settings.surface_datum = SurfaceDatum::Wgs84Ellipsoid;
    assert!(settings.validate().is_err());
    settings.geoid = None;
    assert!(settings.validate().is_ok());
    settings.surface_datum = SurfaceDatum::Egm96;
    assert!(settings.validate().is_err());
}

#[test]
fn ellipsoid_surface_needs_no_geoid_and_missing_grid_cells_block_search() {
    use geosonnar::receiver_heights::*;
    let surface = fixture(Spec {
        constant: Some(40.0),
        ..Spec::default()
    });
    let bindings = vec![height_binding(&surface)];
    let settings = ReceiverTerrain {
        surface_sha256s: vec![bindings[0].sha256.clone()],
        surface_datum: SurfaceDatum::Wgs84Ellipsoid,
        geoid: None,
        above_surface_m: 3.0,
        notes: String::new(),
    };
    let report = resolve(
        Some(&settings),
        &bindings,
        &[receiver(40.5, -74.5), receiver(30.0, -70.0)],
    )
    .unwrap();
    assert_eq!(
        report.observers[0].as_ref().unwrap().ellipsoid_height_m,
        43.0
    );
    assert!(report.observers[1].is_none());
    assert_eq!(report.unavailable[0].observer_index, 1);
    assert!(report.complete_observers().unwrap_err().contains("1 of 2"));
}

#[test]
fn conflicting_surface_overlap_and_nodata_are_not_arbitrarily_selected() {
    use geosonnar::receiver_heights::*;
    let surface1 = fixture(Spec {
        constant: Some(40.0),
        ..Spec::default()
    });
    let surface2 = fixture(Spec {
        constant: Some(45.0),
        ..Spec::default()
    });
    let bindings = vec![height_binding(&surface1), height_binding(&surface2)];
    let mut settings = ReceiverTerrain {
        surface_sha256s: bindings.iter().map(|b| b.sha256.clone()).collect(),
        surface_datum: SurfaceDatum::Wgs84Ellipsoid,
        geoid: None,
        above_surface_m: 0.0,
        notes: String::new(),
    };
    let point = [receiver(40.5, -74.5)];
    let report = resolve(Some(&settings), &bindings, &point).unwrap();
    assert!(report.unavailable[0].reason.contains("disagree"));
    let missing = fixture(Spec {
        constant: Some(-999.0),
        nodata: Some("-999".into()),
        ..Spec::default()
    });
    let binding = height_binding(&missing);
    settings.surface_sha256s = vec![binding.sha256.clone()];
    let report = resolve(Some(&settings), &[binding], &point).unwrap();
    assert!(report.observers[0].is_none());
    assert!(report.unavailable[0].reason.contains("Missing terrain"));
}

#[test]
fn receiver_height_source_identity_is_verified_and_fixed_mode_stays_explicit() {
    use geosonnar::receiver_heights::*;
    let surface = fixture(Spec {
        constant: Some(40.0),
        ..Spec::default()
    });
    let binding = height_binding(&surface);
    let settings = ReceiverTerrain {
        surface_sha256s: vec![binding.sha256.clone()],
        surface_datum: SurfaceDatum::Wgs84Ellipsoid,
        geoid: None,
        above_surface_m: 3.0,
        notes: String::new(),
    };
    let point = [receiver(40.5, -74.5)];
    assert!(resolve(Some(&settings), &[], &point).is_err());
    let mut bytes = fs::read(&surface.path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(&surface.path, bytes).unwrap();
    assert!(
        resolve(Some(&settings), &[binding], &point)
            .unwrap_err()
            .contains("changed")
    );
    let fixed = resolve(None, &[], &point).unwrap();
    assert_eq!(
        fixed.complete_observers().unwrap()[0].ellipsoid_height_m,
        777.0
    );
    assert!(fixed.sources.is_empty());
}

struct Fixture {
    path: PathBuf,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Clone)]
struct Spec {
    width: u32,
    height: u32,
    tiled: bool,
    pixel_type: u16,
    epsg: u16,
    orientation: u16,
    transform: bool,
    nodata: Option<String>,
    nan_pixel: bool,
    constant: Option<f32>,
    chunk_side: u32,
    step: f64,
}
impl Default for Spec {
    fn default() -> Self {
        Self {
            width: 4,
            height: 4,
            tiled: true,
            pixel_type: 2,
            epsg: 4326,
            orientation: 1,
            transform: false,
            nodata: None,
            nan_pixel: false,
            constant: None,
            chunk_side: 2,
            step: 0.25,
        }
    }
}

fn shorts(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn longs(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn doubles(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

// Independent tiny classic-TIFF writer: no shared encoder/georeferencing code.
fn fixture(spec: Spec) -> Fixture {
    let chunk_width = if spec.tiled {
        spec.chunk_side
    } else {
        spec.width
    };
    let chunk_height = spec.chunk_side;
    let across = spec.width.div_ceil(chunk_width);
    let down = spec.height.div_ceil(chunk_height);
    let mut chunks = Vec::new();
    for north in 0..down {
        for east in 0..across {
            let rows = if spec.tiled {
                chunk_height
            } else {
                chunk_height.min(spec.height - north * chunk_height)
            };
            let mut bytes = Vec::new();
            for y in 0..rows {
                for x in 0..chunk_width {
                    let (row, col) = (north * chunk_height + y, east * chunk_width + x);
                    let value = if row >= spec.height || col >= spec.width {
                        9999.0
                    } else if spec.nan_pixel && row == 1 && col == 1 {
                        f32::NAN
                    } else {
                        spec.constant.unwrap_or((row * 10 + col) as f32)
                    };
                    bytes.extend(value.to_le_bytes());
                }
            }
            chunks.push(bytes);
        }
    }
    let mut tags: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (256, 4, 1, longs(&[spec.width])),
        (257, 4, 1, longs(&[spec.height])),
        (258, 3, 1, shorts(&[32])),
        (259, 3, 1, shorts(&[1])),
        (262, 3, 1, shorts(&[1])),
        (274, 3, 1, shorts(&[spec.orientation])),
        (277, 3, 1, shorts(&[1])),
        (284, 3, 1, shorts(&[1])),
        (339, 3, 1, shorts(&[3])),
        (33550, 12, 3, doubles(&[spec.step, spec.step, 0.0])),
        (33922, 12, 6, doubles(&[0.0, 0.0, 0.0, -75.0, 41.0, 0.0])),
        (
            34735,
            3,
            16,
            shorts(&[
                1,
                1,
                0,
                3,
                1024,
                0,
                1,
                2,
                1025,
                0,
                1,
                spec.pixel_type,
                2048,
                0,
                1,
                spec.epsg,
            ]),
        ),
    ];
    let offsets_tag;
    if spec.tiled {
        offsets_tag = 324;
        tags.extend([
            (322, 4, 1, longs(&[chunk_width])),
            (323, 4, 1, longs(&[chunk_height])),
            (324, 4, chunks.len() as u32, longs(&vec![0; chunks.len()])),
            (
                325,
                4,
                chunks.len() as u32,
                longs(&chunks.iter().map(|v| v.len() as u32).collect::<Vec<_>>()),
            ),
        ]);
    } else {
        offsets_tag = 273;
        tags.extend([
            (278, 4, 1, longs(&[chunk_height])),
            (273, 4, chunks.len() as u32, longs(&vec![0; chunks.len()])),
            (
                279,
                4,
                chunks.len() as u32,
                longs(&chunks.iter().map(|v| v.len() as u32).collect::<Vec<_>>()),
            ),
        ]);
    }
    if let Some(text) = spec.nodata {
        let mut bytes = text.into_bytes();
        bytes.push(0);
        tags.push((42113, 2, bytes.len() as u32, bytes));
    }
    if spec.transform {
        tags.push((34264, 12, 16, doubles(&[1.0; 16])));
    }
    tags.sort_by_key(|t| t.0);
    let external_start = 8 + 2 + tags.len() * 12 + 4;
    let pixel_start = external_start
        + tags
            .iter()
            .filter(|t| t.3.len() > 4)
            .map(|t| t.3.len().next_multiple_of(2))
            .sum::<usize>();
    let mut at = pixel_start as u32;
    let mut offsets = Vec::new();
    for chunk in &chunks {
        offsets.push(at);
        at += chunk.len() as u32;
    }
    tags.iter_mut().find(|t| t.0 == offsets_tag).unwrap().3 = longs(&offsets);
    let mut bytes = b"II".to_vec();
    bytes.extend(42u16.to_le_bytes());
    bytes.extend(8u32.to_le_bytes());
    bytes.extend((tags.len() as u16).to_le_bytes());
    let mut external = Vec::new();
    for (tag, kind, count, value) in tags {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        if value.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..value.len()].copy_from_slice(&value);
            bytes.extend(inline);
        } else {
            bytes.extend(((external_start + external.len()) as u32).to_le_bytes());
            external.extend(value);
            if external.len() % 2 != 0 {
                external.push(0);
            }
        }
    }
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(external);
    assert_eq!(bytes.len(), pixel_start);
    for chunk in chunks {
        bytes.extend(chunk);
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tmp");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!(
        "terrain-control-{}-{}.tif",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    fs::write(&path, bytes).unwrap();
    Fixture { path }
}

fn patch_inline(file: &Fixture, tag: u16, value: u32) {
    let mut bytes = fs::read(&file.path).unwrap();
    let count = u16::from_le_bytes(bytes[8..10].try_into().unwrap()) as usize;
    let offset = (0..count)
        .map(|i| 10 + i * 12)
        .find(|&i| u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap()) == tag)
        .unwrap();
    bytes[offset + 8..offset + 12].copy_from_slice(&value.to_le_bytes());
    fs::write(&file.path, bytes).unwrap();
}

#[test]
fn bilinear_sampling_crosses_tile_edges_without_padding_leakage() {
    let file = fixture(Spec {
        width: 3,
        height: 3,
        ..Spec::default()
    });
    let mut raster = GeographicRaster::open(&file.path).unwrap();
    assert_eq!(raster.sample(41.0, -75.0).unwrap(), 0.0);
    assert_eq!(raster.sample(40.625, -74.625).unwrap(), 16.5);
    assert_eq!(raster.sample(40.5, -74.5).unwrap(), 22.0);
    assert!(raster.cached_chunk_count() <= 4);
    assert_eq!(raster.decoded_chunk_count(), 4);
    assert!(raster.sample(40.49, -74.5).is_err());
    raster.verify_unchanged().unwrap();
}

#[test]
fn strips_and_tiles_produce_identical_physical_samples() {
    let tiled = fixture(Spec {
        width: 3,
        height: 3,
        ..Spec::default()
    });
    let striped = fixture(Spec {
        width: 3,
        height: 3,
        tiled: false,
        ..Spec::default()
    });
    let mut one = GeographicRaster::open(&tiled.path).unwrap();
    let mut two = GeographicRaster::open(&striped.path).unwrap();
    for (lat, lon) in [(41.0, -75.0), (40.9, -74.8), (40.6, -74.6), (40.5, -74.5)] {
        assert_eq!(one.sample(lat, lon).unwrap(), two.sample(lat, lon).unwrap());
    }
}

#[test]
fn pixel_is_area_uses_half_pixel_sample_centres() {
    let file = fixture(Spec {
        pixel_type: 1,
        ..Spec::default()
    });
    let mut raster = GeographicRaster::open(&file.path).unwrap();
    assert_eq!(raster.metadata().sample_zero_lon_lat, [-74.875, 40.875]);
    assert_eq!(raster.sample(40.875, -74.875).unwrap(), 0.0);
    assert_eq!(raster.sample(40.75, -74.75).unwrap(), 5.5);
    assert!(raster.sample(41.0, -75.0).is_err());
}

#[test]
fn missing_samples_and_outside_support_never_become_zero() {
    for spec in [
        Spec {
            nodata: Some("11".into()),
            ..Spec::default()
        },
        Spec {
            nan_pixel: true,
            ..Spec::default()
        },
    ] {
        let file = fixture(spec);
        let mut raster = GeographicRaster::open(&file.path).unwrap();
        assert!(raster.sample(40.8, -74.8).is_err());
        assert!(raster.sample(f64::NAN, -75.0).is_err());
        assert!(raster.sample(41.0, -180.1).is_err());
        assert!(raster.sample(40.0, -75.0).is_err());
    }
}

#[test]
fn unsupported_coordinates_and_transformations_are_rejected() {
    for spec in [
        Spec {
            epsg: 4269,
            ..Spec::default()
        },
        Spec {
            pixel_type: 0,
            ..Spec::default()
        },
        Spec {
            orientation: 3,
            ..Spec::default()
        },
        Spec {
            transform: true,
            ..Spec::default()
        },
    ] {
        let file = fixture(spec);
        assert!(GeographicRaster::open(&file.path).is_err());
    }
}

#[test]
fn invalid_dimensions_and_oversized_chunks_fail_before_decoding() {
    let file = fixture(Spec::default());
    patch_inline(&file, 256, 1_000_001);
    assert!(GeographicRaster::open(&file.path).is_err());
    let file = fixture(Spec::default());
    patch_inline(&file, 322, 5000);
    patch_inline(&file, 323, 5000);
    assert!(GeographicRaster::open(&file.path).is_err());
}

#[test]
fn copernicus_sized_padded_tiles_decode_with_a_finite_memory_budget() {
    let file = fixture(Spec {
        width: 3,
        height: 3,
        chunk_side: 2048,
        ..Spec::default()
    });
    let mut raster = GeographicRaster::open(&file.path).unwrap();
    assert_eq!(raster.metadata().chunk_dimensions, [2048, 2048]);
    assert_eq!(raster.metadata().maximum_decoded_chunk_pixels, 4_194_304);
    assert_eq!(raster.sample(40.5, -74.5).unwrap(), 22.0);
    assert_eq!(raster.cached_chunk_count(), 1);
    drop(raster);
    patch_inline(&file, 322, 4096);
    assert!(GeographicRaster::open(&file.path).is_err());
}

#[test]
fn downloaded_or_cached_copernicus_tiles_must_match_region_sampling_and_decoding() {
    use geosonnar::data_catalog::requests::{
        TerrainResolution, plan_terrain, validate_terrain_tile,
    };
    let file = fixture(Spec {
        width: 1200,
        height: 1200,
        chunk_side: 2048,
        step: 1.0 / 1200.0,
        constant: Some(32.0),
        ..Spec::default()
    });
    let plan = plan_terrain(
        [40., -75., 41., -74.],
        TerrainResolution::Glo90,
        100_000_000,
    )
    .unwrap();
    validate_terrain_tile(&file.path, &plan.tiles[0], TerrainResolution::Glo90).unwrap();
    let wrong = plan_terrain(
        [41., -75., 42., -74.],
        TerrainResolution::Glo90,
        100_000_000,
    )
    .unwrap();
    assert!(
        validate_terrain_tile(&file.path, &wrong.tiles[0], TerrainResolution::Glo90)
            .unwrap_err()
            .contains("geographic support")
    );
    assert!(validate_terrain_tile(&file.path, &plan.tiles[0], TerrainResolution::Glo30).is_err());
    fs::write(&file.path, b"not a terrain raster").unwrap();
    assert!(validate_terrain_tile(&file.path, &plan.tiles[0], TerrainResolution::Glo90).is_err());
}

#[test]
fn chunk_cache_is_bounded_when_crossing_many_tiles() {
    let file = fixture(Spec {
        width: 10,
        height: 10,
        ..Spec::default()
    });
    let mut raster = GeographicRaster::open(&file.path).unwrap();
    for row in 0..10 {
        for col in 0..10 {
            assert_eq!(
                raster
                    .sample(41.0 - f64::from(row) * 0.25, -75.0 + f64::from(col) * 0.25)
                    .unwrap(),
                f64::from(row * 10 + col)
            );
            assert!(raster.cached_chunk_count() <= 4);
        }
    }
    assert!(raster.decoded_chunk_count() > 4);
}

#[test]
fn observer_grid_reconciles_declared_heights_and_preserves_boundaries() {
    let a = fixture(Spec {
        constant: Some(100.0),
        ..Spec::default()
    });
    let b = fixture(Spec {
        constant: Some(-30.0),
        ..Spec::default()
    });
    let mut dem = GeographicRaster::open(&a.path).unwrap();
    let mut geoid = GeographicRaster::open(&b.path).unwrap();
    let grid = observer_grid(
        &mut dem,
        &mut geoid,
        [40.5, -75.0, 41.0, -74.5],
        30_000.0,
        3.0,
    )
    .unwrap();
    assert_eq!(grid.observers.len(), 9);
    assert_eq!(grid.observers[0].latitude_deg, 40.5);
    assert_eq!(grid.observers[0].longitude_deg, -75.0);
    assert_eq!(grid.observers.last().unwrap().latitude_deg, 41.0);
    assert_eq!(grid.observers.last().unwrap().longitude_deg, -74.5);
    assert!(grid.observers.iter().all(|o| o.ellipsoid_height_m == 73.0));
    assert!(!grid.location_verified);
    assert!(observer_grid(&mut dem, &mut geoid, [40.5, -75.0, 41.0, -74.5], 0.01, 3.0).is_err());
    assert!(
        observer_grid(
            &mut dem,
            &mut geoid,
            [40.5, 179.0, 41.0, -179.0],
            1000.0,
            3.0
        )
        .is_err()
    );
    assert!(
        observer_grid(
            &mut dem,
            &mut geoid,
            [40.0, -75.0, 41.0, -74.5],
            30_000.0,
            3.0
        )
        .is_err()
    );
}

#[test]
fn source_mutation_is_detected_before_committing_sampled_output() {
    let file = fixture(Spec::default());
    let raster = GeographicRaster::open(&file.path).unwrap();
    patch_inline(&file, 274, 3);
    assert!(raster.verify_unchanged().is_err());
}

#[test]
#[ignore = "Requires the separately downloaded, checksum-verified reference terrain files"]
fn cached_copernicus_and_egm96_match_independent_python_sampler() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/dopplergeist/terrain");
    let coordinates = [
        (40.83, -73.59),
        (40.92, -73.46),
        (40.875, -73.525),
        (40.9999, -73.9999),
        (40.001, -73.001),
    ];
    for (name, expected) in [
        (
            "Copernicus_DSM_COG_10_N40_00_W074_00_DEM.tif",
            [
                45.03089141844248,
                15.515583991984009,
                0.7154266238066184,
                36.19976397701862,
                0.0,
            ],
        ),
        (
            "us_nga_egm96_15.tif",
            [
                -31.86752901611329,
                -31.483282394409173,
                -31.657759284973153,
                -32.3117551990155,
                -34.866193742187505,
            ],
        ),
    ] {
        let mut raster = GeographicRaster::open(&root.join(name)).unwrap();
        for ((lat, lon), expected) in coordinates.into_iter().zip(expected) {
            let actual = raster.sample(lat, lon).unwrap();
            assert!(
                (actual - expected).abs() < 1e-8,
                "{name} {lat},{lon}: {actual} vs {expected}"
            );
        }
        assert!(raster.cached_chunk_count() <= 4);
        raster.verify_unchanged().unwrap();
    }
}

#[test]
#[ignore = "Requires cached reference terrain and the prior independent Python observer grid"]
fn cached_native_observer_grid_matches_all_independent_reference_heights() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dem = GeographicRaster::open(
        &root.join("data/dopplergeist/terrain/Copernicus_DSM_COG_10_N40_00_W074_00_DEM.tif"),
    )
    .unwrap();
    let mut geoid =
        GeographicRaster::open(&root.join("data/dopplergeist/terrain/us_nga_egm96_15.tif"))
            .unwrap();
    let expected: Vec<geosonnar::verification::Observer> = serde_json::from_slice(
        &fs::read(root.join("output/reference-data/observers.json")).unwrap(),
    )
    .unwrap();
    let actual = observer_grid(
        &mut dem,
        &mut geoid,
        [40.83, -73.59, 40.92, -73.46],
        200.0,
        3.0,
    )
    .unwrap();
    assert_eq!(actual.observers.len(), 2856);
    assert_eq!(actual.observers.len(), expected.len());
    for (actual, expected) in actual.observers.iter().zip(&expected) {
        assert_eq!(actual.id, expected.id);
        assert!((actual.latitude_deg - expected.latitude_deg).abs() < 1e-12);
        assert!((actual.longitude_deg - expected.longitude_deg).abs() < 1e-12);
        assert!(
            (actual.ellipsoid_height_m - expected.ellipsoid_height_m).abs() < 1e-8,
            "{} native height {} vs independent {}",
            actual.id,
            actual.ellipsoid_height_m,
            expected.ellipsoid_height_m
        );
    }
    assert!(dem.cached_chunk_count() <= 4);
    assert!(geoid.cached_chunk_count() <= 4);
}
