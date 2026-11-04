# Experimental atmospheric profiles

GeoSonnar can optionally use a local altitude profile for propagation instead of one constant sound speed. The first implementation integrates temperature, pressure, humidity and horizontal wind along a **straight ECEF source-to-receiver path**. It uses that same travel-time model for lightning arrivals, aircraft closest-approach arrivals and aircraft Doppler frequency predictions. It does not establish improved location accuracy.

The constant-speed mode remains the default. A profile is an explicit assumption stored with the review and score configuration. Selecting a profile must not silently change its coverage, extend it to unsupported heights/times, or fall back to the constant model when an atmospheric calculation fails. The atmospheric comparison is intended for bounded local refinement, with tighter work limits than the constant-speed search.

## Implemented model

The native module is `src/atmosphere.rs`. It provides `AtmosphereProfile::validate`, `load_profile`, `PreparedAtmosphere`, and `PreparedAtmosphericTrack`.

For each sampled position along the path, the module converts ECEF coordinates to WGS84 latitude, longitude and ellipsoid height. Temperature, humidity and wind components interpolate linearly between neighboring profile heights; pressure interpolates logarithmically. It rejects missing height coverage. Geographic coverage checks include both endpoints and interior latitude extrema, found from the local-north path derivative after explicitly splitting any equator crossing. Longitude is monotone on the supported short, non-wrapping chord. A narrow high-latitude excursion cannot hide between quadrature nodes. Both emission and arrival must fall inside the declared UTC interval.

Water vapour pressure is calculated from relative humidity over liquid water and the [Murphy and Koop (2005) liquid-water saturation formula](https://doi.org/10.1256/qj.04.94), equation 10. The implementation uses the expression reproduced in the original paper and the [published equations in this atmospheric study](https://acp.copernicus.org/articles/22/65/2022/acp-22-65-2022.html#section8). Pressure is in pascals and temperature in kelvin.

The vapour mole fraction is `x = RH × e_sat(T) / p`. For an ideal dry-air/water-vapour mixture:

```text
M = (1 − x) M_dry + x M_water
Cp = (1 − x) Cp_dry + x Cp_water
gamma = Cp / (Cp − R)
c = sqrt(gamma R T / M)
```

The constants are `R = 8.31446261815324 J/(mol K)`, `M_dry = 0.02896546 kg/mol`, `M_water = 0.01801528 kg/mol`, `Cp_dry = 29.12 J/(mol K)` and `Cp_water = 33.58 J/(mol K)`. Constant heat capacities and an ideal mixture are deliberate approximations. Pressure influences humidity's mole fraction; dry ideal-gas sound speed at fixed temperature does not depend on pressure. This follows the basic ideal-gas relationship described by [NPL](https://www.npl.co.uk/getattachment/19d0d4d3-5bd9-47d9-b392-01be4c8e453d/Speed-of-Sound-Toilet-Roll-Report.pdf?lang=en-GB). It is not the high-accuracy real-gas model, whose additional thermodynamic and frequency effects are described by [Gavioso et al.](https://doi.org/10.1063/5.0294663).

Wind is expressed as eastward and northward velocity, converted to local ECEF axes at each sample. The model adds the wind component along the source-to-receiver path to the local sound speed:

```text
c_effective = c + wind · path_direction
travel_time = integral(ds / c_effective)
```

This is the effective-sound-speed approximation discussed by [Wilson, Pettit and Ostashev](https://acousticstoday.org/wp-content/uploads/2015/06/Sound-Propagation-in-the-Atmospheric-Boundary-Layer-D.-Keith-Wilson-Chris-L.-Pettit-and-Vladimir-E.-Ostashev.pdf). A tailwind decreases delay and a headwind increases it. The projected-wind approximation omits second-order crosswind effects. The numerical path remains straight: temperature and wind shear can physically refract sound, but this implementation does not solve the refracted trajectory.

Each path is split at its minimum ellipsoid height and every profile-height crossing before integration. This prevents a thin listed layer from hiding between uniform quadrature nodes and explicitly checks the minimum-height coverage. Composite Simpson integration starts with at least eight even path segments in total and a declared maximum step. It doubles resolution until the sum of the absolute per-piece successive-grid estimates `abs(t_fine − t_coarse) / 15` meets the tolerance, or returns a convergence/budget error. Errors in different layers cannot cancel in that estimate. The reported error is still a numerical estimate, not a weather uncertainty, acoustic model error bound or confidence interval; unrepresented real atmospheric layers are outside the supplied model.

For aircraft, the observed arrival time is inverted inside the original continuous linear track segment. The solver uses segment-relative seconds to avoid losing precision from Unix-epoch arithmetic. It checks increasing endpoint and midpoint arrivals, verifies the final arrival residual, and calculates:

```text
frequency_observed / frequency_emitted = 1 / (1 + d(travel_time)/d(emission_time))
```

The derivative uses the same path integral at two numerical step sizes. It uses one-sided differences at track boundaries and never reads a position outside the track segment. Endpoint arrivals may be snapped to the supported endpoint within bounded floating-point roundoff, rather than extrapolating a position. Numerically unresolved derivatives, near-sonic behavior, excessive gaps and unsupported coverage are errors. The subsonic check is conservative: the source must be slower than the minimum dry sound speed minus maximum horizontal wind magnitude across the complete profile.

A second safeguard bounds the travel-time derivative using the worst supported sound-speed/wind gradient, path length and wind magnitude. With `c_min` the conservative minimum effective speed, `G` the spatial speed-gradient bound, `U_max` the maximum wind and `v` the source speed, it requires `v × [1/c_min + (L × G/2 + U_max)/c_min²] < 0.95`. This is a sufficient monotonicity condition for the declared straight-path model, not a physical condition for every real atmosphere. Sharp profiles that fail it are explicitly unavailable for moving-source inversion; the solver does not select an arbitrary root. A path that would be usable with a more sophisticated local ray model can therefore be rejected. Closest approach remains the geometric minimum range; only its predicted arrival is propagated through the profile.

## Profile interchange and provenance

`examples/synthetic-atmosphere-profile.json` is a **fabricated schema example**, not downloaded weather and not a recommended atmosphere for any recording. The loader accepts UTF-8 JSON up to 4 MiB, rejects unknown fields, and validates the profile before use.

Required content includes:

- A name, provider/source description, source-input SHA-256 and notes. These are declared provenance; the hash does not authenticate a provider or establish a forecast's accuracy. The entire selected profile remains part of the review/scoring evidence identity.
- `valid_from_utc_s` and `valid_to_utc_s`, in Unix UTC seconds. The profile is frozen over this explicit applicability interval, at most 24 hours; there is no interpolation between weather times. The user/importer declares the interval, rather than the program inferring that a single observation is valid for a whole day.
- `bounds` in south, west, north, east order. This version supports a non-wrapping local box at most five degrees per side, within ±85° latitude. One vertical column is treated as horizontally homogeneous inside that box; it is not a two-dimensional temperature map.
- `height_datum: "wgs84_ellipsoid"` and `humidity_reference: "liquid_water"`. Other references are rejected, not guessed.
- Two to 256 strictly ascending height levels separated by at least 1 m, each with ellipsoid height in metres, temperature in kelvin, pressure in pascals, relative humidity as a fraction from 0 to 1, and eastward/northward wind in m/s. These wind components describe where the air moves, not the meteorological direction it comes from.

Supported numerical bounds are −1000 to 20000 m ellipsoid height, 200 to 330 K, 5000 to 110000 Pa, horizontal wind magnitude up to 100 m/s, and water vapour no more than 10 mole percent. These are input guards, not accuracy guarantees across the full range. There is no extrapolation, implicit sorting, height-datum conversion, humidity clamping or pressure-unit conversion. Paths must be between 0.01 and 100000 m long. Default integration uses a maximum initial step of 250 m, a `1e-7 s` numerical tolerance and at most 4096 segments. Missing coverage remains unavailable evidence.

## Obtaining actual weather profiles

The first native implementation imports a normalized JSON profile. It does **not** download atmospheric datasets, decode GRIB/NetCDF, or convert geopotential/terrain height datums automatically.

[ERA5 pressure-level data](https://cds.climate.copernicus.eu/datasets/reanalysis-era5-pressure-levels?tab=overview) is a possible future source for temperature, pressure levels, wind, humidity and geopotential. Its [official documentation](https://confluence.ecmwf.int/pages/viewpage.action?pageId=185075432) must govern normalization: temperature is K; pressure-level identifiers commonly use hPa; `u` and `v` are eastward and northward m/s; geopotential is m²/s², not WGS84 ellipsoid height. Dividing geopotential by standard gravity yields geopotential height, which still requires an explicit conversion to the vertical reference used by the microphone, aircraft and lightning data. Do not paste it into `ellipsoid_height_m`.

ERA5 relative humidity can use a mixed water/ice convention. This schema requires RH relative to liquid water at every level. Prefer a documented conversion from specific humidity and pressure rather than silently treating mixed-phase RH as liquid-water RH. A future importer must preserve original bytes and retrieval parameters, reconcile the height datum and humidity convention, record interpolation/coverage, and require an explicit user request for acquisition. A modeled atmosphere is not a direct measurement along the acoustic path.

## Scope and next increments

Implemented: bounded profile import/validation, vertical interpolation, humid ideal-air sound speed, projected horizontal wind, path-integrated travel times, travel-time-based Doppler, numerical convergence diagnostics, gap/coverage handling and traceable assumptions.

Not implemented: a weather map downloader or horizontal/time interpolation, refracted/eigenray search, multiple arrivals, caustics/shadow zones, turbulence, frequency-dependent molecular absorption, rain scattering, lightning-channel acoustic source extent, moving receiver, terrain occlusion, ground reflection or ground impedance. A straight ECEF chord can intersect terrain; profile height coverage alone does not prove an unobstructed air path. Profiles must not be sold as a correction for these omissions.

The next atmospheric model increment should test refraction against independent analytic/ray benchmarks and known meteorological profiles before applying it to searches. Fine-scale weather changes must be evaluated on fresh held-out data. A lower residual after adding atmospheric freedom is not, by itself, a better location.

Run the headless numerical contracts with:

```powershell
cargo test --offline --locked --test atmosphere_validation
```

The tests compare uniform propagation and Doppler against analytic formulas, vertical linear-temperature paths and a thin thermal anomaly against closed-form integrals, published humid-air reference values with an explicitly approximate tolerance, wind directions, profile interpolation, tighter quadrature, sharp-gradient rejection, CPA/track coverage, height/time/region guards and invalid-unit rejection. Geographic controls include northern and southern interior latitude extrema that fall between ordinary quadrature nodes, plus an equator-crossing path. They establish software/model behavior, not real-world atmospheric accuracy.
