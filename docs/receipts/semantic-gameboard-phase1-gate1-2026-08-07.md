# Semantic Gameboard Phase 1 Gate 1 receipt

**Date:** 2026-08-07
**Phase:** 1 — introduce the design-position and move contracts
**Gate:** GREEN
**BPMN-Lite entry:** `a8ac2056d6d119e56da63589505a0e87e5f1393c`
**Shared release:** `bc547723e6831cdb46fb8028071db3f537129d77`

This receipt closes Phase 1 only. It does not claim that concrete position-bound move
enumeration, preview, evidence fusion, attempt persistence, correction handling,
belief/history, information-gaining dispositions or later rollout phases are complete.

## Architectural decisions

1. The reusable contracts live in the MIT-licensed, safe-Rust
   `semantic-decision-contracts` capability crate. The implementation module is
   private and the root exports named facade items only.
2. `DesignPosition` closes over a domain and board path, admitted semantic snapshot,
   graph revision and SHA-256 content identity, compiler profile, policy identity,
   explicit focus, append-only history identity, optional current-proposal identity,
   canonical legal moves and move-set/state hashes.
3. Focus absence is explicit. The Designer response records either the resolved graph
   element or `not_provided`; it never selects a recent or first node implicitly.
4. All ten attempted-interaction outcomes are typed, including incomplete, ambiguous,
   inapplicable, stale, disclosure-safe, compiler-refused, rejected, corrected and
   technical-failure non-transitions. Correction links are validated and acyclic.
5. Rules, recovery feedback and disclosure classes are governed data contracts. No
   BPMN or `ob-poc` vocabulary entered the generic gameboard implementation.
6. Models, evidence and belief remain non-authoritative. No legality, proposal,
   ratification, compiler admission, graph mutation or runtime execution path changed.
7. The Phase 1 Designer adapter projects the same production
   `SemanticDecisionBoard`; it does not perform Phase 2 concrete binding or preview.
   The serialized position describes the pre-attempt board used for inference.
8. The existing graph revision remains the legacy BLAKE3 edit-log identity to avoid
   changing serving/proposal behavior. A separate framed SHA-256 edit-log hash supplies
   the gameboard graph-content identity. History is framed SHA-256 over the observed
   ordered session events.
9. The legacy pending-proposal cache permits multiple entries and designates no single
   current proposal. The compatibility projection records `None` rather than guessing.
10. Legacy DSL-source sessions expose `design_position: null` because they have no
    authoritative `DesignerDag`; graph-backed requests expose the validated contract.

## Shared release and exact pin

The shared release consists of two phase-scoped commits on
`refactor/sem-os-pack-policy`, both pushed and verified at the remote ref:

```text
52614364d39fba5e053117b88e4681a8b3ba4be6 feat: add semantic gameboard contracts
bc547723e6831cdb46fb8028071db3f537129d77 test: receipt gameboard fuzz semantic coverage
```

BPMN-Lite's five workspace DSL dependencies and the two standalone fuzz-project
dependencies are pinned to the exact final revision. All three lockfiles resolve the
same `bc547723` source; no old DSL revision remains in the affected closure.

## Shared contract and fuzz evidence

Shared crate verification at the final revision:

- 29 unit tests and 28 doc tests passed;
- all-target/all-feature Clippy passed with warnings denied;
- docs passed with rustdoc warnings denied;
- default and all-feature public surfaces are identical: 751 items,
  SHA-256 `a651180d2fe3b7386228b1f31715a750b311e5cf8324b68c4f7eb6744c236e81`;
- facade compile-pass, private-module import compile-fail and unchecked-constructor
  compile-fail fixtures passed;
- dependency direction is exactly `hex`, `serde`, `serde_json`, `sha2`, `thiserror`;
- the domain-neutral source scan found no BPMN or `ob-poc` vocabulary;
- all six fuzz targets were discovered and completed isolated 64-run, 2,048-byte
  local smokes;
- all ten attempt-outcome and all five disclosure-class semantic counters were
  observed and recorded;
- every PR and nightly target now emits and uploads an independent JSON receipt and
  log; missing required counters fail the job;
- the committed finite-score JSON round-trip regression replayed successfully.

Aggregate local fuzz receipt:
`docs/receipts/artifacts/semantic-gameboard-phase1-contract-fuzz-smoke.json`.

## BPMN-Lite verification

Green at the exact shared pin:

- `cargo test -p utterance-engine`: 62 unit, 4 integration and 1 doc test passed;
- `cargo test -p bpmn-lite-server-designer`: 56 tests passed;
- `cargo test -p utterance-engine --features candle-probe`: 66 passed, 2 documented
  model-loading tests ignored, integration/doc tests passed;
- `cargo test -p bpmn-lite-server-designer --features candle-probe`: 57 passed;
- `q9-capture` and `embed,candle-probe` feature checks passed;
- changed-package library Clippy passed with warnings denied;
- evaluator-versus-serving identity, board, workbook, proposal audit, stale drift,
  ratification and graph non-mutation tests remained green;
- graph-backed anchored and whole-graph requests deserialize their emitted
  `DesignPosition`; legacy requests retain a null compatibility boundary;
- `python3 scripts/check-semantic-gameboard-boundaries.py` passed API snapshots,
  approved modules, dependency direction and compile-pass/fail fixtures;
- `python3 scripts/check_fuzz_regressions.py` validated the existing governed BPMN
  regression;
- discovery still reports exactly 20 BPMN-Lite fuzz targets.

Known unchanged baseline conditions:

- `q9-capture` checking reports the existing private dead-code warning in
  `capture.rs`;
- the broad workspace formatter reports pre-existing drift in unrelated or unchanged
  portions of the repository. The new Rust hunks were checked against rustfmt without
  formatting those unrelated files;
- the broader compiler/all-target Clippy warnings recorded by Gate 0 remain outside
  this phase.

## Public API and dependency review

The shared release adds 496 reviewed facade items and removes none. Its default,
test/fuzz and all-feature production surfaces are identical.

BPMN-Lite adds exactly two `utterance-engine::bpmn_board` facade items:

- `BpmnBoardError::Gameboard`;
- `project_design_position`.

The real external consumer is `bpmn-lite-server-designer`, the owning facade is
`utterance-engine::bpmn_board`, and the stability contract is the v1 compatibility
projection from an admitted `SemanticDecisionBoard`. The server's Rust public surface
remains 8 items under every checked feature. No visibility was widened for a test,
fuzzer, example, benchmark or `xtask`. No capability crate acquired an application,
server, fuzzer or orchestration dependency. The server adds only `sha2` for its
application-owned graph/history compatibility identities.

## Phase 1 file ledger

BPMN-Lite phase files:

```text
Cargo.lock
Cargo.toml
bpmn-lite-server-designer/Cargo.toml
bpmn-lite-server-designer/fuzz/Cargo.lock
bpmn-lite-server-designer/fuzz/Cargo.toml
bpmn-lite-server-designer/src/rest.rs
docs/receipts/artifacts/semantic-gameboard-phase1-contract-fuzz-smoke.json
docs/receipts/semantic-gameboard-phase1-gate1-2026-08-07.md
docs/receipts/semantic-gameboard-phase1-red-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/fixtures/gameboard_api/facade_consumer.rs
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/src/bpmn_board.rs
```

The shared release file ledgers are the exact file lists in commits `52614364` and
`bc547723`. Generated model, corpus and training artifacts were not changed by Phase 1.

## Untouched concurrent work

The pre-existing `.DS_Store` files, runner import-order change, corpus/bundle outputs,
deleted split manifest, untracked normative documents, untracked v3 corpora and
training logs remain outside the Phase 1 commit. None is staged, regenerated or
discarded.

## Gate decision and next entry

Gate 1 is green. Phase 2 may begin only from this exact pin with the existing compiler
and `PositionalLegality` authority available. Its entry work is deterministic concrete
legal-move enumeration and non-mutating compiler preview; any disagreement between
enumeration and compiler legality is a stop condition.
