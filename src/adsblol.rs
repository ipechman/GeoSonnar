//! Native import of the documented readsb per-aircraft historical trace format.
//!
//! Format: https://github.com/wiedehopf/readsb/blob/dev/README-json.md#trace-jsons
//! Position time is top-level Unix seconds plus the row's relative seconds.
//! Geometric altitude is feet above the WGS84 ellipsoid, not orthometric or
//! pressure altitude. We never interpolate missing heights or carry sparse
//! aircraft-object fields forward. Original archive bytes belong in the
//! acquisition receipt; this module preserves selected point provenance.

use crate::physics::{Track, TrackPoint, geodetic_to_ecef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum TraceAltitudePolicy {
    #[default]
    GeometricOnly,
    /// Explicit engineering approximation, not a pressure-height correction.
    AllowBarometricApproximation {
        ellipsoid_offset_m: f64,
        uncertainty_m: f64,
        rationale: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceImportConfig {
    pub source_label: String,
    pub time_origin_utc_s: f64,
    pub time_window_utc_s: [f64; 2],
    /// south, west, north, east. West > east explicitly crosses the date line.
    /// A qualifying continuous pass selects its whole aircraft trace within
    /// the time window. Positions are not clipped at the horizontal boundary.
    pub bounds: Option<[f64; 4]>,
    pub max_gap_s: f64,
    pub max_speed_mps: f64,
    pub accepted_sources: Vec<String>,
    #[serde(default)]
    pub altitude_policy: TraceAltitudePolicy,
}

impl Default for TraceImportConfig {
    fn default() -> Self {
        Self {
            source_label: String::new(),
            time_origin_utc_s: 0.0,
            time_window_utc_s: [0.0, 86_400.0],
            bounds: None,
            max_gap_s: 30.0,
            max_speed_mps: 330.0,
            accepted_sources: vec!["adsb_icao".into(), "adsr_icao".into()],
            altitude_policy: TraceAltitudePolicy::GeometricOnly,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TracePointSource {
    pub row_index: usize,
    pub timestamp_utc_s: f64,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub ellipsoid_height_m: f64,
    pub altitude_basis: String,
    pub altitude_uncertainty_m: Option<f64>,
    pub altitude_main_ft: Option<f64>,
    pub altitude_geom_ft: Option<f64>,
    pub flags: u64,
    pub position_source: String,
    pub ground_speed_knots: Option<f64>,
    pub track_deg: Option<f64>,
    pub vertical_rate_fpm: Option<f64>,
    pub geometric_vertical_rate_fpm: Option<f64>,
    pub indicated_airspeed_knots: Option<f64>,
    pub roll_deg: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TracePass {
    pub track: Track,
    pub icao24: String,
    pub source_label: String,
    pub source_timestamp_utc_s: f64,
    pub source_points: Vec<TracePointSource>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceSource {
    pub icao24: String,
    pub source_timestamp_utc_s: f64,
    pub registration: Option<String>,
    pub aircraft_type: Option<String>,
    pub readsb_version: Option<String>,
    pub selected_by_region: bool,
    pub source_rows: usize,
    pub position_source_counts: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TraceImportStats {
    pub source_traces: usize,
    pub source_rows: usize,
    pub selected_traces: usize,
    pub outside_time_window: usize,
    pub exact_duplicates_removed: usize,
    /// Distinct timestamps whose source rows disagree in any field.
    #[serde(default)]
    pub conflicting_timestamp_groups: usize,
    /// Every row at a conflicting timestamp is excluded, including its first row.
    #[serde(default)]
    pub rejected_conflicting_timestamp_rows: usize,
    /// Adjacent backward steps in the unmodified source order.
    #[serde(default)]
    pub time_reversal_boundaries: usize,
    /// Rows in a reversed time overlap, excluding rows already counted as conflicts.
    #[serde(default)]
    pub rejected_time_reversal_rows: usize,
    pub rejected_ground: usize,
    pub rejected_stale_position: usize,
    pub rejected_position_source: usize,
    pub rejected_geometry: usize,
    pub rejected_altitude: usize,
    pub geometric_slot_points: usize,
    pub geometric_flag_points: usize,
    pub barometric_approximation_points: usize,
    pub leg_splits: usize,
    pub gap_splits: usize,
    pub speed_jump_splits: usize,
    pub altitude_basis_splits: usize,
    pub singleton_chunks_discarded: usize,
    pub output_passes: usize,
    pub output_points: usize,
}

impl TraceImportStats {
    pub(crate) fn add(&mut self, other: &Self) {
        macro_rules! add {
            ($($field:ident),+ $(,)?) => { $(self.$field += other.$field;)+ };
        }
        add!(
            source_traces,
            source_rows,
            selected_traces,
            outside_time_window,
            exact_duplicates_removed,
            conflicting_timestamp_groups,
            rejected_conflicting_timestamp_rows,
            time_reversal_boundaries,
            rejected_time_reversal_rows,
            rejected_ground,
            rejected_stale_position,
            rejected_position_source,
            rejected_geometry,
            rejected_altitude,
            geometric_slot_points,
            geometric_flag_points,
            barometric_approximation_points,
            leg_splits,
            gap_splits,
            speed_jump_splits,
            altitude_basis_splits,
            singleton_chunks_discarded,
            output_passes,
            output_points
        );
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TraceImportReport {
    pub schema_version: u32,
    pub provider: String,
    pub config: TraceImportConfig,
    pub coordinate_frame: String,
    pub timestamp_definition: String,
    pub location_verified: bool,
    pub quality_notes: Vec<String>,
    pub stats: TraceImportStats,
    pub sources: Vec<TraceSource>,
    pub passes: Vec<TracePass>,
}

pub fn validate_config(config: &TraceImportConfig) -> Result<(), String> {
    let [start, end] = config.time_window_utc_s;
    if config.source_label.trim().is_empty()
        || !config.time_origin_utc_s.is_finite()
        || !(0.0..10_000_000_000.0).contains(&config.time_origin_utc_s)
        || !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || start >= end
        || end >= 10_000_000_000.0
        || end - start > 7.0 * 86_400.0
    {
        return Err("Trace import needs a source label and a bounded UTC-second window of at most seven days; timestamps are not milliseconds".into());
    }
    if !config.max_gap_s.is_finite()
        || config.max_gap_s <= 0.0
        || !config.max_speed_mps.is_finite()
        || config.max_speed_mps <= 0.0
    {
        return Err("Trace gap and speed limits must be finite positive values".into());
    }
    if let Some([south, west, north, east]) = config.bounds
        && (![south, west, north, east].iter().all(|x| x.is_finite())
            || south < -90.0
            || north > 90.0
            || south >= north
            || !(-180.0..=180.0).contains(&west)
            || !(-180.0..=180.0).contains(&east)
            || west == east)
    {
        return Err("Invalid south,west,north,east bounds; west > east explicitly selects a date-line crossing".into());
    }
    let sources: BTreeSet<_> = config.accepted_sources.iter().collect();
    if sources.is_empty()
        || sources.len() != config.accepted_sources.len()
        || sources.iter().any(|s| !known_source(s))
    {
        return Err("Provide unique documented readsb position sources; unknown sources cannot be implicitly accepted".into());
    }
    if let TraceAltitudePolicy::AllowBarometricApproximation {
        ellipsoid_offset_m,
        uncertainty_m,
        rationale,
    } = &config.altitude_policy
        && (!ellipsoid_offset_m.is_finite()
            || !uncertainty_m.is_finite()
            || *uncertainty_m <= 0.0
            || rationale.trim().is_empty())
    {
        return Err("Barometric approximation requires an explicit finite offset, positive uncertainty and rationale".into());
    }
    Ok(())
}

fn known_source(source: &str) -> bool {
    matches!(
        source,
        "adsb_icao"
            | "adsb_icao_nt"
            | "adsr_icao"
            | "tisb_icao"
            | "adsc"
            | "mlat"
            | "other"
            | "mode_s"
            | "adsb_other"
            | "adsr_other"
            | "tisb_other"
            | "tisb_trackfile"
    )
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn blank_report(config: &TraceImportConfig) -> TraceImportReport {
    TraceImportReport {
        schema_version: 1,
        provider: "ADSB.lol/readsb historical traces".into(),
        config: config.clone(),
        coordinate_frame: "WGS84 ECEF metres; geometric aircraft heights reference the WGS84 ellipsoid".into(),
        timestamp_definition: "top-level timestamp + row-relative seconds - one common UTC origin".into(),
        location_verified: false,
        quality_notes: vec![
            "ADSB.lol receiver coverage is not complete aircraft coverage; import success does not establish completeness or location accuracy.".into(),
            "Only per-point geometric altitude slot 10 or main altitude with flag 8 is geometric. Sparse aircraft-object altitude is not copied; missing heights are not interpolated.".into(),
            "Rejected rows, new-leg flags, time gaps, altitude-basis changes and implausible motion break tracks. Single-point pieces are excluded.".into(),
            "Conflicting timestamps exclude all rows at that time, not an arbitrarily selected position. Reversed time overlaps exclude every timestamp with an earlier greater or later smaller timestamp, including all repeated endpoint rows. Neither source rows nor overlapping branches are sorted or combined; exclusions break support.".into(),
            "Region selection uses retained measured positions and straight longitude-unwrapped geographic segments inside continuous passes; qualifying aircraft retain every valid pass in the requested time window, including positions outside the box.".into(),
            "Regional selection is a bounded candidate filter, not proof of complete geographic coverage. Date-line bounds are supported in this importer; downstream observer grids may require separate boxes.".into(),
            "Source ground speed, direction and rates are preserved as metadata; the matcher derives motion from positions without inventing measured derivatives.".into(),
            "Geometric slot availability and raw-source quality are not calibrated positional or altitude confidence bounds.".into(),
        ],
        stats: TraceImportStats::default(), sources: Vec::new(), passes: Vec::new(),
    }
}

fn finish_chunk(
    report: &mut TraceImportReport,
    chunk: &mut Vec<TracePointSource>,
    icao: &str,
    anchor: f64,
) -> Result<(), String> {
    if chunk.len() < 2 {
        report.stats.singleton_chunks_discarded += usize::from(!chunk.is_empty());
        chunk.clear();
        return Ok(());
    }
    let points = chunk
        .iter()
        .map(|p| {
            Ok(TrackPoint {
                t_s: p.timestamp_utc_s - report.config.time_origin_utc_s,
                xyz_m: geodetic_to_ecef(p.latitude_deg, p.longitude_deg, p.ellipsoid_height_m)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let index = report.passes.len();
    report.passes.push(TracePass {
        track: Track {
            flight_id: format!("{icao}#part{index}"),
            points,
        },
        icao24: icao.into(),
        source_label: report.config.source_label.clone(),
        source_timestamp_utc_s: anchor,
        source_points: std::mem::take(chunk),
    });
    Ok(())
}

fn valid_height(height: f64) -> bool {
    height.is_finite() && (-1_000.0..=100_000.0).contains(&height)
}

/// Validate time/schema before emitting any positions, so a conflict discovered
/// later can also exclude its earlier occurrence. Time-order ambiguity is the
/// entire overlap of reversed runs, not just the row making a backward step.
/// The two scans preserve source order and exclude both branches of an inversion.
struct TraceTimes {
    timestamps: Vec<f64>,
    conflicts: Vec<bool>,
    reversed: Vec<bool>,
}

/// Shared structural validation for acquisition and native import. Rejected
/// source/altitude/geometry observations remain data-quality decisions; malformed
/// arrays, relative times and flags make the document non-importable.
pub(crate) fn validate_trace_rows(rows: &[Value], anchor: f64) -> Result<(), String> {
    if !(0.0..10_000_000_000.0).contains(&anchor) || rows.len() > 1_000_000 {
        return Err("Trace anchor or row count exceeds native import limits".into());
    }
    for (index, value) in rows.iter().enumerate() {
        inspect_row_schema(value, index, anchor)?;
    }
    Ok(())
}

fn inspect_row_schema(value: &Value, row_index: usize, anchor: f64) -> Result<f64, String> {
    let row = value.as_array().filter(|r| r.len() >= 9).ok_or_else(|| {
        format!("Trace row {row_index} does not match the documented positional array")
    })?;
    let offset = number(row.first())
        .ok_or_else(|| format!("Trace row {row_index} has invalid relative seconds"))?;
    let timestamp = anchor + offset;
    if !(0.0..=7.0 * 86_400.0).contains(&offset)
        || !timestamp.is_finite()
        || timestamp >= 10_000_000_000.0
    {
        return Err(format!(
            "Trace row {row_index} has invalid relative seconds"
        ));
    }
    row[6]
        .as_u64()
        .filter(|f| *f <= u32::MAX as u64)
        .ok_or_else(|| format!("Trace row {row_index} has invalid flags"))?;
    Ok(timestamp)
}

fn inspect_times(rows: &[Value], anchor: f64) -> Result<TraceTimes, String> {
    let mut timestamps = Vec::with_capacity(rows.len());
    let mut groups = BTreeMap::<u64, (usize, bool)>::new();
    for (row_index, value) in rows.iter().enumerate() {
        let timestamp = inspect_row_schema(value, row_index, anchor)?;
        // Finite nonnegative timestamps have an unambiguous bit key; canonicalise zero.
        let key = if timestamp == 0.0 {
            0
        } else {
            timestamp.to_bits()
        };
        match groups.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert((row_index, false));
            }
            std::collections::btree_map::Entry::Occupied(entry) => {
                let group = entry.into_mut();
                group.1 |= rows[group.0] != *value;
            }
        }
        timestamps.push(timestamp);
    }
    let conflicts = timestamps
        .iter()
        .map(|t| groups[&if *t == 0.0 { 0 } else { t.to_bits() }].1)
        .collect();
    let mut reversed = vec![false; timestamps.len()];
    let mut high_water = f64::NEG_INFINITY;
    for (i, &time) in timestamps.iter().enumerate() {
        reversed[i] = time < high_water;
        high_water = high_water.max(time);
    }
    let mut low_water = f64::INFINITY;
    for (i, &time) in timestamps.iter().enumerate().rev() {
        reversed[i] |= time > low_water;
        low_water = low_water.min(time);
    }
    // A repeated boundary timestamp belongs to the same ambiguous interval,
    // even when that particular copy is not itself part of a strict inversion.
    let reversed_times = timestamps
        .iter()
        .zip(&reversed)
        .filter(|(_, bad)| **bad)
        .map(|(time, _)| time.to_bits())
        .collect::<BTreeSet<_>>();
    for (time, bad) in timestamps.iter().zip(&mut reversed) {
        *bad |= reversed_times.contains(&time.to_bits());
    }
    Ok(TraceTimes {
        timestamps,
        conflicts,
        reversed,
    })
}

/// Normalize one decoded JSON document. Decompression, raw-file checksums,
/// archive enumeration and network completeness belong to acquisition.
pub fn normalize_trace(
    input: &str,
    config: &TraceImportConfig,
) -> Result<TraceImportReport, String> {
    validate_config(config)?;
    let raw: Value =
        serde_json::from_str(input).map_err(|e| format!("Invalid readsb JSON: {e}"))?;
    let icao = raw["icao"]
        .as_str()
        .ok_or("Trace has no aircraft identity")?;
    let hex = icao.strip_prefix('~').unwrap_or(icao);
    if hex.len() != 6
        || !hex.bytes().all(|c| c.is_ascii_hexdigit())
        || icao != icao.to_ascii_lowercase()
    {
        return Err("Trace ICAO identity must be six lowercase hexadecimal digits, optionally prefixed by ~".into());
    }
    let anchor = number(raw.get("timestamp")).ok_or("Trace has no finite UTC timestamp")?;
    if !(0.0..10_000_000_000.0).contains(&anchor) {
        return Err("Trace timestamp must be Unix seconds, not milliseconds".into());
    }
    let rows = raw["trace"].as_array().ok_or("Trace needs a row array")?;
    if rows.len() > 1_000_000 {
        return Err("One aircraft trace exceeds the one-million-row processing budget".into());
    }
    let mut report = blank_report(config);
    report.stats.source_traces = 1;
    report.stats.source_rows = rows.len();
    let TraceTimes {
        timestamps,
        conflicts,
        reversed,
    } = inspect_times(rows, anchor)?;
    report.stats.conflicting_timestamp_groups = timestamps
        .iter()
        .zip(&conflicts)
        .filter(|(_, conflict)| **conflict)
        .map(|(time, _)| time.to_bits())
        .collect::<BTreeSet<_>>()
        .len();
    report.stats.time_reversal_boundaries = timestamps
        .windows(2)
        .filter(|pair| pair[1] < pair[0])
        .count();
    let mut chunk = Vec::new();
    let mut previous_time = None;
    let mut previous_raw: Option<&Value> = None;
    let mut source_counts = BTreeMap::<String, usize>::new();
    for (row_index, row_value) in rows.iter().enumerate() {
        // Schema and absolute timestamps were checked before emitting any pass.
        let row = row_value.as_array().unwrap();
        let timestamp = timestamps[row_index];
        let flags = row[6]
            .as_u64()
            .filter(|f| *f <= u32::MAX as u64)
            .ok_or_else(|| format!("Trace row {row_index} has invalid flags"))?;
        let source = row
            .get(9)
            .and_then(Value::as_str)
            .or_else(|| row[8].get("type").and_then(Value::as_str))
            .unwrap_or("unknown");
        if let Some(count) = source_counts.get_mut(source) {
            *count += 1;
        } else {
            source_counts.insert(source.to_owned(), 1);
        }
        if conflicts[row_index] || reversed[row_index] {
            if conflicts[row_index] {
                report.stats.rejected_conflicting_timestamp_rows += 1;
            } else {
                report.stats.rejected_time_reversal_rows += 1;
            }
            finish_chunk(&mut report, &mut chunk, icao, anchor)?;
            previous_time = None;
            previous_raw = None;
            continue;
        }
        if previous_time == Some(timestamp) && previous_raw == Some(row_value) {
            report.stats.exact_duplicates_removed += 1;
            continue;
        }
        previous_time = Some(timestamp);
        previous_raw = Some(row_value);
        let mut reject = false;
        if timestamp < config.time_window_utc_s[0] || timestamp > config.time_window_utc_s[1] {
            report.stats.outside_time_window += 1;
            reject = true;
        } else if row[3].as_str() == Some("ground") {
            report.stats.rejected_ground += 1;
            reject = true;
        } else if flags & 1 != 0 {
            report.stats.rejected_stale_position += 1;
            reject = true;
        } else if !config.accepted_sources.iter().any(|s| s == source) {
            report.stats.rejected_position_source += 1;
            reject = true;
        }
        if reject {
            finish_chunk(&mut report, &mut chunk, icao, anchor)?;
            continue;
        }
        let latitude = number(row.get(1));
        let longitude = number(row.get(2));
        let Some((latitude, longitude)) = latitude
            .zip(longitude)
            .filter(|(lat, lon)| (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon))
        else {
            report.stats.rejected_geometry += 1;
            finish_chunk(&mut report, &mut chunk, icao, anchor)?;
            continue;
        };
        let main_ft = number(row.get(3));
        let geometric_ft = number(row.get(10));
        let altitude = if let Some(feet) = geometric_ft {
            Some((feet * 0.3048, "geometric_slot", None))
        } else if flags & 8 != 0 {
            main_ft.map(|feet| (feet * 0.3048, "geometric_flag", None))
        } else if let TraceAltitudePolicy::AllowBarometricApproximation {
            ellipsoid_offset_m,
            uncertainty_m,
            ..
        } = &config.altitude_policy
        {
            main_ft.map(|feet| {
                (
                    feet * 0.3048 + ellipsoid_offset_m,
                    "barometric_approximation",
                    Some(*uncertainty_m),
                )
            })
        } else {
            None
        };
        let Some((height, basis, uncertainty)) = altitude.filter(|(h, _, _)| valid_height(*h))
        else {
            report.stats.rejected_altitude += 1;
            finish_chunk(&mut report, &mut chunk, icao, anchor)?;
            continue;
        };
        match basis {
            "geometric_slot" => report.stats.geometric_slot_points += 1,
            "geometric_flag" => report.stats.geometric_flag_points += 1,
            _ => report.stats.barometric_approximation_points += 1,
        }
        let point = TracePointSource {
            row_index,
            timestamp_utc_s: timestamp,
            latitude_deg: latitude,
            longitude_deg: longitude,
            ellipsoid_height_m: height,
            altitude_basis: basis.into(),
            altitude_uncertainty_m: uncertainty,
            altitude_main_ft: main_ft,
            altitude_geom_ft: geometric_ft,
            flags,
            position_source: source.into(),
            ground_speed_knots: number(row.get(4)),
            track_deg: number(row.get(5)),
            vertical_rate_fpm: number(row.get(7)),
            geometric_vertical_rate_fpm: number(row.get(11)),
            indicated_airspeed_knots: number(row.get(12)),
            roll_deg: number(row.get(13)),
        };
        let mut split = false;
        if let Some(previous) = chunk.last() {
            if flags & 2 != 0 {
                report.stats.leg_splits += 1;
                split = true;
            } else if timestamp - previous.timestamp_utc_s > config.max_gap_s {
                report.stats.gap_splits += 1;
                split = true;
            } else if (basis == "barometric_approximation")
                != (previous.altitude_basis == "barometric_approximation")
            {
                report.stats.altitude_basis_splits += 1;
                split = true;
            } else {
                let one = geodetic_to_ecef(
                    previous.latitude_deg,
                    previous.longitude_deg,
                    previous.ellipsoid_height_m,
                )?;
                let two = geodetic_to_ecef(latitude, longitude, height)?;
                let distance = (one[0] - two[0])
                    .hypot(one[1] - two[1])
                    .hypot(one[2] - two[2]);
                if distance / (timestamp - previous.timestamp_utc_s) > config.max_speed_mps {
                    report.stats.speed_jump_splits += 1;
                    split = true;
                }
            }
        }
        if split {
            finish_chunk(&mut report, &mut chunk, icao, anchor)?;
        }
        chunk.push(point);
    }
    finish_chunk(&mut report, &mut chunk, icao, anchor)?;
    let selected = config.bounds.is_none_or(|bounds| {
        report.passes.iter().any(|pass| {
            pass.source_points
                .iter()
                .any(|p| point_in_bounds(p.latitude_deg, p.longitude_deg, bounds))
                || pass.source_points.windows(2).any(|p| {
                    segment_intersects_bounds(
                        [p[0].latitude_deg, p[0].longitude_deg],
                        [p[1].latitude_deg, p[1].longitude_deg],
                        bounds,
                    )
                })
        })
    });
    if !selected {
        report.passes.clear();
    }
    report.stats.selected_traces = usize::from(selected);
    report.stats.output_passes = report.passes.len();
    report.stats.output_points = report.passes.iter().map(|p| p.track.points.len()).sum();
    report.sources.push(TraceSource {
        icao24: icao.into(),
        source_timestamp_utc_s: anchor,
        registration: raw["r"].as_str().map(str::to_string),
        aircraft_type: raw["t"].as_str().map(str::to_string),
        readsb_version: raw["version"].as_str().map(str::to_string),
        selected_by_region: selected,
        source_rows: rows.len(),
        position_source_counts: source_counts,
    });
    Ok(report)
}

pub fn point_in_bounds(latitude: f64, longitude: f64, bounds: [f64; 4]) -> bool {
    let [south, west, north, east] = bounds;
    latitude >= south
        && latitude <= north
        && if west <= east {
            longitude >= west && longitude <= east
        } else {
            longitude >= west || longitude <= east
        }
}

/// Linear geographic segment intersection, explicitly unwrapped at +/-180.
/// Used only for candidate selection, never to interpolate a scoring sample.
pub fn segment_intersects_bounds(one: [f64; 2], two: [f64; 2], bounds: [f64; 4]) -> bool {
    if !one
        .iter()
        .chain(two.iter())
        .chain(bounds.iter())
        .all(|v| v.is_finite())
    {
        return false;
    }
    if point_in_bounds(one[0], one[1], bounds) || point_in_bounds(two[0], two[1], bounds) {
        return true;
    }
    let mut delta_lon = two[1] - one[1];
    if delta_lon > 180.0 {
        delta_lon -= 360.0;
    }
    if delta_lon < -180.0 {
        delta_lon += 360.0;
    }
    let [south, west, north, east] = bounds;
    let boxes = if west <= east {
        vec![(west, east)]
    } else {
        vec![(west, 180.0), (-180.0, east)]
    };
    for (left, right) in boxes {
        for shift in [-360.0, 0.0, 360.0] {
            let mut low: f64 = 0.0;
            let mut high: f64 = 1.0;
            let mut intersects = true;
            for (origin, direction, minimum, maximum) in [
                (one[0], two[0] - one[0], south, north),
                (one[1], delta_lon, left + shift, right + shift),
            ] {
                if direction == 0.0 {
                    if origin < minimum || origin > maximum {
                        intersects = false;
                        break;
                    }
                } else {
                    let a = (minimum - origin) / direction;
                    let b = (maximum - origin) / direction;
                    low = low.max(a.min(b));
                    high = high.min(a.max(b));
                    if low > high {
                        intersects = false;
                        break;
                    }
                }
            }
            if intersects {
                return true;
            }
        }
    }
    false
}

/// Merge one provider instance's per-aircraft reports. Multi-instance/date
/// reconciliation needs an explicit deduplication policy and is not implicit.
pub fn merge_reports(
    reports: Vec<TraceImportReport>,
    config: &TraceImportConfig,
) -> Result<TraceImportReport, String> {
    validate_config(config)?;
    let mut result = blank_report(config);
    let mut identities = BTreeSet::new();
    let mut track_ids = BTreeSet::new();
    for report in reports {
        if report.schema_version != 1 || report.config != *config || report.location_verified {
            return Err(
                "Cannot merge reports with incompatible settings or verification claims".into(),
            );
        }
        for source in &report.sources {
            if !identities.insert(source.icao24.clone()) {
                return Err(format!(
                    "Duplicate aircraft {} across trace documents; select one provider instance/day or reconcile explicitly",
                    source.icao24
                ));
            }
        }
        for pass in &report.passes {
            if !track_ids.insert(pass.track.flight_id.clone()) {
                return Err("Duplicate normalized pass identity".into());
            }
        }
        result.stats.add(&report.stats);
        result.sources.extend(report.sources);
        result.passes.extend(report.passes);
    }
    Ok(result)
}
