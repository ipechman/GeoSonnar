# Independent MP3 and product verification

This lane supports DG-12 / issue #13. It complements the independent regional
replay lane and does not replace held-out real national localization evidence.

Run the codec measurement from an installed checkout with ffmpeg/libmp3lame:

```bash
.venv/bin/python -m pytest -q tests/test_methodology_verifier.py
.venv/bin/python scripts/verify_methodology.py codec --output results/mp3-timing-new
```

The output directory must be new. The runner generates a deterministic stereo
signal with three nonperiodic timing pilots, encodes five MP3 variants, and decodes
the WAV and MP3s independently to mono 4 kHz. It measures each pilot by normalized
cross-correlation. Silence, unrelated signals, truncated input and offsets beyond
the measurement range cannot pass. Original/MP3/code hashes, versions, exact
commands, stderr and decoded arrays remain in that directory. `report.json`
records all cases; the command fails if a required encoding fails.

The four gapless encodings must retain pilot timing and decoded duration within
2 ms. The fifth deliberately omits the Xing header and is a diagnostic: an MP3
without gapless metadata need not identify the original recorder's sample zero.
A failed diagnostic measurement still fails the command. A measured delay on this
test signal is not a universal correction that can be applied to other files.

## Local codec measurement, 2026-11-02

Platform: Linux x86-64, CPython 3.12.3, ffmpeg 6.1.1 with libmp3lame. The environment
was created with `uv venv --python 3.12 .venv` and installed with
`uv pip install --python .venv/bin/python -e . pytest`.

| Encoding | Maximum pilot offset | Duration difference | Outcome |
| --- | ---: | ---: | --- |
| Mono 8 kHz / CBR 16 kbps | 0.000966 ms | 0 ms | Pass |
| Mono 16 kHz / CBR 32 kbps | 0.000234 ms | 0 ms | Pass |
| Stereo 44.1 kHz / CBR 96 kbps | 0.000055 ms | 0.75 ms | Pass |
| Stereo 48 kHz / VBR q4 | 0.000014 ms | 0 ms | Pass |
| Mono 16 kHz / CBR 32 kbps, no Xing header | 69.051 ms | 93 ms | Diagnostic |

Subsample offsets are interpolation estimates, not claims of clock precision
beyond the 0.25 ms decoded sample period. Five verifier tests cover planted delays
and rejection controls. The original 11 baseline tests passed in this clean
environment. The checked-in codec receipt is
`evidence/verification/mp3-timing-20261102.json`; full local output is
`results/mp3-timing-20261102/`.

## Automatic workflow acceptance

The actual CLI/API/upload acceptance depends on the producer implementations in
issues #4–#11. It must invoke real intake, discovery, acquisition, association,
search and decision code; inspectable output alone does not establish a successful
location. An undated file must retain `needs_input`; unsupported audio/coverage
must retain failed/inconclusive distinctions. Manual event windows/bands, true
flight IDs and truth-derived bounds are not normal automatic inputs.

No automatic location or national validation is established by the codec results
above. Final acceptance remains open until the integrated upload path is executed
and the real benchmark outcomes, time/location uncertainty and searched coverage
are reported.
