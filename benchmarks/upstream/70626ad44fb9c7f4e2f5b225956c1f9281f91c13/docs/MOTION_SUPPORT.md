# Trajectory motion support

The emission-time model assumes subsonic three-dimensional source motion. A
positive observer-specific arrival derivative does not establish this assumption:
an inconsistent native position interval can pass that radial check and fit an
acoustic ridge exactly from a side observer. No interpolation or acoustic fit
repairs such a source inconsistency.

`Track.validate_motion_support(c)` checks each native Hermite interval before CPA
proposal generation. A quadratic Bezier velocity bound cheaply certifies ordinary
intervals. When that bound reaches sound speed, endpoints and real stationary
points of squared speed determine the continuous maximum. Results are cached per
interval and rechecked against each requested sound speed. This uses floating
point polynomial roots, not formally certified interval arithmetic; a relative
1e-10 margin treats the sonic boundary as unsupported. Work and storage scale
linearly with the number of native intervals, with at most one cubic root solve
per suspicious interval over the Track's lifetime.

Both sparse and dense `search.cpa_table` paths, and `Track.closest_approaches`,
invoke the whole-source validator. An unsupported interval raises
`AcousticSupportError(track_id, "non_subsonic_motion")`. Its additive
`emission_intervals_s` field retains the affected native time intervals. CPA has
no narrower receive envelope: `receive_intervals_s` is `None`. The outer search
may preserve other track rows while recording the whole affected track/tile
scope as lost model coverage. It must keep the search partial and finite evidence
incomplete. Calling the CPA function directly still propagates the typed failure;
it does not silently drop the source.

Direct `Track.acoustic` scoring checks only possible arrivals from unsupported
native intervals. Cubic Bezier position boxes bound minimum/maximum range to the
observer; adding those travel-time bounds to the native interval encloses all
possible arrival times without assuming monotonic motion there. An overlapping
requested receive sample raises the same typed failure with both emission and
conservative receive intervals. A separate, clean event outside these envelopes
can retain its original emission-time solution even if the trace is inconsistent
elsewhere. An entire native interval is conservatively unsupported, including
its slower portions; this guard does not split or repair it. Existing
`non_monotone_arrival` failures and messages are preserved.

The finite Newton iteration must also finish with an emission-time correction
within 1e-7 s or four receive-timestamp ULPs, whichever is larger. A finite last
iterate is insufficient: a subsonic straight near-sonic control previously
returned emission times about 293 s from the analytic solution after ten
iterations. An unfinished solve now raises `emission_solver_not_converged` and
remains unknown. Samples outside the declared trajectory support retain NaNs;
they do not require an extrapolated converged root.

The robust clock-trial adapter records these failures as unknown, never as an
acoustic rejection or an evaluated fit. A perfect ridge match cannot establish
physical support. The guard does not change source observations, velocity
inference, interpolation, altitude labels, gap rules, clock grids or acceptance
thresholds. It does not establish source correctness, independent real accuracy
or complete national coverage. The fresh-raw audit and preserved historical
failures remain in [research/trajectory-motion](../research/trajectory-motion/README.md).

Independent controls cover a 750 m/s interior peak hidden by zero measured knot
velocities and monotone side-view arrival times, its incorrectly perfect old
acoustic fit, a 300 m/s control, an unaffected separate event, actual sparse/dense
CPA failures, sound-speed sensitivity, conservative arrival envelopes and a dense
continuous-speed oracle. W2's outer isolation must additionally verify an actual
bad-source plus clean-source search and all-invalid abstention.
