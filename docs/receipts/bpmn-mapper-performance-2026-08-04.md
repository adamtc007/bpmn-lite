# BPMN mapper performance receipt

**Date:** 4 August 2026
**Build:** Rust 1.95, `--release`
**Host:** Apple arm64, macOS
**Samples per native measurement:** 2,000
**Legal mid-sequence board size:** 15

| measurement | board size | p50 µs | p95 µs |
|---|---:|---:|---:|
| semantic board construction | 4 | 219.416 | 233.416 |
| semantic board construction | 8 | 243.584 | 255.500 |
| semantic board construction | 12 | 265.584 | 279.459 |
| governed exact lane | 15 | 10.125 | 10.459 |
| serialize all semantic candidates | 15 | 11.208 | 11.500 |
| serialize all bounded candidate pairs | 15 | 70.042 | 73.459 |

The reproducible harness is
`utterance-engine/examples/semantic_perf_receipt.rs`. These measurements are a
receipt, not a regression threshold: runner and baseline ratification remain an
owner decision.

## Explicitly unavailable

- Candle batch latency for 4/8/12/20/26, cold bundle load and warm inference:
  there is no admitted v3 bundle; Phase 6 remains externally blocked.
- Full-board versus K=12 accuracy: there is no admitted v3 evaluation evidence.
- Board sizes 20/26: no reviewed position exposes that many legal actions. The
  harness does not fabricate a synthetic authority board to make the table fit.
- Peak RSS/per-request allocation: no allocator or portable process-memory
  probe is installed. Fuzz RSS is a separate sanitizer measurement.

The unavailable cells are deliberately visible. Existing incompatible v2
weights were not relabelled or used for performance claims.
