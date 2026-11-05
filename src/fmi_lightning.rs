//! Bounded, explicit FMI/NORDLIS historical-lightning requests and XML import.
//! This module performs no network or filesystem operations. Provider records
//! retain their original ETRS89 coordinates and whole-second representation.
use crate::{
    evidence_score::{
        self, LightningAltitudeDatum, LightningDocument, LightningProvenance, LightningStrike,
    },
    lightning_source::{FmiRecord, FmiSourceMetadata},
    workspace,
};
use quick_xml::{XmlVersion, events::Event, name::ResolveResult, reader::NsReader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_STRIKES: usize = 250_000;
pub const MAX_SPAN_S: u64 = 3600;
pub const CHUNK_SPAN_S: u64 = 60;
pub const ENDPOINT: &str = "https://opendata.fmi.fi/wfs";
pub const LICENSE: &str =
    "CC BY 4.0 — FMI/NORDLIS; https://en.ilmatieteenlaitos.fi/open-data-licence";
const WFS: &str = "http://www.opengis.net/wfs/2.0";
const OM: &str = "http://www.opengis.net/om/2.0";
const OMSO: &str = "http://inspire.ec.europa.eu/schemas/omso/3.0";
const GML: &str = "http://www.opengis.net/gml/3.2";
const COV: &str = "http://www.opengis.net/gmlcov/1.0";
const SWE: &str = "http://www.opengis.net/swe/2.0";
const XLINK: &str = "http://www.w3.org/1999/xlink";
const COMPOUND_CRS: &str = "http://xml.fmi.fi/gml/crs/compoundCRS.php?crs=4258&time=unixtime";
const FIELDS: [&str; 4] = [
    "multiplicity",
    "peak_current",
    "cloud_indicator",
    "ellipse_major",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiRequest {
    pub start_utc: String,
    pub end_utc: String,
    /// South, west, north, east. The URL explicitly converts to FMI lon/lat order.
    pub bounds: [f64; 4],
    pub max_total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiChunk {
    pub index: usize,
    pub start_utc_s: u64,
    pub end_utc_s: u64,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiPlan {
    pub schema_version: u32,
    pub request: FmiRequest,
    pub start_utc_s: u64,
    pub end_utc_s: u64,
    pub chunks: Vec<FmiChunk>,
    pub max_response_bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmiChunkData {
    pub schema_version: u32,
    pub plan_sha256: String,
    pub chunk_index: usize,
    pub body_sha256: String,
    pub raw_rows: usize,
    /// Inclusive provider endpoint rows retained in raw bytes, but assigned only
    /// to the next chunk by represented-time ownership [start,end).
    pub excluded_boundary_rows: usize,
    pub strikes: Vec<LightningStrike>,
}

fn utc(value: &str) -> Result<u64, String> {
    if value.len() != 20
        || !value.is_ascii()
        || value.as_bytes()[10] != b'T'
        || !value.ends_with('Z')
    {
        return Err(
            "FMI requests require explicit UTC YYYY-MM-DDTHH:MM:SSZ, with whole seconds".into(),
        );
    }
    let time = workspace::parse_utc(value)?;
    if time < 0.0 || !time.is_finite() || time.fract() != 0.0 {
        return Err("FMI requests require nonnegative whole Unix UTC seconds".into());
    }
    Ok(time as u64)
}

/// Format a supported Unix second without introducing local-time interpretation.
pub fn format_utc(time: u64) -> Result<String, String> {
    let mut days = time / 86400;
    let leap = |year: u64| {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    };
    let mut year = 1970;
    while year <= 2200 {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    if year > 2200 {
        return Err("FMI UTC is outside supported calendar years 1970..2200".into());
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while days >= months[month] {
        days -= months[month];
        month += 1;
    }
    let seconds = time % 86400;
    Ok(format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        month + 1,
        days + 1,
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    ))
}

pub fn plan(request: &FmiRequest) -> Result<FmiPlan, String> {
    let start = utc(&request.start_utc)?;
    let end = utc(&request.end_utc)?;
    if end <= start || end - start > MAX_SPAN_S {
        return Err("Choose an increasing FMI UTC interval of at most one hour per request".into());
    }
    let [south, west, north, east] = request.bounds;
    if !request.bounds.iter().all(|v| v.is_finite())
        || !(-90.0..=90.0).contains(&south)
        || !(-90.0..=90.0).contains(&north)
        || !(-180.0..=180.0).contains(&west)
        || !(-180.0..=180.0).contains(&east)
        || south >= north
        || west >= east
        || north - south > 30.0
        || east - west > 60.0
    {
        return Err("FMI requests need a non-wrapping region at most 30 degrees latitude by 60 degrees longitude; these are application limits, not provider coverage".into());
    }
    if request.max_total_bytes == 0 || request.max_total_bytes > MAX_TOTAL_BYTES {
        return Err(
            "FMI total transfer allowance must be between 0.0000000009 GiB and 0.0625 GiB".into(),
        );
    }
    let mut chunks = Vec::new();
    let mut from = start;
    while from < end {
        let to = (from + CHUNK_SPAN_S).min(end);
        let url = format!(
            "{ENDPOINT}?service=WFS&version=2.0.0&request=getFeature&storedquery_id=fmi::observations::lightning::multipointcoverage&starttime={}&endtime={}&bbox={west},{south},{east},{north}",
            format_utc(from)?,
            format_utc(to)?
        );
        chunks.push(FmiChunk {
            index: chunks.len(),
            start_utc_s: from,
            end_utc_s: to,
            url,
        });
        from = to;
    }
    Ok(FmiPlan {
        schema_version: 1,
        request: request.clone(),
        start_utc_s: start,
        end_utc_s: end,
        chunks,
        max_response_bytes: MAX_RESPONSE_BYTES.min(request.max_total_bytes),
    })
}

/// Never trust URLs, endpoints, limits or chunk boundaries supplied in saved JSON.
pub fn validate_plan(value: &FmiPlan) -> Result<(), String> {
    if *value != plan(&value.request)? {
        return Err("FMI plan differs from its reconstructed explicit request".into());
    }
    Ok(())
}

pub fn plan_sha256(value: &FmiPlan) -> Result<String, String> {
    validate_plan(value)?;
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|e| e.to_string())?)
    ))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Name {
    namespace: String,
    local: String,
}
impl Name {
    fn is(&self, namespace: &str, local: &str) -> bool {
        self.namespace == namespace && self.local == local
    }
}
struct Node {
    name: Name,
    text: String,
}
fn namespace(result: ResolveResult<'_>) -> Result<String, String> {
    match result {
        ResolveResult::Bound(value) => std::str::from_utf8(value.as_ref())
            .map(str::to_owned)
            .map_err(|_| "Invalid XML namespace encoding".into()),
        ResolveResult::Unbound => Ok(String::new()),
        ResolveResult::Unknown(_) => Err("FMI XML contains an unbound namespace prefix".into()),
    }
}
fn path_is(stack: &[Node], expected: &[(&str, &str)]) -> bool {
    stack.len() == expected.len()
        && stack
            .iter()
            .zip(expected)
            .all(|(node, (ns, name))| node.name.is(ns, name))
}
fn coverage_path() -> Vec<(&'static str, &'static str)> {
    vec![
        (WFS, "FeatureCollection"),
        (WFS, "member"),
        (OMSO, "GridSeriesObservation"),
        (OM, "result"),
        (COV, "MultiPointCoverage"),
    ]
}
fn path_after(stack: &[Node], suffix: &[(&str, &str)]) -> bool {
    let mut path = coverage_path();
    path.extend_from_slice(suffix);
    path_is(stack, &path)
}
fn coverage_child_allowed(stack: &[Node], name: &Name) -> bool {
    let children: Option<Vec<(&str, &str)>> = if path_after(stack, &[]) {
        Some(vec![
            (GML, "domainSet"),
            (GML, "rangeSet"),
            (GML, "coverageFunction"),
            (COV, "rangeType"),
        ])
    } else if path_after(stack, &[(GML, "domainSet")]) {
        Some(vec![(COV, "SimpleMultiPoint")])
    } else if path_after(stack, &[(GML, "domainSet"), (COV, "SimpleMultiPoint")]) {
        Some(vec![(COV, "positions")])
    } else if path_after(stack, &[(GML, "rangeSet")]) {
        Some(vec![(GML, "DataBlock")])
    } else if path_after(stack, &[(GML, "rangeSet"), (GML, "DataBlock")]) {
        Some(vec![
            (GML, "rangeParameters"),
            (GML, "doubleOrNilReasonTupleList"),
        ])
    } else if path_after(stack, &[(COV, "rangeType")]) {
        Some(vec![(SWE, "DataRecord")])
    } else if path_after(stack, &[(COV, "rangeType"), (SWE, "DataRecord")]) {
        Some(vec![(SWE, "field")])
    } else if path_after(stack, &[(GML, "coverageFunction")]) {
        Some(vec![(GML, "CoverageMappingRule")])
    } else if path_after(
        stack,
        &[(GML, "coverageFunction"), (GML, "CoverageMappingRule")],
    ) {
        Some(vec![(GML, "ruleDefinition")])
    } else {
        None
    };
    children.is_none_or(|children| children.iter().any(|(ns, local)| name.is(ns, local)))
}
fn once(slot: &mut Option<String>, text: String, label: &str) -> Result<(), String> {
    if slot.replace(text).is_some() {
        return Err(format!("Duplicate FMI {label}"));
    }
    Ok(())
}
fn integer(value: &str, label: &str) -> Result<usize, String> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("FMI {label} needs an explicit nonnegative integer"));
    }
    value
        .parse()
        .map_err(|_| format!("FMI {label} exceeds its integer limit"))
}
fn numbers(text: &str, limit: usize) -> Result<Vec<f64>, String> {
    let mut values = Vec::new();
    for token in text.split_ascii_whitespace() {
        if values.len() == limit || token.len() > 64 {
            return Err("FMI numeric inventory or token exceeds its limit".into());
        }
        let value = token
            .parse::<f64>()
            .map_err(|_| "Invalid or unsupported nil FMI numeric value")?;
        if !value.is_finite() {
            return Err(
                "FMI records require finite numeric values; nil values are not replaced".into(),
            );
        }
        values.push(value);
    }
    Ok(values)
}

/// Parse the exact supported WFS multipoint structure. XML declarations are
/// local data only: no schema, xlink, DTD or external entity is ever fetched.
pub fn parse_chunk(value: &FmiPlan, index: usize, body: &[u8]) -> Result<FmiChunkData, String> {
    let identity = plan_sha256(value)?;
    let chunk = value
        .chunks
        .get(index)
        .ok_or("FMI chunk index is outside the request")?;
    if body.is_empty() || body.len() as u64 > value.max_response_bytes {
        return Err("FMI response is empty or exceeds its transfer allowance".into());
    }
    let xml = std::str::from_utf8(body).map_err(|_| "FMI XML must use UTF-8")?;
    if xml.chars().any(|c| {
        (c < ' ' && !matches!(c, '\t' | '\r' | '\n')) || matches!(c, '\u{fffe}' | '\u{ffff}')
    }) {
        return Err("FMI XML contains a forbidden XML character".into());
    }
    let mut reader = NsReader::from_str(xml.trim_start_matches('\u{feff}'));
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().check_end_names = true;
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Node> = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let mut matched = 0;
    let mut members = 0;
    let mut observations = 0;
    let mut coverages = 0;
    let mut simple_points = 0;
    let mut positions = None;
    let mut tuples = None;
    let mut begin = None;
    let mut end = None;
    let mut fields = Vec::new();
    let mut mapping = None;
    let mut containers = BTreeSet::new();
    let mut events = 0usize;
    loop {
        events += 1;
        if events > 50_000 {
            return Err("FMI XML event budget exceeded".into());
        }
        let (resolved, event) = reader
            .read_resolved_event()
            .map_err(|e| format!("Invalid FMI XML: {e}"))?;
        match event {
            Event::Start(element) => {
                let name = Name {
                    namespace: namespace(resolved)?,
                    local: std::str::from_utf8(element.local_name().as_ref())
                        .map_err(|_| "Invalid XML element name")?
                        .into(),
                };
                if element.len() > 16_384 || name.local.len() > 256 || stack.len() >= 32 {
                    return Err("FMI XML name, start tag or nesting limit exceeded".into());
                }
                if matches!(
                    name.local.as_str(),
                    "ExceptionReport" | "Exception" | "truncatedResponse"
                ) {
                    return Err("FMI returned an exception or truncated response".into());
                }
                if stack.last().is_some_and(|node| {
                    node.name.is(COV, "positions")
                        || node.name.is(GML, "doubleOrNilReasonTupleList")
                        || node.name.is(GML, "beginPosition")
                        || node.name.is(GML, "endPosition")
                        || node.name.is(GML, "ruleDefinition")
                        || node.name.is(GML, "rangeParameters")
                        || node.name.is(SWE, "field")
                }) {
                    return Err("FMI scalar/list fields may not contain child elements".into());
                }
                if (stack.len() == 1 && !name.is(WFS, "member"))
                    || (path_is(&stack, &[(WFS, "FeatureCollection"), (WFS, "member")])
                        && !name.is(OMSO, "GridSeriesObservation"))
                    || (path_is(
                        &stack,
                        &[
                            (WFS, "FeatureCollection"),
                            (WFS, "member"),
                            (OMSO, "GridSeriesObservation"),
                            (OM, "result"),
                        ],
                    ) && !name.is(COV, "MultiPointCoverage"))
                {
                    return Err(
                        "FMI response contains an unsupported feature or result shape".into(),
                    );
                }
                if !coverage_child_allowed(&stack, &name) {
                    return Err("FMI coverage contains an unsupported field or container".into());
                }
                let mut attributes = BTreeMap::new();
                for attribute in element.attributes() {
                    let attribute =
                        attribute.map_err(|e| format!("Invalid FMI XML attribute: {e}"))?;
                    if attribute.key.as_ref().len() > 256
                        || attribute.value.len() > 4096
                        || attributes.len() >= 64
                    {
                        return Err("FMI XML attribute limit exceeded".into());
                    }
                    let (ns, local) = reader.resolver().resolve_attribute(attribute.key);
                    let key = (
                        namespace(ns)?,
                        std::str::from_utf8(local.as_ref())
                            .map_err(|_| "Invalid XML attribute name")?
                            .to_owned(),
                    );
                    let decoded = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|e| format!("Invalid FMI XML attribute value: {e}"))?
                        .into_owned();
                    if attributes.insert(key, decoded).is_some() {
                        return Err("Duplicate expanded FMI XML attribute".into());
                    }
                }
                let attr = |ns: &str, key: &str| {
                    attributes
                        .get(&(ns.to_owned(), key.to_owned()))
                        .map(String::as_str)
                };
                if stack.is_empty() {
                    if root_seen || root_closed || !name.is(WFS, "FeatureCollection") {
                        return Err("FMI response needs one WFS2 FeatureCollection root".into());
                    }
                    root_seen = true;
                    matched = integer(
                        attr("", "numberMatched")
                            .ok_or("FMI response lacks complete feature counts")?,
                        "numberMatched",
                    )?;
                    let returned = integer(
                        attr("", "numberReturned")
                            .ok_or("FMI response lacks complete feature counts")?,
                        "numberReturned",
                    )?;
                    if matched != returned
                        || matched > 1
                        || attr("", "next").is_some_and(|v| !v.is_empty())
                    {
                        return Err("FMI response is incomplete, paginated or has unsupported multiple coverage features".into());
                    }
                }
                stack.push(Node {
                    name,
                    text: String::new(),
                });
                if path_is(&stack, &[(WFS, "FeatureCollection"), (WFS, "member")]) {
                    members += 1;
                }
                if path_is(
                    &stack,
                    &[
                        (WFS, "FeatureCollection"),
                        (WFS, "member"),
                        (OMSO, "GridSeriesObservation"),
                    ],
                ) {
                    observations += 1;
                }
                if path_after(&stack, &[]) {
                    coverages += 1;
                }
                for path in [
                    vec![(GML, "domainSet")],
                    vec![(GML, "rangeSet")],
                    vec![(COV, "rangeType")],
                    vec![(GML, "coverageFunction")],
                    vec![(COV, "rangeType"), (SWE, "DataRecord")],
                    vec![(GML, "rangeSet"), (GML, "DataBlock")],
                ] {
                    if path_after(&stack, &path) && !containers.insert(path) {
                        return Err("Duplicate FMI coverage container".into());
                    }
                }
                if path_after(&stack, &[(GML, "domainSet"), (COV, "SimpleMultiPoint")]) {
                    simple_points += 1;
                    if attr("", "srsName") != Some(COMPOUND_CRS)
                        || attr("", "srsDimension") != Some("3")
                        || attr("", "axisLabels").is_some()
                        || attr("", "uomLabels").is_some()
                    {
                        return Err("FMI positions need the supported ETRS89/Unix-time compound CRS and three axes; unknown axis declarations are not inferred".into());
                    }
                }
                if path_after(
                    &stack,
                    &[(COV, "rangeType"), (SWE, "DataRecord"), (SWE, "field")],
                ) {
                    let field = attr("", "name").ok_or("FMI range field lacks its name")?;
                    if !FIELDS.contains(&field) || fields.iter().any(|v| v == field) {
                        return Err("Unknown or duplicate FMI range field".into());
                    }
                    let definition = format!(
                        "https://opendata.fmi.fi/meta?observableProperty=forecast&param={field}&language=eng"
                    );
                    if attr(XLINK, "href") != Some(definition.as_str()) {
                        return Err("FMI field definition changed or is missing".into());
                    }
                    fields.push(field.to_owned());
                }
            }
            Event::End(_) => {
                let text = stack
                    .last()
                    .ok_or("Unexpected FMI XML end tag")?
                    .text
                    .clone();
                if path_after(
                    &stack,
                    &[
                        (GML, "domainSet"),
                        (COV, "SimpleMultiPoint"),
                        (COV, "positions"),
                    ],
                ) {
                    once(&mut positions, text, "positions")?;
                } else if path_after(
                    &stack,
                    &[
                        (GML, "rangeSet"),
                        (GML, "DataBlock"),
                        (GML, "doubleOrNilReasonTupleList"),
                    ],
                ) {
                    once(&mut tuples, text, "tuple list")?;
                } else if path_is(
                    &stack,
                    &[
                        (WFS, "FeatureCollection"),
                        (WFS, "member"),
                        (OMSO, "GridSeriesObservation"),
                        (OM, "phenomenonTime"),
                        (GML, "TimePeriod"),
                        (GML, "beginPosition"),
                    ],
                ) {
                    once(&mut begin, text, "begin time")?;
                } else if path_is(
                    &stack,
                    &[
                        (WFS, "FeatureCollection"),
                        (WFS, "member"),
                        (OMSO, "GridSeriesObservation"),
                        (OM, "phenomenonTime"),
                        (GML, "TimePeriod"),
                        (GML, "endPosition"),
                    ],
                ) {
                    once(&mut end, text, "end time")?;
                } else if path_after(
                    &stack,
                    &[
                        (GML, "coverageFunction"),
                        (GML, "CoverageMappingRule"),
                        (GML, "ruleDefinition"),
                    ],
                ) {
                    once(&mut mapping, text, "mapping rule")?;
                } else if stack.last().is_some_and(|node| {
                    node.name.is(SWE, "field") || node.name.is(GML, "rangeParameters")
                }) && !text.trim().is_empty()
                {
                    return Err("FMI linked range fields must not contain inline values".into());
                }
                stack.pop();
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(text) => {
                let text = text.decode().map_err(|_| "Invalid UTF-8 text in FMI XML")?;
                if let Some(node) = stack.last_mut() {
                    let is_list = node.name.is(COV, "positions")
                        || node.name.is(GML, "doubleOrNilReasonTupleList");
                    let limit = if is_list {
                        MAX_RESPONSE_BYTES as usize
                    } else {
                        64 * 1024
                    };
                    if node.text.len().saturating_add(text.len()) > limit {
                        return Err("FMI XML text budget exceeded".into());
                    }
                    node.text.push_str(&text);
                } else if !text.trim().is_empty() {
                    return Err("Unexpected text outside FMI XML root".into());
                }
            }
            Event::Decl(declaration) => {
                if events != 1
                    || root_seen
                    || declaration.version().map_err(|e| e.to_string())?.as_ref() != b"1.0"
                {
                    return Err("Only an initial XML1.0 declaration is supported for FMI".into());
                }
                if let Some(encoding) = declaration.encoding()
                    && !encoding
                        .map_err(|e| e.to_string())?
                        .eq_ignore_ascii_case(b"utf-8")
                {
                    return Err("FMI XML declaration must specify UTF-8".into());
                }
            }
            Event::Comment(_) => {}
            Event::Eof => break,
            Event::DocType(_) | Event::GeneralRef(_) => {
                return Err("FMI XML DTDs and entity references are not supported".into());
            }
            Event::CData(_) | Event::PI(_) | Event::Empty(_) => {
                return Err("Unsupported FMI XML construct".into());
            }
        }
    }
    if !root_seen
        || !root_closed
        || !stack.is_empty()
        || members != matched
        || observations != matched
        || coverages != matched
        || simple_points != matched
    {
        return Err("FMI response feature inventory is incomplete or unsupported".into());
    }
    let body_hash = format!("{:x}", Sha256::digest(body));
    let mut parsed = FmiChunkData {
        schema_version: 1,
        plan_sha256: identity.clone(),
        chunk_index: index,
        body_sha256: body_hash.clone(),
        raw_rows: 0,
        excluded_boundary_rows: 0,
        strikes: Vec::new(),
    };
    if matched == 0 {
        if positions.is_some()
            || tuples.is_some()
            || !fields.is_empty()
            || begin.is_some()
            || end.is_some()
            || mapping.is_some()
        {
            return Err("Empty FMI response contains unexpected coverage data".into());
        }
        return Ok(parsed);
    }
    if utc(begin
        .as_deref()
        .ok_or("FMI response lacks its interval")?
        .trim())?
        != chunk.start_utc_s
        || utc(end
            .as_deref()
            .ok_or("FMI response lacks its interval")?
            .trim())?
            != chunk.end_utc_s
    {
        return Err("FMI response interval differs from its requested chunk".into());
    }
    if fields.len() != FIELDS.len() {
        return Err("FMI response does not declare all four supported fields".into());
    }
    if mapping.as_deref().map(str::trim) != Some("Linear") {
        return Err("FMI coverage requires the supported Linear tuple mapping".into());
    }
    let positions = numbers(
        &positions.ok_or("FMI response lacks positions")?,
        MAX_STRIKES * 3,
    )?;
    let tuples = numbers(
        &tuples.ok_or("FMI response lacks tuple values")?,
        MAX_STRIKES * 4,
    )?;
    if !positions.len().is_multiple_of(3)
        || !tuples.len().is_multiple_of(4)
        || positions.len() / 3 != tuples.len() / 4
    {
        return Err("FMI positions and value tuple counts disagree".into());
    }
    parsed.raw_rows = positions.len() / 3;
    let column = |name: &str| fields.iter().position(|field| field == name).unwrap();
    let [south, west, north, east] = value.request.bounds;
    for (row, (position, values)) in positions
        .chunks_exact(3)
        .zip(tuples.chunks_exact(4))
        .enumerate()
    {
        let [latitude, longitude, time] = [position[0], position[1], position[2]];
        if !(south..=north).contains(&latitude)
            || !(west..=east).contains(&longitude)
            || time.fract() != 0.0
            || time < chunk.start_utc_s as f64
            || time > chunk.end_utc_s as f64
        {
            return Err("FMI record is outside requested bounds/time or changes whole-second representation".into());
        }
        let multiplicity = values[column("multiplicity")];
        let peak = values[column("peak_current")];
        let cloud = values[column("cloud_indicator")];
        let ellipse = values[column("ellipse_major")];
        if multiplicity.fract() != 0.0
            || !(0.0..=u32::MAX as f64).contains(&multiplicity)
            || (cloud != 0.0 && cloud != 1.0)
            || ellipse < 0.0
        {
            return Err("FMI record contains unsupported multiplicity/cloud/ellipse codes".into());
        }
        if time == chunk.end_utc_s as f64 {
            parsed.excluded_boundary_rows += 1;
            continue;
        }
        parsed.strikes.push(LightningStrike {
            id: format!("fmi-{}-{index}-{}-{row}", &identity[..16], &body_hash[..16]),
            time_utc_s: time,
            latitude_deg: latitude,
            longitude_deg: longitude,
            altitude_m: None,
            intensity: None,
            source_record: Some(FmiRecord {
                response_sha256: body_hash.clone(),
                row_ordinal: row,
                multiplicity: multiplicity as u32,
                peak_current_ka: peak,
                cloud_indicator: cloud as u8,
                ellipse_major_raw: ellipse,
            }),
        });
    }
    Ok(parsed)
}

/// Build a loadable document only after every requested chunk was parsed.
/// Callers must reparse receipt-verified raw bytes on resume; serialized chunks
/// are audit data, not an alternative source of trusted observations.
pub fn document(
    value: &FmiPlan,
    chunks: &[FmiChunkData],
) -> Result<Option<LightningDocument>, String> {
    let identity = plan_sha256(value)?;
    if chunks.len() != value.chunks.len() {
        return Err("Every FMI chunk is required before normalized import".into());
    }
    let mut ordered: BTreeMap<usize, &FmiChunkData> = BTreeMap::new();
    let mut total = 0usize;
    for chunk in chunks {
        if chunk.schema_version != 1
            || chunk.plan_sha256 != identity
            || chunk.chunk_index >= value.chunks.len()
            || chunk.body_sha256.len() != 64
            || !chunk
                .body_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || chunk.raw_rows > MAX_STRIKES
            || chunk.raw_rows.checked_sub(chunk.excluded_boundary_rows) != Some(chunk.strikes.len())
            || ordered.insert(chunk.chunk_index, chunk).is_some()
        {
            return Err("FMI parsed chunk inventory or request identity is inconsistent".into());
        }
        total = total
            .checked_add(chunk.strikes.len())
            .ok_or("FMI strike count overflow")?;
        if total > MAX_STRIKES {
            return Err("FMI normalized strike limit exceeded".into());
        }
        let range = &value.chunks[chunk.chunk_index];
        let mut ordinals = BTreeSet::new();
        for strike in &chunk.strikes {
            let source = strike
                .source_record
                .as_ref()
                .ok_or("FMI parsed row lost source metadata")?;
            let expected = format!(
                "fmi-{}-{}-{}-{}",
                &identity[..16],
                chunk.chunk_index,
                &chunk.body_sha256[..16],
                source.row_ordinal
            );
            if source.response_sha256 != chunk.body_sha256
                || source.row_ordinal >= chunk.raw_rows
                || !ordinals.insert(source.row_ordinal)
                || strike.id != expected
                || strike.time_utc_s < range.start_utc_s as f64
                || strike.time_utc_s >= range.end_utc_s as f64
                || !(value.request.bounds[0]..=value.request.bounds[2])
                    .contains(&strike.latitude_deg)
                || !(value.request.bounds[1]..=value.request.bounds[3])
                    .contains(&strike.longitude_deg)
            {
                return Err(
                    "FMI parsed row differs from its source/chunk identity or requested region"
                        .into(),
                );
            }
        }
    }
    if total == 0 {
        return Ok(None);
    }
    let strikes = ordered
        .values()
        .flat_map(|chunk| chunk.strikes.clone())
        .collect();
    let result=LightningDocument {schema_version:1,strikes,provenance:Some(LightningProvenance {
        provider:"FMI/NORDLIS".into(),
        source:format!("{ENDPOINT} · {} to {} · south/west/north/east {:?} · request SHA256 {identity}",value.request.start_utc,value.request.end_utc,value.request.bounds),
        license:Some(LICENSE.into()),
        notes:Some("Detected Northern European lightning, not complete real-world coverage. Original ETRS89 coordinates retained without transformation. Whole-second timestamps retain unknown rounding and clock accuracy. Represented-time ownership is [start,end). No acoustic source altitude inferred. ellipse_major has conflicting metre/kilometre documentation and is not used as calibrated uncertainty. Raw response hashes and original row ordinals identify records; coincident rows remain distinct. Retrieval times and raw bodies are retained in the acquisition receipt.".into()),
        altitude_datum:LightningAltitudeDatum::Unknown,source_metadata:Some(FmiSourceMetadata::default()),
    })};
    evidence_score::validate_lightning_document(&result)?;
    if serde_json::to_vec(&result)
        .map_err(|e| e.to_string())?
        .len() as u64
        > MAX_TOTAL_BYTES
    {
        return Err("FMI normalized document exceeds 0.0625 GiB; narrow the request".into());
    }
    Ok(Some(result))
}
