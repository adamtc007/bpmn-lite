# Critical fuzz-gate remediation receipt

**Date:** 4 August 2026  
**Review source:** `bpmn_fuzz_testing_review.md`  
**Scope:** FT-01, FT-02 and the corpus/artifact portion of FT-07

## Reconciliation correction

The source review says `cargo xtask fuzz list` discovered 16 targets, but its
target table contains 15 and current manifest discovery also returns 15:

- compiler: 1;
- engine: 5;
- kernel: 4;
- server boundary: 1;
- types/artifacts: 4.

The pre-remediation nightly declaration was therefore 300 target-minutes, not
320. It was still impossible inside the single 180-minute job, and later
targets were still systematically deprived of execution time. The remediation
does not encode either count: target manifests are now the matrix source of
truth.

## FT-01 / FT-07 remediation

- `cargo run -p xtask -- fuzz list --json` emits the discovered target matrix,
  including crate, target and fuzz-project directory.
- Nightly runs one target per job with a 20-minute fuzz budget inside a
  60-minute job timeout and `fail-fast: false`.
- Every target has its own evolved-corpus cache and crash-artifact upload, so
  compiler and server-runner persistence is no longer omitted.
- A successful runner writes `completed-targets.txt` only after receipt writing
  and crash evaluation.
- Cargo-fuzz 0.13 has no `--locked` flag, so the runner performs a Cargo
  `metadata --locked` preflight for every selected fuzz project before invoking
  cargo-fuzz. Stale independent fuzz lockfiles now fail before execution.
- The final `complete` job runs under `always()`, downloads target receipts and
  compares completion markers with the exact discovery matrix. Missing,
  cancelled, failed or unexpected target receipts fail the workflow.

## FT-02 remediation

- The minimal F8-COMPILER-001 multi-instance/no-successor reproducer is now in
  `bpmn-lite-engine/fuzz/regressions/xml_compile/`.
- `fuzz-regressions.json` records its finding ID, target, fixed commit, expected
  current outcome, input SHA-256 and original evolved-artifact provenance.
- `scripts/check_fuzz_regressions.py` fails on an empty manifest, missing or
  ungoverned inputs, duplicate entries or hash drift.
- The production gate is unconditional. It validates governance, installs the
  required toolchains and executes regression replay; zero inputs can no longer
  produce a green skip.
- The runner independently rejects a regression invocation that executes zero
  inputs, so this invariant is not CI-YAML-only.

## Verification

- `python3 scripts/check_fuzz_regressions.py` — 1 governed case validated.
- Both changed workflow files parse as YAML.
- `cargo check -p xtask` — green from an isolated Cargo home.
- locked metadata resolution — green for all five fuzz projects.
- JSON discovery — 15 targets, including `dsl_compile`, `wire_decode` and
  `xml_compile`.
- `cargo run -p xtask -- fuzz regress` equivalent direct runner invocation —
  1 target-run, F8 input executed twice, 0 crashes, completion receipt written.

## Remaining fuzz assurance work

This receipt does not close the review as a whole. Stateful lease/job authority
models, PostgreSQL crash cuts, native/Wasm corpus differential execution,
resource budgets, corpus minimisation and coverage/admission telemetry remain
required under Phase 9.
