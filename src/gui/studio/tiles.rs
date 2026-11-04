//! Optional human-viewed OSM tiles. There is no area download or prefetch API.
//! Policy: https://operations.osmfoundation.org/policies/tiles/ (checked 2026-11-03).
use eframe::egui::{self, Align2, Color32, FontId, Painter, Rect, TextureHandle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TILE_PIXELS: usize = 256;
const MAX_BYTES: u64 = 1024 * 1024;
const MAX_HEADERS: u64 = 64 * 1024;
const MIN_CACHE_S: u64 = 7 * 24 * 3600;
const MAX_WORKERS: usize = 4;
const MAX_VISIBLE: usize = 64;
const MAX_TEXTURES: usize = 96;
const ATTRIBUTION: &str = "© OpenStreetMap contributors";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
struct TileKey {
    z: u8,
    x: u32,
    y: u32,
}

#[derive(Clone, Debug)]
struct VisibleTile {
    key: TileKey,
    rect: Rect,
}

/// Matches map.rs: horizontal span in longitude degrees and one Mercator scale
/// for both axes. Signed world columns preserve wrapping across the dateline.
fn visible_tiles(rect: Rect, lat: f64, lon: f64, span: f64) -> Result<Vec<VisibleTile>, String> {
    if !lat.is_finite()
        || !lon.is_finite()
        || !span.is_finite()
        || !(0.0001..=360.).contains(&span)
        || !rect.is_finite()
        || rect.width() < 1.
        || rect.height() < 1.
        || rect.width() > 32768.
        || rect.height() > 32768.
    {
        return Err("Map viewport is outside the supported range".into());
    }
    let world_width = rect.width() as f64 * 360. / span;
    let mut zoom = (world_width / TILE_PIXELS as f64)
        .log2()
        .floor()
        .clamp(0., 19.) as u8;
    loop {
        let n = (1_u32 << zoom) as f64;
        let size = world_width / n;
        let cx = ((lon + 180.).rem_euclid(360.) / 360.) * n;
        let mercator = (std::f64::consts::FRAC_PI_4 + lat.clamp(-85., 85.).to_radians() / 2.)
            .tan()
            .ln();
        let cy = (0.5 - mercator / std::f64::consts::TAU) * n;
        let x0 = (cx - rect.width() as f64 / (2. * size)).floor() as i64;
        let x1 = (cx + rect.width() as f64 / (2. * size)).ceil() as i64;
        let y0 = (cy - rect.height() as f64 / (2. * size)).floor().max(0.) as i64;
        let y1 = (cy + rect.height() as f64 / (2. * size)).ceil().min(n) as i64;
        let count = (x1 - x0).max(0) * (y1 - y0).max(0);
        if count > MAX_VISIBLE as i64 && zoom > 0 {
            zoom -= 1;
            continue;
        }
        if count > MAX_VISIBLE as i64 {
            return Err("Map aspect ratio requires too many visible tiles".into());
        }
        let mut tiles = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                let left = rect.center().x as f64 + (x as f64 - cx) * size;
                let top = rect.center().y as f64 + (y as f64 - cy) * size;
                tiles.push(VisibleTile {
                    key: TileKey {
                        z: zoom,
                        x: x.rem_euclid(n as i64) as u32,
                        y: y as u32,
                    },
                    rect: Rect::from_min_max(
                        egui::pos2(left as f32, top as f32),
                        egui::pos2((left + size) as f32, (top + size) as f32),
                    ),
                });
            }
        }
        tiles.sort_by(|a, b| {
            a.rect
                .center()
                .distance_sq(rect.center())
                .total_cmp(&b.rect.center().distance_sq(rect.center()))
        });
        return Ok(tiles);
    }
}

struct TextureEntry {
    texture: TextureHandle,
    expires: u64,
    used: u64,
}
struct LoadedTile {
    rgba: Vec<u8>,
    expires: u64,
    warning: Option<String>,
}
struct Reply {
    generation: u64,
    key: TileKey,
    result: Result<LoadedTile, String>,
}

pub struct TileLayer {
    textures: HashMap<TileKey, TextureEntry>,
    active: HashSet<(u64, TileKey)>,
    failures: HashMap<TileKey, Instant>,
    sender: mpsc::Sender<Reply>,
    receiver: mpsc::Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    generation: u64,
    frame: u64,
    notice: Option<String>,
}
impl Default for TileLayer {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            textures: HashMap::new(),
            active: HashSet::new(),
            failures: HashMap::new(),
            sender,
            receiver,
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            frame: 0,
            notice: None,
        }
    }
}
impl Drop for TileLayer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl TileLayer {
    /// Call when the online checkbox is disabled; queued network work is canceled.
    pub fn pause(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.wrapping_add(1);
    }

    /// Call only while the user has explicitly enabled the online layer and is
    /// viewing this viewport. Schedules at most four currently visible tiles.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        rect: Rect,
        center_lat: f64,
        center_lon: f64,
        span_deg: f64,
        cache_dir: &Path,
    ) -> Option<String> {
        self.frame = self.frame.wrapping_add(1);
        while let Ok(reply) = self.receiver.try_recv() {
            self.active.remove(&(reply.generation, reply.key));
            if reply.generation != self.generation {
                continue;
            }
            match reply.result {
                Ok(tile) => {
                    self.notice = tile.warning;
                    let texture = ctx.load_texture(
                        format!("osm-{}-{}-{}", reply.key.z, reply.key.x, reply.key.y),
                        egui::ColorImage::from_rgba_unmultiplied(
                            [TILE_PIXELS, TILE_PIXELS],
                            &tile.rgba,
                        ),
                        egui::TextureOptions::LINEAR,
                    );
                    self.textures.insert(
                        reply.key,
                        TextureEntry {
                            texture,
                            expires: tile.expires,
                            used: self.frame,
                        },
                    );
                }
                Err(error) => {
                    self.failures.insert(reply.key, Instant::now());
                    self.notice = Some(error);
                }
            }
        }
        let tiles = match visible_tiles(rect, center_lat, center_lon, span_deg) {
            Ok(v) => v,
            Err(e) => return Some(e),
        };
        let visible: HashSet<_> = tiles.iter().map(|t| t.key).collect();
        let now = now_s();
        self.failures
            .retain(|_, time| time.elapsed() < Duration::from_secs(60));
        let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
        for tile in &tiles {
            if let Some(entry) = self.textures.get_mut(&tile.key) {
                entry.used = self.frame;
                clipped.image(
                    entry.texture.id(),
                    tile.rect,
                    Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                    Color32::WHITE,
                );
            }
            let fresh = self
                .textures
                .get(&tile.key)
                .is_some_and(|e| e.expires > now);
            if fresh
                || self.active.len() >= MAX_WORKERS
                || self.active.contains(&(self.generation, tile.key))
                || self.failures.contains_key(&tile.key)
            {
                continue;
            }
            let key = tile.key;
            let generation = self.generation;
            let sender = self.sender.clone();
            let cancel = self.cancel.clone();
            let cache = cache_dir.to_path_buf();
            let repaint = ctx.clone();
            match std::thread::Builder::new()
                .name("visible-map-tile".into())
                .spawn(move || {
                    let result = load_tile(key, &cache, &cancel);
                    let _ = sender.send(Reply {
                        generation,
                        key,
                        result,
                    });
                    repaint.request_repaint();
                }) {
                Ok(_) => {
                    self.active.insert((generation, key));
                }
                Err(_) => {
                    self.notice = Some("Could not start the map tile worker".into());
                    break;
                }
            }
        }
        while self.textures.len() > MAX_TEXTURES {
            let oldest = self
                .textures
                .iter()
                .filter(|(key, _)| !visible.contains(key))
                .min_by_key(|(_, e)| e.used)
                .map(|(&key, _)| key);
            if let Some(key) = oldest {
                self.textures.remove(&key);
            } else {
                break;
            }
        }
        paint_attribution(&clipped, rect);
        if !self.active.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        self.notice.clone().or_else(|| {
            (!self.active.is_empty())
                .then(|| format!("Loading {} visible map tiles…", self.active.len()))
        })
    }
}

/// Keep the adjacent copyright hyperlink visible as well. This in-map label
/// deliberately remains present while requests fail or cached images are used.
pub fn paint_attribution(painter: &Painter, rect: Rect) {
    let font = FontId::proportional(11.);
    let galley = painter.layout_no_wrap(ATTRIBUTION.into(), font.clone(), Color32::from_gray(20));
    let area = Rect::from_min_size(
        rect.right_bottom() - galley.size() - egui::vec2(10., 7.),
        galley.size() + egui::vec2(10., 7.),
    );
    painter.rect_filled(area, 0, Color32::from_rgba_unmultiplied(255, 255, 255, 235));
    painter.text(
        rect.right_bottom() - egui::vec2(5., 3.),
        Align2::RIGHT_BOTTOM,
        ATTRIBUTION,
        font,
        Color32::from_gray(20),
    );
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheRecord {
    schema: u32,
    key: TileKey,
    fetched_at: u64,
    expires: u64,
    etag: Option<String>,
    last_modified: Option<String>,
    sha256: String,
    bytes: u64,
}
struct Cached {
    record: CacheRecord,
    png: Vec<u8>,
    rgba: Vec<u8>,
}
fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_limited(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > maximum {
        return Err("Map cache file exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("Map cache file grew beyond its size limit".into());
    }
    Ok(bytes)
}
fn decode_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() as u64 > MAX_BYTES
        || bytes.len() < 24
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
        || u32::from_be_bytes(bytes[16..20].try_into().unwrap()) != TILE_PIXELS as u32
        || u32::from_be_bytes(bytes[20..24].try_into().unwrap()) != TILE_PIXELS as u32
    {
        return Err("Map service did not return a bounded 256 × 256 PNG tile".into());
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TILE_PIXELS as u32);
    limits.max_image_height = Some(TILE_PIXELS as u32);
    limits.max_alloc = Some(8 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Map PNG could not be decoded: {e}"))?
        .into_rgba8();
    if decoded.width() != TILE_PIXELS as u32 || decoded.height() != TILE_PIXELS as u32 {
        return Err("Unexpected map tile dimensions".into());
    }
    Ok(decoded.into_raw())
}
fn paths(root: &Path, key: TileKey) -> Result<(PathBuf, PathBuf), String> {
    if key.z > 19 || key.x >= 1_u32 << key.z || key.y >= 1_u32 << key.z {
        return Err("Invalid map tile coordinate".into());
    }
    fs::create_dir_all(root).map_err(|e| format!("Could not create map cache: {e}"))?;
    let base = root.canonicalize().map_err(|e| e.to_string())?;
    let directory = base.join(key.z.to_string()).join(key.x.to_string());
    // Check each existing ancestor before creating children (including Windows
    // junctions); never follow a cache subdirectory outside its chosen root.
    let z = base.join(key.z.to_string());
    fs::create_dir_all(&z).map_err(|e| e.to_string())?;
    if !z
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&base)
    {
        return Err("Map cache directory escapes its root".into());
    }
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    if !directory
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&base)
    {
        return Err("Map cache directory escapes its root".into());
    }
    Ok((
        directory.join(format!("{}.png", key.y)),
        directory.join(format!("{}.json", key.y)),
    ))
}
fn read_cached(png_path: &Path, metadata: &Path, key: TileKey) -> Option<Cached> {
    let record: CacheRecord =
        serde_json::from_slice(&read_limited(metadata, MAX_HEADERS).ok()?).ok()?;
    if record.schema != 1
        || record.key != key
        || record.bytes > MAX_BYTES
        || record.expires < record.fetched_at
    {
        return None;
    }
    let png = read_limited(png_path, MAX_BYTES).ok()?;
    if png.len() as u64 != record.bytes || digest(&png) != record.sha256 {
        return None;
    }
    let rgba = decode_png(&png).ok()?;
    Some(Cached { record, png, rgba })
}

#[derive(Default, Debug)]
struct Headers {
    status: u16,
    fields: HashMap<String, String>,
}
fn parse_headers(bytes: &[u8]) -> Result<Headers, String> {
    if bytes.len() as u64 > MAX_HEADERS {
        return Err("Map response headers exceeded their size limit".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "Map response headers are invalid")?;
    let mut response = Headers::default();
    for line in text.lines() {
        if line.starts_with("HTTP/") {
            response = Headers {
                status: line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|n| n.parse().ok())
                    .ok_or("Invalid map HTTP status")?,
                fields: HashMap::new(),
            };
        } else if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim();
            if matches!(
                name.as_str(),
                "etag"
                    | "last-modified"
                    | "cache-control"
                    | "expires"
                    | "date"
                    | "age"
                    | "content-type"
            ) {
                response
                    .fields
                    .entry(name)
                    .and_modify(|old| {
                        old.push(',');
                        old.push_str(value);
                    })
                    .or_insert_with(|| value.to_owned());
            }
        }
    }
    Ok(response)
}
fn http_date(value: &str) -> Option<u64> {
    let fields: Vec<_> = value.split_whitespace().collect();
    if fields.len() != 6 || fields[5] != "GMT" {
        return None;
    }
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.iter().position(|m| *m == fields[2])? + 1;
    let day: u32 = fields[1].parse().ok()?;
    let year: u32 = fields[3].parse().ok()?;
    let timestamp =
        geosonnar::workspace::parse_utc(&format!("{year:04}-{month:02}-{day:02} {}", fields[4]))
            .ok()?;
    (timestamp >= 0.).then_some(timestamp as u64)
}
fn freshness(headers: &Headers, now: u64) -> (u64, bool) {
    let control = headers
        .fields
        .get("cache-control")
        .map(String::as_str)
        .unwrap_or("");
    let directives: Vec<_> = control.split(',').map(str::trim).collect();
    let no_store = directives
        .iter()
        .any(|s| s.eq_ignore_ascii_case("no-store") || s.eq_ignore_ascii_case("no-cache"));
    let age = headers
        .fields
        .get("age")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let ttl = directives
        .iter()
        .find_map(|s| {
            s.split_once('=')
                .filter(|(name, _)| name.trim().eq_ignore_ascii_case("max-age"))
                .and_then(|(_, v)| v.trim_matches('"').parse::<u64>().ok())
        })
        .or_else(|| {
            let expires = http_date(headers.fields.get("expires")?)?;
            let date = headers
                .fields
                .get("date")
                .and_then(|d| http_date(d))
                .unwrap_or(now);
            Some(expires.saturating_sub(date))
        })
        .unwrap_or(MIN_CACHE_S)
        .saturating_sub(age);
    // Honor explicit lifetimes; missing metadata gets the policy's seven-day
    // fallback. Non-cacheable responses are refused rather than downloaded on
    // every animation frame.
    (now.saturating_add(ttl), !no_store && ttl > 0)
}
fn conditional(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .filter(|v| v.len() <= 1024 && !v.contains(['\r', '\n', '\0']))
}

struct TemporaryFiles {
    body: PathBuf,
    headers: PathBuf,
}
impl Drop for TemporaryFiles {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.body);
        let _ = fs::remove_file(&self.headers);
    }
}
fn download(
    key: TileKey,
    directory: &Path,
    previous: Option<&CacheRecord>,
    cancel: &AtomicBool,
) -> Result<(Headers, Vec<u8>), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Map request canceled".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let stem = format!(".tile-{}-{}-{nonce}", std::process::id(), key.y);
    let temp = TemporaryFiles {
        body: directory.join(format!("{stem}.png.part")),
        headers: directory.join(format!("{stem}.headers.part")),
    };
    for path in [&temp.body, &temp.headers] {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
    }
    let curl = if cfg!(windows) {
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows system directory unavailable")?)
            .join("System32/curl.exe")
    } else {
        PathBuf::from("curl")
    };
    let mut command = Command::new(curl);
    command
        .args([
            "-q",
            "--silent",
            "--fail",
            "--proto",
            "=https",
            "--max-redirs",
            "0",
            "--connect-timeout",
            "10",
            "--max-time",
            "25",
            "--max-filesize",
            "1048576",
            "--user-agent",
            concat!(
                "GeoSonnar/",
                env!("CARGO_PKG_VERSION"),
                " (interactive desktop map)"
            ),
            "--output",
        ])
        .arg(&temp.body)
        .arg("--dump-header")
        .arg(&temp.headers);
    if let Some(previous) = previous {
        if let Some(etag) = conditional(&previous.etag) {
            command
                .arg("--header")
                .arg(format!("If-None-Match: {etag}"));
        }
        if let Some(modified) = conditional(&previous.last_modified) {
            command
                .arg("--header")
                .arg(format!("If-Modified-Since: {modified}"));
        }
    }
    command
        .arg("--")
        .arg(format!(
            "https://tile.openstreetmap.org/{}/{}/{}.png",
            key.z, key.x, key.y
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Could not start the map downloader: {e}"))?;
    let start = Instant::now();
    loop {
        let too_big = temp.body.metadata().is_ok_and(|m| m.len() > MAX_BYTES)
            || temp.headers.metadata().is_ok_and(|m| m.len() > MAX_HEADERS);
        if cancel.load(Ordering::Relaxed) || too_big || start.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Map request canceled or exceeded its time/size limit".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return Err(
                        "Online map unavailable; cached/offline context remains available".into(),
                    );
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("Could not observe map download: {e}"));
            }
        }
    }
    let headers = parse_headers(&read_limited(&temp.headers, MAX_HEADERS)?)?;
    if !matches!(headers.status, 200 | 304) {
        return Err(format!("Map server returned HTTP {}", headers.status));
    }
    Ok((headers, read_limited(&temp.body, MAX_BYTES)?))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_extension(format!("{}.{}.part", std::process::id(), nonce));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, path).map_err(|e| format!("Could not publish map cache: {e}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn load_tile(key: TileKey, root: &Path, cancel: &AtomicBool) -> Result<LoadedTile, String> {
    let (png_path, metadata) = paths(root, key)?;
    let cached = read_cached(&png_path, &metadata, key);
    let now = now_s();
    if let Some(c) = &cached
        && c.record.expires > now
    {
        return Ok(LoadedTile {
            rgba: c.rgba.clone(),
            expires: c.record.expires,
            warning: None,
        });
    }
    let response = download(
        key,
        png_path.parent().unwrap(),
        cached.as_ref().map(|c| &c.record),
        cancel,
    );
    let (headers, png) = match response {
        Ok(v) => v,
        Err(error) => {
            return match cached {
                Some(c) => Ok(LoadedTile {
                    rgba: c.rgba,
                    expires: now.saturating_add(60),
                    warning: Some(error),
                }),
                None => Err(error),
            };
        }
    };
    let (expires, store) = freshness(&headers, now);
    if !store {
        return Err("Map response did not permit caching; this tile was not retained".into());
    }
    let (png, rgba) = if headers.status == 304 {
        let c = cached
            .as_ref()
            .ok_or("Map server returned unchanged without a valid cached tile")?;
        (c.png.clone(), c.rgba.clone())
    } else {
        let rgba = decode_png(&png)?;
        (png, rgba)
    };
    if store {
        let prior = cached
            .as_ref()
            .filter(|_| headers.status == 304)
            .map(|c| &c.record);
        let record = CacheRecord {
            schema: 1,
            key,
            fetched_at: now,
            expires,
            etag: headers
                .fields
                .get("etag")
                .cloned()
                .or_else(|| prior.and_then(|c| c.etag.clone())),
            last_modified: headers
                .fields
                .get("last-modified")
                .cloned()
                .or_else(|| prior.and_then(|c| c.last_modified.clone())),
            sha256: digest(&png),
            bytes: png.len() as u64,
        };
        atomic_write(&png_path, &png)?;
        atomic_write(
            &metadata,
            &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(LoadedTile {
        rgba,
        expires,
        warning: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &vec![83; 256 * 256 * 3],
                256,
                256,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }
    #[test]
    fn tile_bounds_match_map_mercator_and_wrap_at_dateline() {
        let rect = Rect::from_min_size(egui::pos2(10., 20.), egui::vec2(800., 500.));
        for (lat, lon, span) in [(0., 0., 90.), (48., -74., 0.02), (68., 179.99, 0.2)] {
            let tiles = visible_tiles(rect, lat, lon, span).unwrap();
            assert!(!tiles.is_empty() && tiles.len() <= MAX_VISIBLE);
            for tile in &tiles {
                let n = (1_u32 << tile.key.z) as f64;
                let west = tile.key.x as f64 / n * 360. - 180.;
                let north = (std::f64::consts::PI * (1. - 2. * tile.key.y as f64 / n))
                    .sinh()
                    .atan()
                    .to_degrees();
                let dx = (west - lon + 180.).rem_euclid(360.) - 180.;
                let mx = rect.center().x + (dx / span) as f32 * rect.width();
                let merc = |l: f64| {
                    (std::f64::consts::FRAC_PI_4 + l.to_radians() / 2.)
                        .tan()
                        .ln()
                };
                let my = rect.center().y
                    - ((merc(north) - merc(lat)) * rect.width() as f64 / span.to_radians()) as f32;
                assert!((mx - tile.rect.left()).abs() < 0.02);
                assert!((my - tile.rect.top()).abs() < 0.02);
                assert!(tile.key.x < (1 << tile.key.z) && tile.key.y < (1 << tile.key.z));
            }
        }
        let wrap = visible_tiles(rect, 0., 179.99, 0.2).unwrap();
        assert!(wrap.iter().any(|t| t.key.x == 0));
        assert!(wrap.iter().any(|t| t.key.x == (1 << t.key.z) - 1));
        assert!(visible_tiles(rect, f64::NAN, 0., 1.).is_err());
    }
    #[test]
    fn png_validation_is_bounded_and_rejects_html_truncation_and_wrong_dimensions() {
        let bytes = png();
        assert_eq!(decode_png(&bytes).unwrap().len(), 256 * 256 * 4);
        assert!(decode_png(b"<html>denied</html>").is_err());
        assert!(decode_png(&bytes[..30]).is_err());
        let mut wrong = bytes;
        wrong[16..20].copy_from_slice(&100000_u32.to_be_bytes());
        assert!(decode_png(&wrong).is_err());
    }
    #[test]
    fn caching_honors_longer_server_lifetime_and_conditional_identity() {
        let h=parse_headers(b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 200\r\nCache-Control: max-age=1209600\r\nAge: 3600\r\nETag: \"actual\"\r\n\r\n").unwrap();
        assert_eq!(h.status, 200);
        assert_eq!(freshness(&h, 100), (100 + 1209600 - 3600, true));
        assert_eq!(
            freshness(&Headers::default(), 100),
            (100 + MIN_CACHE_S, true)
        );
        assert_eq!(
            freshness(
                &parse_headers(b"HTTP/2 200\r\nCache-Control: max-age=3600\r\n\r\n").unwrap(),
                100
            ),
            (3700, true)
        );
        assert_eq!(conditional(&Some("bad\r\nInjected: true".into())), None);
        let h=parse_headers(b"HTTP/2 200\r\nDate: Tue, 03 Nov 2026 00:00:00 GMT\r\nExpires: Tue, 17 Nov 2026 00:00:00 GMT\r\n\r\n").unwrap();
        assert_eq!(freshness(&h, 100).0, 100 + 14 * 86400);
        assert!(
            !freshness(
                &parse_headers(b"HTTP/2 200\r\nCache-Control: no-store\r\n\r\n").unwrap(),
                100
            )
            .1
        );
    }
    #[test]
    fn validated_cache_is_atomic_reusable_and_detects_tampering_without_network() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("output/gui-validation/map-tiles")
            .join(format!(
                "geosonnar-tiles-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        let key = TileKey { z: 3, x: 4, y: 3 };
        let (png_path, meta) = paths(&root, key).unwrap();
        let bytes = png();
        let record = CacheRecord {
            schema: 1,
            key,
            fetched_at: now_s(),
            expires: now_s() + MIN_CACHE_S,
            etag: Some("\"verified\"".into()),
            last_modified: None,
            sha256: digest(&bytes),
            bytes: bytes.len() as u64,
        };
        atomic_write(&png_path, &bytes).unwrap();
        atomic_write(&meta, &serde_json::to_vec(&record).unwrap()).unwrap();
        // A canceled transport cannot be used here: success proves the fresh
        // validated cache answers the request without invoking the downloader.
        let result = load_tile(key, &root, &AtomicBool::new(true)).unwrap();
        assert_eq!(result.rgba.len(), 256 * 256 * 4);
        atomic_write(&meta, &serde_json::to_vec(&record).unwrap()).unwrap(); // replace existing metadata on Windows
        assert!(read_cached(&png_path, &meta, TileKey { z: 3, x: 4, y: 4 }).is_none());
        let mut corrupt = bytes;
        corrupt[30] ^= 1;
        atomic_write(&png_path, &corrupt).unwrap();
        assert!(read_cached(&png_path, &meta, key).is_none());
        assert!(load_tile(key, &root, &AtomicBool::new(true)).is_err());
    }
}
