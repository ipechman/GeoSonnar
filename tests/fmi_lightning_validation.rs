//! Frozen provider rows plus adversarial XML and explicit request controls.
use geosonnar::fmi_lightning::{self as fmi, FmiRequest};
use serde_json::Value;
use std::collections::BTreeSet;

const XML: &str = include_str!("fixtures/fmi/nordlis-2025-07-19-minute.xml");
const EXPECTED: &str = include_str!("fixtures/fmi/nordlis-2025-07-19-expected.json");

fn request() -> FmiRequest {
    FmiRequest {
        start_utc: "2025-07-19T16:00:00Z".into(),
        end_utc: "2025-07-19T16:01:00Z".into(),
        bounds: [65., 13., 66., 15.],
        max_total_bytes: fmi::MAX_TOTAL_BYTES,
    }
}
fn parse(xml: &str) -> Result<fmi::FmiChunkData, String> {
    fmi::parse_chunk(&fmi::plan(&request()).unwrap(), 0, xml.as_bytes())
}
fn replace_list(xml: &str, tag: &str, text: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open).unwrap() + open.len();
    let end = start + xml[start..].find(&close).unwrap();
    format!("{}{text}{}", &xml[..start], &xml[end..])
}

#[test]
fn frozen_36_rows_match_independent_coordinates_and_all_144_parameter_values() {
    let value = parse(XML).unwrap();
    let expected: Value = serde_json::from_str(EXPECTED).unwrap();
    let rows = expected["rows"].as_array().unwrap();
    assert_eq!(value.raw_rows, 36);
    assert_eq!(value.excluded_boundary_rows, 0);
    assert_eq!(value.strikes.len(), 36);
    assert_eq!(
        value.body_sha256,
        "8a1e23ba96a79a2cd513145a8a7a8ba33f6a07dc723b98b52f426ae0d54b77c4"
    );
    assert_eq!(rows.len(), value.strikes.len());
    for (actual, expected) in value.strikes.iter().zip(rows) {
        assert_eq!(
            actual.latitude_deg,
            expected["latitude_etrs89"].as_f64().unwrap()
        );
        assert_eq!(
            actual.longitude_deg,
            expected["longitude_etrs89"].as_f64().unwrap()
        );
        assert_eq!(actual.time_utc_s, expected["unix_time_s"].as_f64().unwrap());
        assert_eq!(actual.altitude_m, None);
        assert_eq!(actual.intensity, None);
        let record = actual.source_record.as_ref().unwrap();
        assert_eq!(
            record.row_ordinal,
            expected["index"].as_u64().unwrap() as usize
        );
        assert_eq!(record.response_sha256, value.body_sha256);
        assert_eq!(
            record.multiplicity as f64,
            expected["multiplicity"].as_f64().unwrap()
        );
        assert_eq!(
            record.peak_current_ka,
            expected["peak_current"].as_f64().unwrap()
        );
        assert_eq!(
            record.cloud_indicator as f64,
            expected["cloud_indicator"].as_f64().unwrap()
        );
        assert_eq!(
            record.ellipse_major_raw,
            expected["ellipse_major"].as_f64().unwrap()
        );
    }
    assert_eq!(
        value
            .strikes
            .iter()
            .map(|s| s.id.clone())
            .collect::<BTreeSet<_>>()
            .len(),
        36
    );
    assert_eq!(
        value
            .strikes
            .iter()
            .map(|s| s.time_utc_s as u64)
            .collect::<BTreeSet<_>>()
            .len(),
        16
    );
    let document = fmi::document(&fmi::plan(&request()).unwrap(), &[value])
        .unwrap()
        .unwrap();
    let provenance = document.provenance.unwrap();
    assert_eq!(provenance.provider, "FMI/NORDLIS");
    assert!(provenance.license.unwrap().contains("CC BY 4.0"));
    assert_eq!(provenance.source_metadata.unwrap().time_resolution_s, 1.0);
}

#[test]
fn planner_fixes_urls_axis_order_limits_and_cross_midnight_chunks() {
    let mut req = request();
    req.start_utc = "2024-02-29T23:59:30Z".into();
    req.end_utc = "2024-03-01T00:01:00Z".into();
    let plan = fmi::plan(&req).unwrap();
    assert_eq!(plan.chunks.len(), 2);
    assert!(
        plan.chunks[0]
            .url
            .starts_with("https://opendata.fmi.fi/wfs?")
    );
    assert!(plan.chunks[0].url.contains("bbox=13,65,15,66"));
    assert!(
        plan.chunks[0]
            .url
            .contains("starttime=2024-02-29T23:59:30Z&endtime=2024-03-01T00:00:30Z")
    );
    assert_eq!(plan.chunks[0].end_utc_s, plan.chunks[1].start_utc_s);
    assert_eq!(plan.chunks[1].end_utc_s - plan.chunks[1].start_utc_s, 30);
    assert_eq!(plan.max_response_bytes, fmi::MAX_RESPONSE_BYTES);
    for time in [0, 86400, 951_782_400, 1_709_251_200, 7_258_118_400] {
        let text = fmi::format_utc(time).unwrap();
        assert_eq!(geosonnar::workspace::parse_utc(&text).unwrap(), time as f64);
    }
    assert!(fmi::format_utc(u64::MAX).is_err());
    let original = fmi::plan_sha256(&plan).unwrap();
    let mut forged = plan.clone();
    forged.chunks[0].url = "https://example.com/".into();
    assert!(fmi::validate_plan(&forged).is_err());
    assert!(fmi::plan_sha256(&forged).is_err());
    let mut forged = plan.clone();
    forged.max_response_bytes += 1;
    assert!(fmi::validate_plan(&forged).is_err());
    assert_eq!(fmi::plan_sha256(&plan).unwrap(), original);
}

#[test]
fn invalid_request_intervals_bounds_and_budgets_fail_before_parsing() {
    for invalid in [
        "2025-07-19 16:00:00",
        "2025-07-19T16:00:00",
        "2025-07-19T16:00:00+00:00",
        "2025-07-19T16:00:00.1Z",
        "2025-02-29T16:00:00Z",
        "1969-01-01T00:00:00Z",
        "2025-07-19T16:00:60Z",
    ] {
        let mut req = request();
        req.start_utc = invalid.into();
        assert!(fmi::plan(&req).is_err(), "{invalid}");
    }
    for bounds in [
        [66., 13., 65., 15.],
        [65., 15., 66., 13.],
        [-90., -180., 90., 180.],
        [65., 13., 96., 15.],
        [65., 13., 66., 74.],
        [f64::NAN, 13., 66., 15.],
    ] {
        let mut req = request();
        req.bounds = bounds;
        assert!(fmi::plan(&req).is_err());
    }
    for bytes in [0, fmi::MAX_TOTAL_BYTES + 1] {
        let mut req = request();
        req.max_total_bytes = bytes;
        assert!(fmi::plan(&req).is_err());
    }
    let mut req = request();
    req.end_utc = req.start_utc.clone();
    assert!(fmi::plan(&req).is_err());
    req.end_utc = "2025-07-19T17:00:01Z".into();
    assert!(fmi::plan(&req).is_err());
    req.end_utc = "2025-07-19T17:00:00Z".into();
    assert_eq!(fmi::plan(&req).unwrap().chunks.len(), 60);
}

#[test]
fn namespace_prefixes_and_declared_field_order_do_not_change_physical_records() {
    let original = parse(XML).unwrap();
    let renamed = XML
        .replace("wfs:", "a:")
        .replace("xmlns:wfs=", "xmlns:a=")
        .replace("gmlcov:", "c:")
        .replace("xmlns:gmlcov=", "xmlns:c=")
        .replace("gml:", "g:")
        .replace("xmlns:gml=", "xmlns:g=")
        .replace("swe:", "s:")
        .replace("xmlns:swe=", "xmlns:s=")
        .replace("xlink:", "x:")
        .replace("xmlns:xlink=", "xmlns:x=");
    let renamed = parse(&renamed).unwrap();
    assert_eq!(renamed.strikes.len(), original.strikes.len());
    for (a, b) in renamed.strikes.iter().zip(&original.strikes) {
        assert_eq!(
            (a.latitude_deg, a.longitude_deg, a.time_utc_s),
            (b.latitude_deg, b.longitude_deg, b.time_utc_s)
        );
    }
    let start = XML.find("<gml:doubleOrNilReasonTupleList>").unwrap()
        + "<gml:doubleOrNilReasonTupleList>".len();
    let end = start
        + XML[start..]
            .find("</gml:doubleOrNilReasonTupleList>")
            .unwrap();
    let swapped = XML[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .chunks_exact(4)
        .map(|r| format!("{} {} {} {}", r[1], r[0], r[2], r[3]))
        .collect::<Vec<_>>()
        .join("\n");
    let swapped = replace_list(XML, "gml:doubleOrNilReasonTupleList", &swapped)
        .replace("multiplicity", "swapfield")
        .replace("peak_current", "multiplicity")
        .replace("swapfield", "peak_current");
    let changed = parse(&swapped).unwrap();
    for (a, b) in changed.strikes.iter().zip(&original.strikes) {
        let a = a.source_record.as_ref().unwrap();
        let b = b.source_record.as_ref().unwrap();
        assert_eq!(a.multiplicity, b.multiplicity);
        assert_eq!(a.peak_current_ka, b.peak_current_ka);
        assert_eq!(a.cloud_indicator, b.cloud_indicator);
        assert_eq!(a.ellipse_major_raw, b.ellipse_major_raw);
    }
}

#[test]
fn coincident_rows_are_distinct_and_endpoint_rows_have_half_open_ownership() {
    let xml = replace_list(
        XML,
        "gmlcov:positions",
        "65.5 14 1752940800\n65.5 14 1752940800\n65.5 14 1752940860",
    );
    let xml = replace_list(
        &xml,
        "gml:doubleOrNilReasonTupleList",
        "0 -3 0 0.1\n0 -3 0 0.1\n1 2 1 0.2",
    );
    let parsed = parse(&xml).unwrap();
    assert_eq!(parsed.raw_rows, 3);
    assert_eq!(parsed.excluded_boundary_rows, 1);
    assert_eq!(parsed.strikes.len(), 2);
    assert_ne!(parsed.strikes[0].id, parsed.strikes[1].id);
    let mut req = request();
    req.end_utc = "2025-07-19T16:02:00Z".into();
    let plan = fmi::plan(&req).unwrap();
    let first = fmi::parse_chunk(&plan, 0, xml.as_bytes()).unwrap();
    let second = XML
        .replace("2025-07-19T16:01:00Z", "2025-07-19T16:02:00Z")
        .replace("2025-07-19T16:00:00Z", "2025-07-19T16:01:00Z");
    let second = replace_list(
        &second,
        "gmlcov:positions",
        "65.5 14 1752940860\n65.5 14 1752940920",
    );
    let second = replace_list(
        &second,
        "gml:doubleOrNilReasonTupleList",
        "1 2 1 0.2\n1 -4 0 0.1",
    );
    let second = fmi::parse_chunk(&plan, 1, second.as_bytes()).unwrap();
    let doc = fmi::document(&plan, &[second, first]).unwrap().unwrap();
    assert_eq!(doc.strikes.len(), 3);
    assert_eq!(
        doc.strikes
            .iter()
            .filter(|s| s.time_utc_s == 1752940860.0)
            .count(),
        1
    );
}

#[test]
fn empty_complete_responses_are_preserved_without_manufacturing_loadable_strikes() {
    let empty = "<f:FeatureCollection xmlns:f=\"http://www.opengis.net/wfs/2.0\" numberMatched=\"0\" numberReturned=\"0\"/>";
    let parsed = parse(empty).unwrap();
    assert_eq!(parsed.raw_rows, 0);
    assert!(
        fmi::document(&fmi::plan(&request()).unwrap(), &[parsed])
            .unwrap()
            .is_none()
    );
    let xml = replace_list(XML, "gmlcov:positions", "");
    let xml = replace_list(&xml, "gml:doubleOrNilReasonTupleList", "");
    assert_eq!(parse(&xml).unwrap().raw_rows, 0);
    assert!(parse(&empty.replace("/>", "><other/></f:FeatureCollection>")).is_err());
}

#[test]
fn incomplete_unknown_or_changed_xml_semantics_fail_closed() {
    for (old, new) in [
        ("numberMatched=\"1\"", "numberMatched=\"2\""),
        ("numberMatched=\"1\"", "numberMatched=\"unknown\""),
        ("numberReturned=\"1\"", "numberReturned=\"0\""),
        (
            "numberReturned=\"1\"",
            "numberReturned=\"1\" next=\"https://example.com\"",
        ),
        ("crs=4258&amp;time=unixtime", "crs=4326&amp;time=unixtime"),
        ("srsDimension=\"3\"", "srsDimension=\"2\""),
        (
            "srsDimension=\"3\"",
            "srsDimension=\"3\" axisLabels=\"longitude latitude time\"",
        ),
        ("name=\"ellipse_major\"", "name=\"new_field\""),
        ("name=\"ellipse_major\"", "name=\"cloud_indicator\""),
        (
            "param=ellipse_major&amp;language=eng",
            "param=unknown&amp;language=eng",
        ),
        (">Linear<", ">Reverse<"),
        ("1752940800", "1752940800.5"),
        ("1752940800", "1752940799"),
        ("65.72100", "64.72100"),
        ("3 -33 0 0.2", "3 -33 2 0.2"),
        ("3 -33 0 0.2", "3.5 -33 0 0.2"),
        ("3 -33 0 0.2", "-1 -33 0 0.2"),
        ("3 -33 0 0.2", "3 NaN 0 0.2"),
        ("3 -33 0 0.2", "3 -33 0 -0.2"),
        ("2025-07-19T16:00:00Z", "2025-07-19T15:59:00Z"),
    ] {
        assert!(
            parse(&XML.replacen(old, new, 1)).is_err(),
            "accepted {old} => {new}"
        );
    }
    assert!(
        parse(&replace_list(
            XML,
            "gml:doubleOrNilReasonTupleList",
            "1 -4 0 0.2"
        ))
        .is_err()
    );
    assert!(parse(&XML.replace("</wfs:FeatureCollection>", "")).is_err());
    assert!(parse("<ows:ExceptionReport xmlns:ows=\"http://www.opengis.net/ows/1.1\"/>").is_err());
    assert!(
        parse(&XML.replace(
            "</gmlcov:positions>",
            "<nested>0</nested></gmlcov:positions>"
        ))
        .is_err()
    );
}

#[test]
fn dangerous_encodings_entities_deep_markup_and_large_inputs_are_bounded() {
    for xml in [
        XML.replace("UTF-8", "UTF-16"),
        XML.replace("version=\"1.0\"", "version=\"1.1\""),
        XML.replace(
            "<wfs:FeatureCollection",
            "<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///ignored'>]><wfs:FeatureCollection",
        ),
        XML.replace("1752940800", "&entity;"),
        XML.replace("65.72100", "<![CDATA[65.72100]]>"),
        format!("{XML}{XML}"),
        format!("<?xml version=\"1.0\"?>{XML}"),
        XML.replace("<om:result>", &format!("<om:result>{}", "<a>".repeat(40))),
        XML.replace("65.72100", &"0".repeat(65)),
        XML.replace(
            "numberReturned=\"1\"",
            &format!("numberReturned=\"1\" extra=\"{}\"", "x".repeat(4097)),
        ),
    ] {
        assert!(parse(&xml).is_err());
    }
    let plan = fmi::plan(&request()).unwrap();
    assert!(fmi::parse_chunk(&plan, 0, &vec![b' '; fmi::MAX_RESPONSE_BYTES as usize + 1]).is_err());
    assert!(fmi::parse_chunk(&plan, 0, b"\xff").is_err());
    assert!(fmi::parse_chunk(&plan, 1, XML.as_bytes()).is_err());
}

#[test]
fn normalized_inventory_requires_complete_unique_bound_chunks_and_source_rows() {
    let plan = fmi::plan(&request()).unwrap();
    let original = parse(XML).unwrap();
    assert!(fmi::document(&plan, &[]).is_err());
    assert!(fmi::document(&plan, &[original.clone(), original.clone()]).is_err());
    for choice in 0..6 {
        let mut changed = original.clone();
        match choice {
            0 => changed.plan_sha256 = "0".repeat(64),
            1 => changed.strikes[0].source_record = None,
            2 => changed.strikes[0].time_utc_s = 1752940860.0,
            3 => {
                changed.strikes[0]
                    .source_record
                    .as_mut()
                    .unwrap()
                    .row_ordinal = 1
            }
            4 => changed.raw_rows += 1,
            _ => changed.strikes[0].altitude_m = Some(0.0),
        }
        assert!(fmi::document(&plan, &[changed]).is_err(), "tamper {choice}");
    }
    let before = serde_json::to_vec(&original).unwrap();
    let restored: fmi::FmiChunkData = serde_json::from_slice(&before).unwrap();
    let a = fmi::document(&plan, &[original]).unwrap().unwrap();
    let b = fmi::document(&plan, &[restored]).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(a).unwrap(),
        serde_json::to_value(b).unwrap()
    );
}
