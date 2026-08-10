# Semantic gameboard Phase 7 — bounded libFuzzer smoke

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Closes red-receipt item 8's outstanding clause (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
"A bounded libFuzzer smoke remains host-blocked because this environment has no
nightly sanitizer-capable Rust toolchain."

## Correction

That clause was written 2026-08-07. Earlier in this session I repeated it as still true
without re-checking, and the user ruled "green with documented exception" for the Phase
7 gate on that premise. **The premise was wrong as of today** — this environment now has
a `nightly-2026-08-03` toolchain and `cargo-fuzz 0.13.2` installed. Verified directly
rather than assumed further:

```
$ cargo +nightly fuzz build
   ...
    Finished `release` profile [optimized + debuginfo] target(s) in 37.89s

$ cargo +nightly fuzz run bpmn_binding_extract -- -max_total_time=30 -runs=200000
   ...
#37690 DONE   cov: 1435 ft: 1742 corp: 224/2810b lim: 68 exec/s: 1215 rss: 514Mb
Done 37690 runs in 31 second(s)
```

37,690 executions, 0 crashes, 0 hangs, 0 OOMs (`fuzz/artifacts/bpmn_binding_extract/` was
never created). Coverage climbed steadily (1432→1435 edges) with no plateau-then-crash
pattern. `bpmn_binding_extract` is the only fuzz target in
`bpmn-lite-server-designer/fuzz/fuzz_targets/`, matching red-receipt item 8's own
description of "the designer fuzz target."

This closes item 8 for real — not as a documented exception. The Phase 7 gate receipt
reflects an unconditional green here, not the exception the earlier (incorrect) framing
would have required.

## Results

- `cargo +nightly fuzz build`: clean.
- `cargo +nightly fuzz run bpmn_binding_extract -- -max_total_time=30 -runs=200000`: 37,690
  runs, 0 crashes.
