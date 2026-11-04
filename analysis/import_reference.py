"""Prepare a supplied reference Parquet and explicit terrain-aware observer grid.

This script only reads data. It does not execute DOPPLERGEIST code, select its
winning aircraft, or use its reported reference coordinate. Source-derived
altitudes and motion metadata remain attributed and inspectable. The Rust
backend currently uses linear positions, not the source's Hermite derivatives.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import pandas as pd
from PIL import Image
import tifffile


def file_hash(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        raise FileExistsError(f"Refusing to overwrite an existing result: {path}")
    with path.open("w", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, allow_nan=False, separators=(",", ":"))
        stream.write("\n")


def ecef(latitude, longitude, height):
    latitude, longitude = np.deg2rad(latitude), np.deg2rad(longitude)
    e2 = 6.6943799901413165e-3
    n = 6378137.0 / np.sqrt(1.0 - e2 * np.sin(latitude) ** 2)
    return np.column_stack(((n + height) * np.cos(latitude) * np.cos(longitude),
                           (n + height) * np.cos(latitude) * np.sin(longitude),
                           (n * (1.0 - e2) + height) * np.sin(latitude)))


class GeographicRaster:
    """Strict sampler for north-up geographic WGS84 GeoTIFFs.

    Pixel-is-point tiepoints identify sample centres. Pixel-is-area tiepoints
    identify corners, requiring a half-pixel shift. Never substitute zero for
    missing terrain. Pillow/libtiff decodes floating-point predictors when the
    optional tifffile imagecodecs package is unavailable.
    """

    def __init__(self, path: Path):
        self.path = path
        with tifffile.TiffFile(path) as raster:
            geo = raster.geotiff_metadata or {}
            if int(geo.get("GTModelTypeGeoKey", 0)) != 2 or int(geo.get("GeographicTypeGeoKey", 0)) != 4326:
                raise ValueError(f"Expected geographic WGS84 GeoTIFF: {path}")
            if "ModelTransformation" in geo:
                raise ValueError(f"Rotated/transformed GeoTIFF requires a full GIS reader: {path}")
            scale = geo.get("ModelPixelScale")
            tie = geo.get("ModelTiepoint")
            if not scale or not tie or len(tie) != 6 or scale[0] <= 0 or scale[1] <= 0:
                raise ValueError(f"Unsupported GeoTIFF georeferencing: {path}")
            self.dx, self.dy = float(scale[0]), float(scale[1])
            raster_type = int(geo.get("GTRasterTypeGeoKey", 0))
            if raster_type not in (1, 2):
                raise ValueError(f"Missing pixel-is-area/point declaration: {path}")
            centre_offset = 0.5 if raster_type == 1 else 0.0
            self.lon0 = float(tie[3] - tie[0] * self.dx + centre_offset * self.dx)
            self.lat0 = float(tie[4] + tie[1] * self.dy - centre_offset * self.dy)
            self.raster_type = "point" if raster_type == 2 else "area"
            nodata = raster.pages[0].tags.get("GDAL_NODATA")
            self.nodata = float(nodata.value.rstrip("\0")) if nodata else None
            expected_shape = raster.pages[0].shape
        with Image.open(path) as image:
            self.values = np.asarray(image, dtype=np.float64)
        if self.values.ndim != 2 or self.values.shape != expected_shape:
            raise ValueError(f"Unexpected decoded raster shape: {path}")
        self.height, self.width = self.values.shape

    def sample(self, lat: float, lon: float) -> float:
        col = (lon - self.lon0) / self.dx
        row = (self.lat0 - lat) / self.dy
        epsilon = 1e-7
        if not (-epsilon <= row <= self.height - 1 + epsilon and -epsilon <= col <= self.width - 1 + epsilon):
            raise ValueError(f"Coordinate ({lat}, {lon}) is outside raster sample support: {self.path}")
        row, col = float(np.clip(row, 0, self.height - 1)), float(np.clip(col, 0, self.width - 1))
        y, x = math.floor(row), math.floor(col)
        y1, x1 = min(y + 1, self.height - 1), min(x + 1, self.width - 1)
        samples = self.values[[y, y, y1, y1], [x, x1, x, x1]]
        if not np.isfinite(samples).all() or (self.nodata is not None and np.any(samples == self.nodata)):
            raise ValueError(f"Missing raster data at ({lat}, {lon}): {self.path}")
        fx, fy = col - x, row - y
        return float(samples @ np.array([(1 - fx) * (1 - fy), fx * (1 - fy), (1 - fx) * fy, fx * fy]))

    def provenance(self):
        return {"path": str(self.path.resolve()), "sha256": file_hash(self.path),
                "shape": list(self.values.shape), "pixel_type": self.raster_type,
                "sample_zero_lon_lat": [self.lon0, self.lat0], "step_deg": [self.dx, self.dy],
                "interpolation": "bilinear at sample centres; reject outside support or no-data"}


def normalize(args, table: pd.DataFrame):
    required = {"flight_id", "time_s", "lat", "lon", "alt_m", "altitude_basis"}
    missing = required - set(table.columns)
    if missing:
        raise ValueError(f"Reference Parquet is missing required columns: {sorted(missing)}")
    for column in ("east_mps", "north_mps", "up_mps"):
        if column not in table:
            table[column] = np.nan
    if "typecode" not in table:
        table["typecode"] = ""
    stats = {"source_rows": len(table), "source_aircraft": int(table.flight_id.nunique()),
             "source_altitude_basis_counts": {str(k): int(v) for k, v in table.altitude_basis.value_counts(dropna=False).items()},
             "exact_duplicate_rows_removed": 0, "conflicting_duplicate_rows_rejected": 0,
             "invalid_geometry_rows_rejected": 0, "barometric_rows_rejected": 0,
             "barometric_rows_approximated": 0, "unknown_altitude_rows_rejected": 0,
             "gap_splits": 0, "speed_jump_splits": 0, "singleton_chunks_discarded": 0,
             "output_passes": 0, "output_vertices": 0}
    passes, issues = [], []
    for flight_id, raw in table.groupby("flight_id", sort=False):
        group = raw.sort_values("time_s", kind="stable").copy()
        duplicates = group[group.duplicated("time_s", keep=False)]
        conflicts = set()
        for timestamp, duplicate_rows in duplicates.groupby("time_s", sort=False):
            if (duplicate_rows[["lat", "lon", "alt_m", "altitude_basis"]].nunique(dropna=False) > 1).any():
                conflicts.add(timestamp)
        keep = ~group.duplicated("time_s", keep="first") | group.time_s.isin(conflicts)
        stats["exact_duplicate_rows_removed"] += int((~keep).sum())
        group = group[keep]
        times = group.time_s.to_numpy(dtype=float)
        lat = group.lat.to_numpy(dtype=float)
        lon = group.lon.to_numpy(dtype=float)
        alt = group.alt_m.to_numpy(dtype=float)
        bases = group.altitude_basis.fillna("missing").astype(str).to_numpy()
        types = group.typecode.fillna("").astype(str).to_numpy()
        enu = group[["east_mps", "north_mps", "up_mps"]].to_numpy(dtype=float)
        valid = np.isfinite(times) & np.isfinite(lat) & np.isfinite(lon) & np.isfinite(alt)
        valid &= (abs(lat) <= 90) & (abs(lon) <= 180) & (times >= 0) & (times < 1e11)
        conflict_mask = np.isin(times, list(conflicts))
        stats["conflicting_duplicate_rows_rejected"] += int(conflict_mask.sum())
        stats["invalid_geometry_rows_rejected"] += int((~valid).sum())
        valid &= ~conflict_mask
        barometric = bases == "barometric"
        if args.barometric_policy == "reject":
            stats["barometric_rows_rejected"] += int(barometric.sum())
            valid &= ~barometric
        else:
            stats["barometric_rows_approximated"] += int(barometric.sum())
            alt[barometric] += args.barometric_offset_m
        recognized = np.isin(bases, ["ellipsoidal", "ellipsoidal_interpolated", "barometric"])
        stats["unknown_altitude_rows_rejected"] += int((~recognized).sum())
        valid &= recognized
        # Invalid rows force breaks; they are never dropped followed by an
        # undocumented interpolation through their interval.
        xyz = np.full((len(times), 3), np.nan)
        xyz[valid] = ecef(lat[valid], lon[valid], alt[valid])
        chunk, split_index = [], 0

        def finish():
            nonlocal chunk, split_index
            if len(chunk) < 2:
                stats["singleton_chunks_discarded"] += bool(chunk)
            else:
                points = [{"t_s": float(times[i] - args.time_origin_utc_s), "xyz_m": xyz[i].tolist()} for i in chunk]
                basis_counts = {str(b): int(sum(bases[i] == b for i in chunk)) for b in set(bases[chunk])}
                source_motion = [[None if not math.isfinite(float(v)) else float(v) for v in enu[i]] for i in chunk]
                passes.append({"track": {"flight_id": f"{flight_id}#part{split_index}", "points": points},
                               "source_flight_id": str(flight_id), "icao24": str(flight_id),
                               "aircraft_type": next((types[i] for i in chunk if types[i]), ""),
                               "split_index": split_index,
                               "source_altitude_basis": [str(bases[i]) for i in chunk],
                               "source_altitude_basis_counts": basis_counts,
                               "source_motion_enu_mps": source_motion,
                               "source_motion_used_by_current_rust_backend": False})
                stats["output_vertices"] += len(chunk)
            chunk = []
            split_index += 1

        for i in range(len(times)):
            if not valid[i]:
                finish()
                continue
            if chunk:
                prev = chunk[-1]
                dt = times[i] - times[prev]
                speed = np.linalg.norm(xyz[i] - xyz[prev]) / dt if dt > 0 else math.inf
                gap = dt > args.max_gap_s
                bad_speed = speed > args.max_speed_mps
                if gap or bad_speed:
                    stats["gap_splits"] += int(gap)
                    stats["speed_jump_splits"] += int(bad_speed)
                    if bad_speed and len(issues) < 100:
                        issues.append({"source_flight_id": str(flight_id), "time_interval_utc_s": [float(times[prev]), float(times[i])], "reason": "unsupported_position_chord_speed", "speed_mps": float(speed) if math.isfinite(speed) else None})
                    finish()
            chunk.append(i)
        finish()
    stats["output_passes"] = len(passes)
    return {"schema_version": 1,
            "config": {"source_label": str(args.parquet.resolve()), "source_sha256": file_hash(args.parquet),
                       "time_origin_utc_s": args.time_origin_utc_s, "max_gap_s": args.max_gap_s,
                       "max_speed_mps": args.max_speed_mps, "barometric_policy": args.barometric_policy,
                       "barometric_offset_m": args.barometric_offset_m,
                       "barometric_uncertainty_m": args.barometric_uncertainty_m,
                       "position_model": "piecewise linear; preserves source derivative arrays as unused metadata"},
            "coordinate_frame": "WGS84 ECEF metres",
            "quality_notes": [
                "Source-informed regional derived dataset; preserves all supplied aircraft types without a new type or known-flight filter.",
                "Source altitude_basis is preserved per sample; source ellipsoidal_interpolated values are estimates, not independent geometric observations.",
                "Supplied ENU derivative fields are retained but NOT used by the current Rust linear backend. Legacy source may contain missing-heading derivative errors.",
                "Splits preserve missing/invalid geometry, large gaps and excessive position-chord speeds. Rejected scope remains incomplete.",
                "Chunks may contain multiple physical passes; current global CPA selection requires checking this limitation.",
                "Original data provenance: https://github.com/1337isnot1337/DOPPLERGEIST/blob/main/docs/DATA_SOURCES.md"],
            "stats": stats, "issues": issues, "passes": passes}


def observer_grid(args):
    south, west, north, east = args.bounds
    if not (-85 <= south < north <= 85 and -180 <= west < east <= 180):
        raise ValueError("Use ordered local bounds south west north east, within +/-85 degrees latitude")
    if north - south > 5 or east - west > 5:
        raise ValueError("Split geographic extents larger than five degrees")
    latitude = math.radians((south + north) / 2)
    e2 = 6.6943799901413165e-3
    denominator = 1 - e2 * math.sin(latitude) ** 2
    lat_m = 6378137 * (1 - e2) / denominator ** 1.5 * math.pi / 180
    lon_m = 6378137 / math.sqrt(denominator) * math.cos(latitude) * math.pi / 180
    rows = math.ceil((north - south) * lat_m / args.grid_spacing_m)
    cols = math.ceil((east - west) * lon_m / args.grid_spacing_m)
    if (rows + 1) * (cols + 1) > 100000:
        raise ValueError("Grid exceeds the 100,000-observer local workbench cap")
    dem, geoid = GeographicRaster(args.dem), GeographicRaster(args.geoid)
    observers, heights = [], []
    for row, lat in enumerate(np.linspace(south, north, rows + 1)):
        for col, lon in enumerate(np.linspace(west, east, cols + 1)):
            ground = dem.sample(float(lat), float(lon))
            undulation = geoid.sample(float(lat), float(lon))
            ellipsoid = ground + undulation + args.microphone_height_m
            observers.append({"id": f"r{row}-c{col}", "latitude_deg": float(lat), "longitude_deg": float(lon), "ellipsoid_height_m": ellipsoid})
            heights.append({"id": f"r{row}-c{col}", "dem_orthometric_height_m": ground, "geoid_undulation_m": undulation, "microphone_height_m": args.microphone_height_m})
    provenance = {"observer_count": len(observers), "bounds_south_west_north_east": args.bounds,
                  "maximum_grid_spacing_m": args.grid_spacing_m, "microphone_height_m": args.microphone_height_m,
                  "height_formula": "ellipsoid = source DEM orthometric height + EGM96 geoid undulation + assumed microphone height",
                  "limitations": ["Copernicus DSM includes surface objects; it is not surveyed bare earth.", "DEM and EGM96 geoid model differences remain an uncertainty; microphone height is an explicit assumption."],
                  "dem": dem.provenance(), "geoid": geoid.provenance(), "samples": heights}
    return observers, provenance


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--parquet", type=Path, required=True)
    p.add_argument("--out-dir", type=Path, required=True)
    p.add_argument("--time-origin-utc-s", type=float, required=True)
    p.add_argument("--max-gap-s", type=float, default=30.0)
    p.add_argument("--max-speed-mps", type=float, default=330.0)
    p.add_argument("--barometric-policy", choices=["reject", "approximate"], required=True)
    p.add_argument("--barometric-offset-m", type=float, default=0.0)
    p.add_argument("--barometric-uncertainty-m", type=float, default=150.0)
    p.add_argument("--bounds", type=float, nargs=4, required=True, metavar=("SOUTH", "WEST", "NORTH", "EAST"))
    p.add_argument("--grid-spacing-m", type=float, required=True)
    p.add_argument("--microphone-height-m", type=float, required=True)
    p.add_argument("--dem", type=Path, required=True)
    p.add_argument("--geoid", type=Path, required=True)
    args = p.parse_args()
    numbers = [args.time_origin_utc_s, args.max_gap_s, args.max_speed_mps, args.grid_spacing_m,
               args.microphone_height_m, args.barometric_offset_m, args.barometric_uncertainty_m, *args.bounds]
    if not all(math.isfinite(x) for x in numbers) or min(args.max_gap_s, args.max_speed_mps, args.grid_spacing_m, args.barometric_uncertainty_m) <= 0:
        p.error("Inputs must be finite; gaps, speed, grid spacing and uncertainty must be positive")
    if not 0 <= args.time_origin_utc_s < 1e11 or args.microphone_height_m < 0:
        p.error("Use Unix seconds and a nonnegative microphone height")
    targets = [args.out_dir / name for name in ["normalized.json", "observers.json", "observers-provenance.json", "import-summary.json"]]
    if any(path.exists() for path in targets):
        p.error("Use a fresh output directory; result files are never overwritten")
    # Validate all terrain coverage before committing the normalized output.
    observers, provenance = observer_grid(args)
    table = pd.read_parquet(args.parquet)
    report = normalize(args, table)
    write_json(targets[0], report)
    write_json(targets[1], observers)
    write_json(targets[2], provenance)
    summary = {"stats": report["stats"], "config": report["config"],
               "observer_count": len(observers), "output_files": [str(x.resolve()) for x in targets[:3]],
               "script_sha256": file_hash(Path(__file__))}
    write_json(targets[3], summary)
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
