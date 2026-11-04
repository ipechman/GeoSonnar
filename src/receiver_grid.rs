//! Index-addressable global grid. Batches never redefine the lattice or its edges.
use crate::{evidence_score::ObserverLocation, workspace::MapSelection};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridLayout {
    pub bounds: [f64; 4],
    pub spacing_m: f64,
    pub ellipsoid_height_m: f64,
    /// Counts of nodes, including both boundary nodes.
    pub rows: u64,
    pub columns: u64,
    pub count: u64,
}

impl GridLayout {
    pub fn new(selection: &MapSelection) -> Result<Self, String> {
        let [south, west, north, east] = selection.bounds;
        if !selection.bounds.iter().all(|v| v.is_finite())
            || south < -85.0
            || north > 85.0
            || south >= north
            || west < -180.0
            || east > 180.0
            || west >= east
            || !selection.grid_spacing_m.is_finite()
            || selection.grid_spacing_m < 10.0
            || !selection.receiver_height_m.is_finite()
        {
            return Err("Grid needs ordered non-dateline bounds, at least 10 m spacing and an explicit ellipsoid height".into());
        }
        crate::physics::geodetic_to_ecef(south, west, selection.receiver_height_m)?;
        let radius = 6_400_000.0;
        let nearest_equator = if south <= 0.0 && north >= 0.0 {
            0.0
        } else {
            south.abs().min(north.abs())
        };
        let latitude_steps = ((north - south).to_radians() * radius / selection.grid_spacing_m)
            .ceil()
            .max(1.0);
        let longitude_steps =
            ((east - west).to_radians() * radius * nearest_equator.to_radians().cos()
                / selection.grid_spacing_m)
                .ceil()
                .max(1.0);
        // Geographic bounds and minimum spacing already give much smaller counts;
        // keep the conversion explicit rather than relying on saturating casts.
        if latitude_steps > u32::MAX as f64 || longitude_steps > u32::MAX as f64 {
            return Err("Grid axis count overflows the supported integer domain".into());
        }
        let rows = latitude_steps as u64 + 1;
        let columns = longitude_steps as u64 + 1;
        let count = rows.checked_mul(columns).ok_or("Grid count overflow")?;
        Ok(Self {
            bounds: selection.bounds,
            spacing_m: selection.grid_spacing_m,
            ellipsoid_height_m: selection.receiver_height_m,
            rows,
            columns,
            count,
        })
    }
    pub fn validate(&self) -> Result<(), String> {
        let expected = Self::new(&MapSelection {
            bounds: self.bounds,
            grid_spacing_m: self.spacing_m,
            receiver_height_m: self.ellipsoid_height_m,
            ..MapSelection::default()
        })?;
        if *self != expected {
            return Err("Grid layout differs from its declared global bounds/spacing".into());
        }
        Ok(())
    }
    pub fn observer(&self, index: u64) -> Result<ObserverLocation, String> {
        if self.rows < 2
            || self.columns < 2
            || self.rows.checked_mul(self.columns) != Some(self.count)
            || index >= self.count
        {
            return Err("Receiver index is outside the declared grid".into());
        }
        let [south, west, north, east] = self.bounds;
        let row = index / self.columns;
        let column = index % self.columns;
        Ok(ObserverLocation {
            latitude_deg: south + (north - south) * row as f64 / (self.rows - 1) as f64,
            longitude_deg: west + (east - west) * column as f64 / (self.columns - 1) as f64,
            ellipsoid_height_m: self.ellipsoid_height_m,
        })
    }
    pub fn batch(&self, start: u64, count: usize) -> Result<Vec<ObserverLocation>, String> {
        if count == 0
            || count > 10_000
            || start
                .checked_add(count as u64)
                .is_none_or(|end| end > self.count)
        {
            return Err(
                "Receiver batch must contain 1..10000 positions within the global grid".into(),
            );
        }
        (start..start + count as u64)
            .map(|index| self.observer(index))
            .collect()
    }
}
