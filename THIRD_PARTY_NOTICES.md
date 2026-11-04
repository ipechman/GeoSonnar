# Third-party notices

GeoSonnar's own code is distributed under GNU GPL version 3 or, at your option,
any later version (`GPL-3.0-or-later`); the complete license is in `LICENSE`.
This does not replace the notices or license terms of the third-party code,
assets and datasets described below. Preserve their notices with distributions.

GeoSonnar's engineering decision rules, validation cases and codec audit adapt
parts of [DOPPLERGEIST](https://github.com/1337isnot1337/DOPPLERGEIST) at revision
`70626ad44fb9c7f4e2f5b225956c1f9281f91c13`, copyright (c) 2026 Cinna Davis, under
the MIT License. The full notice is preserved in
`benchmarks/upstream/70626ad44fb9c7f4e2f5b225956c1f9281f91c13/LICENSE` and
`analysis/licenses/DOPPLERGEIST-MIT.txt`. Distributions containing the adapted
executable or audit must include that notice.

The adapted areas are `src/decision.rs`, `tests/decision_validation.rs`,
`src/discovery.rs`, `src/discovery_groups.rs`, their discovery validation cases,
the codec timing controls in `analysis/benchmark_audio.py`, the pinned control
signal generator adapted in `analysis/benchmark_discovery.py`, and the frozen
upstream benchmark fixtures. Other validation tests implement the corresponding physical
or data-handling contracts against GeoSonnar's own backend. The benchmark catalog
documents differences and unsupported features. This is not execution of the
entire upstream Python test suite.

Reference flight and terrain data have separate licenses. Preserve the notices
in `data/dopplergeist/provenance/`, the flight archive's ODbL/CC0 notices, and each
import receipt. The data archive revision is distinct from the validation-code
revision above. Source recordings are external inputs and are not redistributed
with the portable executable. Rust dependency licenses are identified by
`Cargo.lock` and the respective packages.

The 40-recording acoustic corpus is credited to Praveen Pathak under CC BY 4.0:
https://zenodo.org/records/23424278 . Source attribution, per-file hashes and
publisher metadata are preserved in `data/research/zenodo-23424278/`. Evaluation
creates metadata-stripped MP3 derivatives solely for the recorded benchmark;
the original WAVs and author attribution are preserved separately. The small
executable distribution does not bundle those recordings.

Native ADSB.lol ingestion reads the provider's daily readsb archives under the
provider's ODbL 1.0 terms. Acquisition plans preserve release metadata and asset
digests; normalized derivatives retain source attribution. The executable does
not bundle the global archives. See https://www.adsb.lol/docs/open-data/historical .
Native decompression, TAR, hashing and GeoTIFF support use the locked flate2,
tar, sha2 and tiff crates and their transitive dependencies; retain their
respective package licenses when distributing the program.

Current upstream licensing (reviewed November 3, 2026) does not replace the
pinned-source notices above. DOPPLERGEIST revision
8a8e85bb19e0215089608c22d43733930d9dc261 uses PolyForm Noncommercial 1.0.0 and
documents the earlier MIT grants separately. No code from that newer revision
was incorporated during the comparison. franken_doppler_finder revision
065a548ddd7ec015a53d3178ff1efbc2a9d36d32 has an MIT-labelled license with an
OpenAI/Anthropic rider; it is a comparison reference, not a vendored dependency.
See docs/UPSTREAM_COMPARISON.md for pinned sources and the scope of the review.

The public FMI/NORDLIS lightning fixture under tests/fixtures/fmi/ is attributed
to the Finnish Meteorological Institute and NORDLIS under CC BY 4.0. Its NOTICE.txt
preserves the request URL, retrieval date, original hash and license link:
https://en.ilmatieteenlaitos.fi/open-data-licence . Native FMI imports retain this
source attribution and the original response identities. The fixture is a parser
control, not an independently known microphone-location recording. XML parsing
uses the locked quick-xml crate; its MIT notice is included in packaged licenses.
