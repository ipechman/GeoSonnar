use geosonnar::{map_data, physics::geodetic_to_ecef};
use serde_json::json;

fn input() -> serde_json::Value {
    json!({"schema_version":1,"config":{"time_origin_utc_s":1788393600.,"max_gap_s":5.},"passes":[{"track":{"flight_id":"fabricated","points":[{"t_s":10.,"xyz_m":geodetic_to_ecef(45.,-73.,1000.).unwrap()},{"t_s":12.,"xyz_m":geodetic_to_ecef(45.001,-73.001,1100.).unwrap()},{"t_s":30.,"xyz_m":geodetic_to_ecef(45.002,-73.002,1200.).unwrap()}]}}]})
}
#[test]
fn normalized_tracks_preserve_exact_physics_time_origin_and_gaps() {
    let source = input();
    let map = map_data::parse_value(&source).unwrap();
    let track = &map.tracks[0];
    assert_eq!(track.segments, vec![[0, 2], [2, 3]]);
    assert_eq!(track.max_gap_s, Some(5.));
    assert_eq!(track.points[0].time_utc_s, Some(1788393610.));
    assert!((track.points[0].latitude_deg - 45.).abs() < 1e-9);
    assert!((track.points[0].longitude_deg + 73.).abs() < 1e-9);
    assert!((track.points[0].height_m.unwrap() - 1000.).abs() < 1e-5);
    assert_eq!(
        serde_json::to_value(&track.physics_track).unwrap(),
        source["passes"][0]["track"]
    );
    assert!(!map.location_verified);
}
#[test]
fn absent_origin_or_gap_is_not_invented_and_conflicting_origin_is_rejected() {
    let mut source = input();
    source["config"]
        .as_object_mut()
        .unwrap()
        .remove("time_origin_utc_s");
    source["config"]
        .as_object_mut()
        .unwrap()
        .remove("max_gap_s");
    let map = map_data::parse_value(&source).unwrap();
    assert!(map.tracks[0].points.iter().all(|p| p.time_utc_s.is_none()));
    assert_eq!(map.tracks[0].segments, vec![[0, 1], [1, 2], [2, 3]]);
    assert!(map.tracks[0].max_gap_s.is_none());
    assert_eq!(
        map_data::parse_value_with_origin(&source, Some(1000.))
            .unwrap()
            .tracks[0]
            .points[0]
            .time_utc_s,
        Some(1010.)
    );
    assert!(map_data::parse_value_with_origin(&input(), Some(1000.)).is_err());
}
#[test]
fn invalid_local_frames_order_origins_and_shapes_are_rejected() {
    for variant in 0..5 {
        let mut source = input();
        match variant {
            0 => source["passes"][0]["track"]["points"][0]["xyz_m"] = json!([0., 100., 200.]),
            1 => source["passes"][0]["track"]["points"][1]["t_s"] = 10.0.into(),
            2 => source["config"]["time_origin_utc_s"] = "local clock".into(),
            3 => source["passes"] = json!({"not":"an array"}),
            _ => source["passes"][0]["track"]["points"] = json!([]),
        }
        assert!(map_data::parse_value(&source).is_err(), "variant {variant}");
    }
}
#[test]
fn inverse_coordinates_handle_poles_and_negative_heights() {
    for point in [
        [0., 180., 0.],
        [89.999, -170., 5000.],
        [-90., 0., 100.],
        [40., -75., -50.],
    ] {
        let converted =
            map_data::ecef_to_geodetic(geodetic_to_ecef(point[0], point[1], point[2]).unwrap())
                .unwrap();
        assert!((converted[0] - point[0]).abs() < 1e-8);
        assert!((converted[2] - point[2]).abs() < 1e-4);
    }
}
#[test]
fn observer_grid_spacing_and_candidate_scores_are_explicit() {
    let value = json!({"maximum_grid_spacing_m":100.,"observers":[{"latitude_deg":45.,"longitude_deg":-73.},{"latitude_deg":45.,"longitude_deg":-72.},{"latitude_deg":46.,"longitude_deg":-73.},{"latitude_deg":46.,"longitude_deg":-72.}],"candidates":[{"observer":{"id":"candidate-a","latitude_deg":45.5,"longitude_deg":-72.5,"ellipsoid_height_m":20.},"score":{"combined_rmse_hz":0.7,"flight_ids":["one","two"]}}]});
    let map = map_data::parse_value(&value).unwrap();
    let grid = map.grid.unwrap();
    assert!(grid.regular_spacing);
    assert_eq!(grid.latitude_step_deg, Some(1.));
    assert_eq!(grid.declared_spacing_m, Some(100.));
    assert_eq!(map.candidates[0].score, Some(0.7));
    assert_eq!(map.candidates[0].flight_ids, vec!["one", "two"]);
    assert!(!map.location_verified);
}
#[test]
fn dateline_extent_does_not_cover_the_long_way_round() {
    let map=map_data::parse_value(&json!({"observers":[{"latitude_deg":0.,"longitude_deg":179.},{"latitude_deg":1.,"longitude_deg":-179.}]})).unwrap();
    assert_eq!(map.grid.unwrap().bounds, [0., 179., 1., -179.]);
}
#[test]
fn terrain_extent_and_lightning_points_use_real_coordinates() {
    let map=map_data::parse_value(&json!({"width":3,"height":2,"sample_zero_lon_lat":[-73.,46.],"step_deg":[0.01,0.02],"nodata":-9999})).unwrap();
    assert_eq!(map.terrain.unwrap().bounds, [45.98, -73., 46., -72.98]);
    let map=map_data::parse_value(&json!({"schema_version":1,"strikes":[{"id":"flash","time_utc_s":1788393600.,"latitude_deg":45.,"longitude_deg":-73.,"altitude_m":null,"intensity":null}]})).unwrap();
    assert_eq!(map.lightning[0].time_utc_s, 1788393600.);
    assert!(map.lightning[0].altitude_m.is_none());
}

#[test]
fn standalone_native_observers_are_supported_without_inventing_resolution() {
    let map=map_data::parse_value(&json!([{"id":"a","latitude_deg":0.,"longitude_deg":1.,"ellipsoid_height_m":10.},{"id":"b","latitude_deg":2.,"longitude_deg":3.,"ellipsoid_height_m":20.}])).unwrap();
    assert_eq!(map.observers.len(), 2);
    let grid = map.grid.unwrap();
    assert!(!grid.regular_spacing);
    assert!(grid.declared_spacing_m.is_none());
}

#[test]
fn contradictory_origins_and_utc_overflow_are_rejected() {
    let mut source = input();
    source["time_origin_utc_s"] = json!(123.);
    assert!(map_data::parse_value(&source).is_err());
    source.as_object_mut().unwrap().remove("time_origin_utc_s");
    source["passes"][0]["track"]["points"][2]["t_s"] = json!(1e308);
    assert!(map_data::parse_value(&source).is_err());
}
