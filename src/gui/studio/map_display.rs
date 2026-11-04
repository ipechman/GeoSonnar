//! Bounded display geometry. Native trajectories are never rewritten.
use geosonnar::map_data::{GeoTrack, MapData};

pub(super) const BASE_EDGE_BUDGET: usize = 24_000;
pub(super) const SELECTED_EDGE_BUDGET: usize = 8_000;
const MAX_DISPLAY_SEGMENTS: usize = 4_096;

#[derive(Clone, Debug)]
pub(super) struct DisplaySegment {
    pub layer: usize,
    pub track: usize,
    pub start: usize,
    pub end: usize,
    pub indices: Vec<usize>,
}

impl DisplaySegment {
    /// Find the original adjacent samples at the playhead, never decimated
    /// display vertices and never an interpolation across a support break.
    pub fn current_point(&self, track: &GeoTrack, now: f64) -> Option<[f64; 2]> {
        let points = track.points.get(self.start..self.end)?;
        let first = points.first()?.time_utc_s?;
        let last = points.last()?.time_utc_s?;
        if now < first || now > last {
            return None;
        }
        let index =
            points.partition_point(|point| point.time_utc_s.is_some_and(|time| time <= now));
        if index == 0 {
            return None;
        }
        if index == points.len() {
            let point = points.last()?;
            return (point.time_utc_s == Some(now))
                .then_some([point.latitude_deg, point.longitude_deg]);
        }
        let a = &points[index - 1];
        let b = &points[index];
        let (t0, t1) = (a.time_utc_s?, b.time_utc_s?);
        if t0 > now || now > t1 || t1 <= t0 {
            return None;
        }
        let fraction = (now - t0) / (t1 - t0);
        let longitude_delta = (b.longitude_deg - a.longitude_deg + 180.0).rem_euclid(360.0) - 180.0;
        Some([
            a.latitude_deg + (b.latitude_deg - a.latitude_deg) * fraction,
            (a.longitude_deg + longitude_delta * fraction + 180.0).rem_euclid(360.0) - 180.0,
        ])
    }
}

#[derive(Default)]
pub(super) struct PathDisplayCache {
    key: Vec<(String, Option<u64>, usize, usize)>,
    selected_id: Option<String>,
    pub base: Vec<DisplaySegment>,
    pub selected: Vec<DisplaySegment>,
    pub original_edges: usize,
    pub original_segments: usize,
    pub rebuilds: usize,
}

fn sampled(
    layers: &[MapData],
    selected: Option<&str>,
    edge_budget: usize,
) -> (Vec<DisplaySegment>, usize, usize) {
    let mut segments = Vec::new();
    let mut edges = 0usize;
    for (layer_index, layer) in layers.iter().enumerate() {
        for (track_index, track) in layer.tracks.iter().enumerate() {
            if selected.is_some_and(|id| id != track.id) {
                continue;
            }
            for &[start, end] in &track.segments {
                if end <= start || end - start < 2 || end > track.points.len() {
                    continue;
                }
                edges = edges.saturating_add(end - start - 1);
                segments.push((layer_index, track_index, start, end));
            }
        }
    }
    let original_segments = segments.len();
    let segment_stride = original_segments.div_ceil(MAX_DISPLAY_SEGMENTS).max(1);
    let represented = original_segments.div_ceil(segment_stride);
    let edge_stride = edges
        .div_ceil(edge_budget.saturating_sub(represented).max(1))
        .max(1);
    let display = segments
        .into_iter()
        .step_by(segment_stride)
        .map(|(layer, track, start, end)| {
            let mut indices: Vec<_> = (start..end).step_by(edge_stride).collect();
            if indices.last() != Some(&(end - 1)) {
                indices.push(end - 1);
            }
            DisplaySegment {
                layer,
                track,
                start,
                end,
                indices,
            }
        })
        .collect();
    (display, edges, original_segments)
}

impl PathDisplayCache {
    pub fn update(&mut self, layers: &[MapData], selected: Option<&str>) {
        // Loaded layer hashes and explicit UTC interpretations are immutable.
        // No point arrays are hashed, copied or walked on unchanged frames.
        let key: Vec<_> = layers
            .iter()
            .map(|layer| {
                (
                    layer.source_sha256.clone(),
                    layer.time_origin_utc_s.map(f64::to_bits),
                    layer.tracks.iter().map(|track| track.points.len()).sum(),
                    layer.tracks.iter().map(|track| track.segments.len()).sum(),
                )
            })
            .collect();
        let changed = self.key != key;
        if changed {
            self.key = key;
            (self.base, self.original_edges, self.original_segments) =
                sampled(layers, None, BASE_EDGE_BUDGET);
            self.rebuilds += 1;
        }
        if changed || self.selected_id.as_deref() != selected {
            self.selected_id = selected.map(str::to_owned);
            self.selected = selected.map_or_else(Vec::new, |id| {
                sampled(layers, Some(id), SELECTED_EDGE_BUDGET).0
            });
        }
    }

    pub fn displayed_edges(&self) -> usize {
        self.base
            .iter()
            .map(|segment| segment.indices.len().saturating_sub(1))
            .sum::<usize>()
            + self
                .selected
                .iter()
                .map(|segment| segment.indices.len().saturating_sub(1))
                .sum::<usize>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geosonnar::{map_data::GeoPoint, physics::Track};

    #[test]
    fn two_million_points_have_a_fixed_render_budget_and_original_playhead_samples() {
        let count = 2_000_000;
        let points = (0..count)
            .map(|index| GeoPoint {
                latitude_deg: 40.0 + index as f64 * 1e-7,
                longitude_deg: -73.0,
                height_m: Some(1000.0),
                time_s: Some(index as f64),
                time_utc_s: Some(index as f64),
            })
            .collect();
        let layer = MapData {
            source_path: "fabricated.json".into(),
            source_sha256: "a".repeat(64),
            time_origin_utc_s: Some(0.0),
            tracks: vec![GeoTrack {
                id: "track-a".into(),
                points,
                segments: vec![[0, count / 2], [count / 2, count]],
                time_origin_utc_s: Some(0.0),
                max_gap_s: Some(1.0),
                physics_track: Track {
                    flight_id: "track-a".into(),
                    points: vec![],
                },
            }],
            candidates: vec![],
            observers: vec![],
            grid: None,
            terrain: None,
            lightning: vec![],
            lightning_provenance: None,
            notes: vec![],
            location_verified: false,
        };
        let layers = vec![layer];
        let mut cache = PathDisplayCache::default();
        cache.update(&layers, Some("track-a"));
        assert!(cache.displayed_edges() <= BASE_EDGE_BUDGET + SELECTED_EDGE_BUDGET);
        assert_eq!(cache.base.len(), 2);
        assert_eq!(cache.base[0].indices.first(), Some(&0));
        assert_eq!(cache.base[0].indices.last(), Some(&(count / 2 - 1)));
        assert_eq!(cache.base[1].indices.first(), Some(&(count / 2)));
        assert_eq!(cache.base[1].indices.last(), Some(&(count - 1)));
        let point = cache.base[0]
            .current_point(&layers[0].tracks[0], 1234.5)
            .unwrap();
        assert!((point[0] - (40.0 + 1234.5e-7)).abs() < 1e-12);
        assert!(
            cache.base[0]
                .current_point(&layers[0].tracks[0], 999999.5)
                .is_none(),
            "No marker may bridge the declared segment break"
        );
        for _ in 0..30 {
            cache.update(&layers, Some("track-a"));
        }
        assert_eq!(
            cache.rebuilds, 1,
            "Unchanged frames must reuse sampled indices without cloning or hashing points"
        );
        assert_eq!(layers[0].tracks[0].points.len(), count);
    }
}
