//! Explicit conversion of adsb.aero's simplified historical flight geometry.
//!
//! The provider's `path` vertices are longitude, latitude, pressure altitude in
//! feet. Its matching timestamps are Unix UTC seconds. Altitude correction is
//! an independent step series and gives approximate MSL height, not ellipsoidal
//! height. Neither path vertices nor derived segment velocities are raw ADS-B
//! measurements. Provenance and the chosen altitude assumption travel with the
//! normalized output so this adapter cannot silently imply exact replication.

use crate::physics::{Track, TrackPoint, geodetic_to_ecef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum AltitudeTreatment {
    /// h = pressure_alt_ft * 0.3048 + an explicitly chosen offset.
    /// This deliberately approximate route is useful for sensitivity studies;
    /// it is not a pressure-to-geometric-altitude correction.
    PressureApproximation {
        ellipsoid_offset_m: f64,
        uncertainty_m: f64,
        rationale: String,
    },
    /// h = (pressure_alt_ft + provider_QNH_correction_ft) * 0.3048 + N.
    /// N is the declared MSL-to-ellipsoid geoid undulation, positive when the
    /// geoid is above the ellipsoid. One constant N is only a local assumption.
    QnhMslWithGeoidOffset {
        geoid_undulation_m: f64,
        uncertainty_m: f64,
        rationale: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizationConfig {
    pub source_label: String,
    /// Common origin subtracted from every source timestamp, normally the
    /// chosen UTC midnight. All flights must use this same origin.
    pub time_origin_utc_s: f64,
    /// Split consecutive simplified vertices farther apart than this. A large
    /// vertex interval can represent simplification, not a raw reception gap.
    /// Provider MultiLineString boundaries are always preserved regardless.
    pub max_gap_s: f64,
    /// Split implausible 3D motion rather than interpolating through jumps.
    pub max_speed_mps: f64,
    pub altitude_treatment: AltitudeTreatment,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizedPass {
    pub track: Track,
    pub source_flight_id: String,
    pub icao24: String,
    pub callsign: Option<String>,
    pub aircraft_type: Option<String>,
    pub source_subsequence: usize,
    pub split_index: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizationIssue {
    pub source_flight_id: String,
    pub source_subsequence: Option<usize>,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NormalizationStats {
    pub source_flights: usize,
    pub source_subsequences: usize,
    pub source_vertices: usize,
    pub output_passes: usize,
    pub output_vertices: usize,
    pub duplicate_vertices_removed: usize,
    pub vertex_gap_splits: usize,
    pub speed_jump_splits: usize,
    pub singleton_chunks_discarded: usize,
    pub rejected_subsequences: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NormalizationReport {
    pub schema_version: u32,
    pub config: NormalizationConfig,
    pub coordinate_frame: String,
    pub timestamp_definition: String,
    pub quality_notes: Vec<String>,
    pub stats: NormalizationStats,
    pub issues: Vec<NormalizationIssue>,
    pub passes: Vec<NormalizedPass>,
}

#[derive(Deserialize)]
struct RawResponse {
    flights: Vec<RawFlight>,
}

#[derive(Deserialize)]
struct RawPath {
    #[serde(rename = "type")]
    geometry_type: String,
    coordinates: Vec<Vec<Vec<f64>>>,
}

#[derive(Deserialize)]
struct RawFlight {
    flight_id: String,
    icao24: String,
    #[serde(default)]
    callsign: Option<String>,
    #[serde(default)]
    icao_type: Option<String>,
    #[serde(default)]
    path: Option<RawPath>,
    #[serde(default)]
    timestamps: Option<Vec<Vec<f64>>>,
    #[serde(default)]
    alt_correction_ft: Option<Vec<Vec<[f64; 2]>>>,
}

fn validate_config(config: &NormalizationConfig) -> Result<(), String> {
    if config.source_label.trim().is_empty() {
        return Err("A source label identifying the saved raw flight response is required".into());
    }
    if !config.time_origin_utc_s.is_finite()
        || !(0.0..100_000_000_000.0).contains(&config.time_origin_utc_s)
    {
        return Err("The common UTC origin must be Unix seconds, not milliseconds".into());
    }
    if !config.max_gap_s.is_finite()
        || config.max_gap_s <= 0.0
        || !config.max_speed_mps.is_finite()
        || config.max_speed_mps <= 0.0
    {
        return Err("Maximum vertex gap and speed must be finite positive values".into());
    }
    let (offset, uncertainty, rationale) = match &config.altitude_treatment {
        AltitudeTreatment::PressureApproximation {
            ellipsoid_offset_m,
            uncertainty_m,
            rationale,
        } => (*ellipsoid_offset_m, *uncertainty_m, rationale),
        AltitudeTreatment::QnhMslWithGeoidOffset {
            geoid_undulation_m,
            uncertainty_m,
            rationale,
        } => (*geoid_undulation_m, *uncertainty_m, rationale),
    };
    if !offset.is_finite()
        || !uncertainty.is_finite()
        || uncertainty <= 0.0
        || rationale.trim().is_empty()
    {
        return Err("Altitude treatment requires a finite offset, positive uncertainty, and explicit rationale".into());
    }
    Ok(())
}

fn altitude_m(
    pressure_ft: f64,
    timestamp: f64,
    correction: Option<&[[f64; 2]]>,
    treatment: &AltitudeTreatment,
) -> Result<f64, String> {
    let altitude = match treatment {
        AltitudeTreatment::PressureApproximation {
            ellipsoid_offset_m, ..
        } => pressure_ft * 0.3048 + ellipsoid_offset_m,
        AltitudeTreatment::QnhMslWithGeoidOffset {
            geoid_undulation_m, ..
        } => {
            let series = correction.ok_or(
                "QNH-corrected height requested but provider correction series is missing",
            )?;
            if series.is_empty()
                || series
                    .iter()
                    .any(|p| !p[0].is_finite() || !p[1].is_finite())
                || series.windows(2).any(|p| p[1][0] <= p[0][0])
            {
                return Err(
                    "Provider QNH correction series is empty, unordered, or nonfinite".into(),
                );
            }
            let index = series.partition_point(|p| p[0] <= timestamp);
            if index == 0 {
                return Err(
                    "Provider QNH correction does not cover the start of the subsequence".into(),
                );
            }
            // Per the source contract, forward-fill each entry to the next
            // within this provider sub-sequence only. Never interpolate QNH.
            (pressure_ft + series[index - 1][1]) * 0.3048 + geoid_undulation_m
        }
    };
    if !altitude.is_finite() {
        return Err("Altitude conversion overflowed".into());
    }
    Ok(altitude)
}

fn convert_subsequence(
    coordinates: &[Vec<f64>],
    times: &[f64],
    corrections: Option<&[[f64; 2]]>,
    config: &NormalizationConfig,
) -> Result<(Vec<TrackPoint>, usize), String> {
    if coordinates.len() != times.len() {
        return Err("Coordinate and timestamp counts differ".into());
    }
    let mut points: Vec<TrackPoint> = Vec::with_capacity(times.len());
    let mut removed = 0;
    for (coordinate, &timestamp) in coordinates.iter().zip(times) {
        if coordinate.len() != 3 {
            return Err(
                "A source coordinate must be [longitude, latitude, pressure_altitude_ft]".into(),
            );
        }
        if !timestamp.is_finite() || !(0.0..100_000_000_000.0).contains(&timestamp) {
            return Err("Source timestamps must be Unix UTC seconds, not milliseconds".into());
        }
        let height = altitude_m(
            coordinate[2],
            timestamp,
            corrections,
            &config.altitude_treatment,
        )?;
        let xyz = geodetic_to_ecef(coordinate[1], coordinate[0], height)?;
        let point = TrackPoint {
            t_s: timestamp - config.time_origin_utc_s,
            xyz_m: xyz,
        };
        if let Some(previous) = points.last() {
            if point.t_s < previous.t_s {
                return Err("Source timestamp order decreases; refusing to sort across possible track discontinuity".into());
            }
            if point.t_s == previous.t_s {
                if point.xyz_m == previous.xyz_m {
                    removed += 1;
                    continue;
                }
                return Err("Conflicting positions at the same source timestamp".into());
            }
        }
        points.push(point);
    }
    Ok((points, removed))
}

fn finish_chunk(
    points: &mut Vec<TrackPoint>,
    flight: &RawFlight,
    subsequence: usize,
    split_index: usize,
    report: &mut NormalizationReport,
) {
    if points.len() < 2 {
        if !points.is_empty() {
            report.stats.singleton_chunks_discarded += 1;
        }
        points.clear();
        return;
    }
    report.stats.output_vertices += points.len();
    report.passes.push(NormalizedPass {
        track: Track {
            flight_id: format!(
                "{}#path{}#part{}",
                flight.flight_id, subsequence, split_index
            ),
            points: std::mem::take(points),
        },
        source_flight_id: flight.flight_id.clone(),
        icao24: flight.icao24.clone(),
        callsign: flight.callsign.clone(),
        aircraft_type: flight.icao_type.clone(),
        source_subsequence: subsequence,
        split_index,
    });
}

/// Normalize a saved `/api/v1/query` response or an aggregated object with a
/// `flights` array. Other response metadata is deliberately left in the saved
/// raw source; set `source_label` to its path or dataset identifier. Structurally
/// invalid JSON fails the call. Missing/malformed individual sub-sequences are
/// reported and skipped; callers must review `issues` and rejected counts.
pub fn normalize_adsb_aero(
    raw_json: &str,
    config: &NormalizationConfig,
) -> Result<NormalizationReport, String> {
    validate_config(config)?;
    let response: RawResponse = serde_json::from_str(raw_json)
        .map_err(|error| format!("Could not parse adsb.aero flight response: {error}"))?;
    let mut report = NormalizationReport {
        schema_version: 1,
        config: config.clone(),
        coordinate_frame: "WGS84 ECEF metres with explicit approximate altitude treatment".into(),
        timestamp_definition: "Unix UTC seconds minus config.time_origin_utc_s (one common origin for every pass)".into(),
        quality_notes: vec![
            "Source paths are simplified geometry (documented 50 m cross-track / 100 ft altitude tolerance), not full-resolution ADS-B records.".into(),
            "Source MultiLineString boundaries are never bridged. Additional splits use simplified-vertex time gaps, which do not necessarily mean missing raw coverage.".into(),
            "ECEF heights use the declared approximate altitude treatment; pressure, MSL and ellipsoid heights are not inherently equivalent. Carry uncertainty through a sensitivity analysis.".into(),
            "Derived segment velocities reflect piecewise-linear simplified geometry. Recover detailed tracks before claiming precise acoustic geolocation.".into(),
            "These chunks preserve data continuity but may still contain multiple physical passes; select the intended pass before using a global CPA.".into(),
        ],
        stats: NormalizationStats { source_flights: response.flights.len(), ..Default::default() },
        issues: Vec::new(),
        passes: Vec::new(),
    };
    for flight in response.flights {
        let fail_flight = |reason: &str, report: &mut NormalizationReport| {
            report.issues.push(NormalizationIssue {
                source_flight_id: flight.flight_id.clone(),
                source_subsequence: None,
                reason: reason.into(),
            });
        };
        if flight.flight_id.trim().is_empty() || flight.icao24.trim().is_empty() {
            fail_flight("Missing source flight ID or ICAO24 identity", &mut report);
            continue;
        }
        let (Some(path), Some(times)) = (&flight.path, &flight.timestamps) else {
            fail_flight(
                "Missing path or timestamps (request include_path=true)",
                &mut report,
            );
            continue;
        };
        if path.geometry_type != "MultiLineString" || path.coordinates.len() != times.len() {
            fail_flight(
                "Expected MultiLineString with matching timestamp sub-sequences",
                &mut report,
            );
            continue;
        }
        for (subsequence, (coordinates, times)) in path.coordinates.iter().zip(times).enumerate() {
            report.stats.source_subsequences += 1;
            report.stats.source_vertices += coordinates.len();
            let correction = flight
                .alt_correction_ft
                .as_ref()
                .and_then(|series| series.get(subsequence))
                .map(Vec::as_slice);
            let (points, removed) =
                match convert_subsequence(coordinates, times, correction, config) {
                    Ok(result) => result,
                    Err(reason) => {
                        report.stats.rejected_subsequences += 1;
                        report.issues.push(NormalizationIssue {
                            source_flight_id: flight.flight_id.clone(),
                            source_subsequence: Some(subsequence),
                            reason,
                        });
                        continue;
                    }
                };
            report.stats.duplicate_vertices_removed += removed;
            let mut chunk: Vec<TrackPoint> = Vec::new();
            let mut split = 0;
            for point in points {
                let should_split = if let Some(previous) = chunk.last() {
                    let dt = point.t_s - previous.t_s;
                    let distance = (point.xyz_m[0] - previous.xyz_m[0])
                        .hypot(point.xyz_m[1] - previous.xyz_m[1])
                        .hypot(point.xyz_m[2] - previous.xyz_m[2]);
                    let gap = dt > config.max_gap_s;
                    let speed = distance / dt > config.max_speed_mps;
                    if gap {
                        report.stats.vertex_gap_splits += 1;
                    }
                    if speed {
                        report.stats.speed_jump_splits += 1;
                    }
                    gap || speed
                } else {
                    false
                };
                if should_split {
                    finish_chunk(&mut chunk, &flight, subsequence, split, &mut report);
                    split += 1;
                }
                chunk.push(point);
            }
            finish_chunk(&mut chunk, &flight, subsequence, split, &mut report);
        }
    }
    report.stats.output_passes = report.passes.len();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> NormalizationConfig {
        NormalizationConfig {
            source_label: "synthetic test fixture".into(),
            time_origin_utc_s: 1_700_000_000.0,
            max_gap_s: 30.0,
            max_speed_mps: 300.0,
            altitude_treatment: AltitudeTreatment::PressureApproximation {
                ellipsoid_offset_m: 0.0,
                uncertainty_m: 150.0,
                rationale: "Explicit approximation for this unit-conversion test".into(),
            },
        }
    }

    fn response(coordinates: serde_json::Value, times: serde_json::Value) -> serde_json::Value {
        json!({"flights":[{"flight_id":"test-flight", "icao24":"abc123",
            "path":{"type":"MultiLineString", "coordinates":coordinates},
            "timestamps":times}]})
    }

    #[test]
    fn converts_feet_lon_lat_order_and_common_utc_seconds() {
        let raw = response(
            json!([[[0.0, 0.0, 10000.0], [0.001, 0.0, 10000.0]]]),
            json!([[1700000000.25, 1700000010.25]]),
        );
        let report = normalize_adsb_aero(&raw.to_string(), &config()).unwrap();
        assert_eq!(report.passes.len(), 1);
        let points = &report.passes[0].track.points;
        assert_eq!(points[0].t_s, 0.25);
        assert_eq!(points[1].t_s, 10.25);
        assert!((points[0].xyz_m[0] - (6_378_137.0 + 3048.0)).abs() < 1e-8);
        assert!(points[1].xyz_m[1] > 100.0);
        assert_eq!(points[1].xyz_m[2], 0.0);
        assert_eq!(report.passes[0].source_flight_id, "test-flight");
    }

    #[test]
    fn preserves_source_breaks_and_splits_large_vertex_gaps() {
        let raw = response(
            json!([
                [
                    [0.0, 0.0, 1000.0],
                    [0.001, 0.0, 1000.0],
                    [0.002, 0.0, 1000.0],
                    [0.003, 0.0, 1000.0]
                ],
                [[0.004, 0.0, 1000.0], [0.005, 0.0, 1000.0]]
            ]),
            json!([
                [1700000000.0, 1700000010.0, 1700000100.0, 1700000110.0],
                [1700000120.0, 1700000130.0]
            ]),
        );
        let report = normalize_adsb_aero(&raw.to_string(), &config()).unwrap();
        assert_eq!(report.passes.len(), 3);
        assert_eq!(report.stats.vertex_gap_splits, 1);
        assert_eq!(report.passes[2].source_subsequence, 1);
        assert!(report.passes.iter().all(|p| p.track.points.len() == 2));
    }

    #[test]
    fn qnh_correction_is_stepwise_and_geoid_sign_is_explicit() {
        let mut raw = response(
            json!([[
                [0.0, 0.0, 1000.0],
                [0.001, 0.0, 1000.0],
                [0.002, 0.0, 1000.0]
            ]]),
            json!([[1700000000.0, 1700000010.0, 1700000020.0]]),
        );
        raw["flights"][0]["alt_correction_ft"] =
            json!([[[1700000000.0, 100.0], [1700000015.0, 200.0]]]);
        let mut config = config();
        config.altitude_treatment = AltitudeTreatment::QnhMslWithGeoidOffset {
            geoid_undulation_m: -30.0,
            uncertainty_m: 100.0,
            rationale: "Local test geoid offset".into(),
        };
        let report = normalize_adsb_aero(&raw.to_string(), &config).unwrap();
        let heights: Vec<f64> = report.passes[0]
            .track
            .points
            .iter()
            .map(|p| p.xyz_m[0].hypot(p.xyz_m[1]) - 6_378_137.0)
            .collect();
        assert!((heights[0] - (1100.0 * 0.3048 - 30.0)).abs() < 1e-8);
        assert!((heights[1] - heights[0]).abs() < 1e-8);
        assert!((heights[2] - (1200.0 * 0.3048 - 30.0)).abs() < 1e-8);
        raw["flights"][0]["alt_correction_ft"] = serde_json::Value::Null;
        let missing = normalize_adsb_aero(&raw.to_string(), &config).unwrap();
        assert!(missing.passes.is_empty());
        assert_eq!(missing.stats.rejected_subsequences, 1);
    }

    #[test]
    fn refuses_milliseconds_conflicting_duplicates_and_implicit_altitude() {
        let raw = response(
            json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
            json!([[1700000000000.0, 1700000010000.0]]),
        );
        assert!(
            normalize_adsb_aero(&raw.to_string(), &config())
                .unwrap()
                .passes
                .is_empty()
        );
        let raw = response(
            json!([[[0.0, 0.0, 1000.0], [0.001, 0.0, 1000.0]]]),
            json!([[1700000000.0, 1700000000.0]]),
        );
        assert!(
            normalize_adsb_aero(&raw.to_string(), &config())
                .unwrap()
                .passes
                .is_empty()
        );
        let mut invalid = config();
        invalid.altitude_treatment = AltitudeTreatment::PressureApproximation {
            ellipsoid_offset_m: 0.0,
            uncertainty_m: 0.0,
            rationale: String::new(),
        };
        assert!(normalize_adsb_aero(&raw.to_string(), &invalid).is_err());
    }
}
