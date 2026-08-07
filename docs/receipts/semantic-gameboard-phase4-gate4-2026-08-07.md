# Semantic Gameboard Phase 4 Gate 4 Receipt — 2026-08-07

## Gate status

**GREEN.** Phase 4 adds bounded append-only attempt history, governed motifs and
non-authoritative belief state without changing compiler legality, preview, human
ratification or mutation authority. This receipt is Phase 4 only; later Gameboard
phases and the complete programme remain open.

No model was trained and no corpus or model artifact was regenerated.

## Shared prerequisite

The generic motif schema was implemented and published from the shared `dsl`
repository as commit
`4858d305248f80f2035891b90c4474affb66ec8a` (`feat: govern semantic gameboard
motifs`). BPMN-Lite pins every affected shared dependency and both lockfiles to that
immutable revision.

The shared extension is domain-neutral: bounded versioned motifs contain fact
patterns, completion facts, likely candidate references, contrasts and explicit
completion/abandonment conditions. Admission rejects duplicate identities, zero
versions, dangling candidates, contradictory terminal conditions and resource-limit
violations. BPMN vocabulary exists only in the BPMN YAML pack and private adapter.

## Implemented architecture

- `history.rs` validates correction graphs, bounds the decision window to 64 typed
  attempts and 64 KiB, and hashes only canonical decision-relevant receipts.
- Raw utterance transcripts and opaque proposal payloads no longer enter the position
  history hash. The application reconstructs at most the last 64 terminal receipts.
- `motifs.rs` deterministically derives bounded BPMN graph facts and evaluates the
  admitted pack's active, completed, abandoned and inactive conditions.
- `belief.rs` performs a bounded deterministic log-linear update over complete Phase 3
  move evidence, prior motif belief and negative rejection/correction evidence.
- Belief snapshots use the shared `DesignBelief` contract, are tied to the exact
  position and pack-derived producer identity, and carry no transition authority.
- The live graph-backed utterance path now supplies prior typed attempts to evidence
  fusion and persists terminal wrong attempts, belief snapshots and the exact history
  projection hash in the append-only session log.
- Terminal workbook rejection, expiry, compiler refusal and ratification are persisted
  as typed receipts in opaque proposal-audit events. Ratification still passes through
  explicit human approval, graph re-stage/admission and `GraphEdit`; no automatic apply
  route exists.
- The semantic snapshot identity now includes the complete admitted pack artifact, so
  motif drift cannot reuse a pre-motif snapshot identity.
- Legal-but-unwanted replacement and its corrective replacement are tested as two
  separately admitted graph transitions plus an acyclic retained correction link.

## Pack receipt

```text
source_sha256:   4764ffb9a402d910abd635d5c4c1a21512107c600dbf60c12aee95b362dd0d68
artifact_sha256: c3dd92720bb671729970c6ef3530b79572c6b8bdd8be2b4d548f8717e5fa0d2a
snapshot:        bpmn-semantic-profile-v1:0ff6747b192aa4df8181b17d0d922d779994a161cadc39c4d3ca36fd7568edc8
motifs:          2
```

## Focused verification

```text
cargo fmt --all -- --check
  PASS

cargo test -p utterance-engine --lib
  PASS: 77 passed

cargo test -p utterance-engine --all-features --lib
  PASS: 91 passed, 5 explicitly ignored model/network qualification tests

cargo test -p bpmn-lite-store --lib
  PASS: 37 passed

cargo test -p bpmn-lite-server-designer --lib
  PASS: 56 passed

cargo test -p bpmn-lite-server-designer --all-features --lib
  PASS: 58 passed, 1 explicitly ignored external-model test

cargo test -p bpmn-lite-store-postgres --all-features --lib
  PASS: 99 passed

cargo clippy -p utterance-engine -p bpmn-lite-server-designer \
  -p bpmn-lite-store --all-targets --no-deps -- -D warnings
  PASS

python3 scripts/check-semantic-gameboard-boundaries.py
  PASS: API, visibility, dependency direction, compile-pass and compile-fail fixtures

python3 scripts/check_fuzz_regressions.py
  PASS: 1 governed regression replayed

cargo run -p xtask -- fuzz list --json
  PASS: 24 discovered targets including history_belief_state
```

The dependency-inclusive Clippy invocation reached standing unrelated failures in
`bpmn-lite-kernel/src/lib.rs` (`collapsible_if`) and
`bpmn-lite-compiler/src/lowering.rs` (`match_like_matches_macro`,
`too_many_arguments`). Those files are outside Phase 4 and were not changed for this
programme. The changed-package `--no-deps` warnings-denied gate is green. Existing
all-feature `capture.rs` dead-code warnings remain recorded baseline warnings and are
not Phase 4 failures.

## Fuzz assurance

The new `history_belief_state` target is a public-facade consumer with an explicit
byte operation tape and compact independent history model. It checks invariants after
every operation: canonical replay identity, unique attempts, existing acyclic
correction targets, bounded amplification, finite belief, stable legality and exact
belief replay. Four committed seeds force active/completed/abandoned motif states and
the 64-attempt resource boundary.

The independently receipted 256-run smoke completed with no crash, coverage 8,052,
feature count 18,189, 168 corpus entries and 531 MiB peak RSS. Every declared semantic
counter fired. See
`docs/receipts/artifacts/semantic-gameboard-phase4-fuzz-smoke.json`.

## Public API and dependency review

The only production surface additions are three named BPMN facade operations and one
typed facade error variant:

```text
project_bpmn_attempt_history
record_bpmn_attempt
update_bpmn_design_belief
BpmnBoardError::Continuity
```

They are required by the real application composition root and expose only stable
shared contracts; private history, motif and belief implementation types remain
absent from every snapshot. No module was made public, no glob re-export was added,
and no constructor or tooling/fuzz bypass was exposed.

```text
utterance-engine default:             388 -> 392, sha256 6fb9b5e56c6f1524367eafa60ffb8c6a52e0cc633a7022f0c5c8b31542126ca0
utterance-engine candle-probe:        414 -> 418, sha256 be82efa457dcfd874fbfb37c9f1be6386375b6731e56bb2d65d920f6bae603a6
utterance-engine embed,candle-probe:  424 -> 428, sha256 426bae98f4c3a07a110373e7e5671462bd1e8f0837a581e77fabb3ba2eeec63c
utterance-engine q9-capture:          442 -> 446, sha256 893d1f7bebdeb2a9e4f52c5fb7070c290b41db441fbb4aafff431ffb970fdc01
server all checked features:             8 ->   8, sha256 8b3ea0f6f1762e702261e1fc8b4dc99dee2ff5fd8d9fb229f8d5a2402ae39576
```

No capability crate depends on the server, fuzzer or `xtask`; `xtask` remains
orchestration-only. Production, test, fuzz and tooling features expose the same Phase
4 facade delta.

## Exact phase file ledger

```text
Cargo.lock
Cargo.toml
bpmn-lite-server-designer/src/rest.rs
bpmn-lite-store/src/store.rs
bpmn-lite-store/src/store_memory.rs
docs/receipts/artifacts/semantic-gameboard-phase4-fuzz-smoke.json
docs/receipts/semantic-gameboard-phase4-gate4-2026-08-07.md
docs/receipts/semantic-gameboard-phase4-red-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/fixtures/gameboard_api/facade_consumer.rs
utterance-engine/config/bpmn-semantic-pack.lock
utterance-engine/config/bpmn-semantic-pack.yaml
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/fuzz/fuzz_targets/history_belief_state.rs
utterance-engine/fuzz/seeds/history_belief_state/abandoned.seed
utterance-engine/fuzz/seeds/history_belief_state/active.seed
utterance-engine/fuzz/seeds/history_belief_state/completed.seed
utterance-engine/fuzz/seeds/history_belief_state/resource-bound.seed
utterance-engine/src/belief.rs
utterance-engine/src/bpmn_board.rs
utterance-engine/src/bpmn_pack.rs
utterance-engine/src/history.rs
utterance-engine/src/lib.rs
utterance-engine/src/motifs.rs
```

All pre-existing/concurrent formatting, runner, corpus, bundle, training-log,
`.DS_Store` and normative-document changes remain unstaged and untouched by the phase
commit.

## Gate 5 entry condition

Phase 5 may start only from this committed Gate 4 baseline, retaining the compiler as
referee and explicit human ratification as the sole mutation gate. Its first red
receipt must enumerate every remaining disposition path that still lacks a terminal
typed attempt/workbook outcome, especially early request validation and stale-board
refusals, before replacing the top-one hand-off with game-aware disposition.
