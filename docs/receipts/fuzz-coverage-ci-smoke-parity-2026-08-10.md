# Fuzz coverage — PR-time smoke parity for the five new targets

Date: 2026-08-10

Scope: closes the CI-asymmetry gap named in the original fuzz-coverage
audit and repeated in every tranche receipt since: only 4 of the (now 36)
fuzz targets got a PR-time live-fuzz smoke pass in
`production-gates.yml`'s `fuzz-regressions` job (`v3_route_admission`,
`legal_move_enumeration`, `preview_compilation`, `evidence_fusion`) — every
other target relied on nightly-only live fuzzing plus PR-time regression-
corpus replay, which only catches previously-found crashes until the next
nightly run. Adam asked for this to be picked up alongside the wiremock
response-decode test.

## What changed

`.github/workflows/production-gates.yml`'s `fuzz-regressions` job gained
four new steps, immediately after the existing "Semantic Gameboard Phase 3
evidence smoke" step, matching that job's exact established pattern
(`mktemp -d` corpus seeded from a real committed seed file, `cd` into the
target's own fuzz workspace, `cargo fuzz run <target> <corpus> --
-runs=64 -max_len=N -print_final_stats=1`):

- **Designer operation-apply smoke** — `designer_operation_apply`
  (`bpmn-lite-server-designer/fuzz`), seeded from
  `admitted-linear.json`, `-max_len=1024`.
- **dmn-lite-parser smoke** — `dmn_lite_parse` (`dmn-lite-parser/fuzz`),
  seeded from all four `.dmn-lite` fixtures, `-max_len=2048` (this is the
  target that found the real UTF-8 char-boundary bug earlier this session —
  now gets the same PR-time attention as the gameboard targets, not just
  nightly).
- **bpmn-lite-authoring smoke** — both `yaml_workflow_parse`
  (`-max_len=512`) and `zeebe_bpmn_import` (`-max_len=2048`), same fuzz
  workspace, seeded from their respective committed seeds.
- **FFI owner_metadata_decode smoke** — both the HTTP and gRPC crates'
  identically-named `owner_metadata_decode` target, each in its own fuzz
  workspace (`bpmn-lite-ffi-http/fuzz`, `bpmn-lite-ffi-grpc/fuzz`), each
  `-max_len=256`.

`-max_len` per step set to roughly 1.5-2x the largest real seed file for
that target, matching the sizing logic implicit in the pre-existing steps
(e.g. `v3_route_admission`'s 1024 against its seed, `evidence_fusion`'s 256).

## Verification

- Ran all six `cargo fuzz run ... -- -runs=64 -max_len=N
  -print_final_stats=1` commands locally, byte-for-byte identical to what
  the new workflow steps invoke (same corpus construction, same working
  directory, same flags) — all six completed `Done 64 runs`, 0 crashes.
- `python3 -c "import yaml; yaml.safe_load(...)"`: the edited workflow file
  parses cleanly; `jobs` unchanged in name/count
  (`rust-and-recovery`, `native-wasm-replay`, `fuzz-regressions`).
- `git diff --stat`: touches only
  `.github/workflows/production-gates.yml` (+43 lines, pure addition, no
  existing step altered).

## What this does not do

- Does not touch the underlying `-runs=64` smoke depth itself for any
  target, new or old — 64 runs from a real seed is a "did we just break
  something obvious" check, not a substitute for the nightly 20-minute
  live-fuzz run or the full regression-corpus replay (`cargo run -p xtask
  -- fuzz regress`), both of which still run in this same job/workflow set
  for every target, new and old alike.
- Does not add PR-time smoke for every one of the 36 targets — only the
  five closed this session. The remaining ~27 pre-existing targets keep
  their prior nightly-only-live-fuzz-plus-regression-replay posture,
  unchanged; extending PR-time smoke to all of them would be a separate,
  larger decision (job runtime budget) not made here.
