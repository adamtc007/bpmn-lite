# Semantic Gameboard Phase 5 Gate 5 Receipt — 2026-08-07

## Gate status

**GREEN.** Phase 5 replaces the graph-backed Designer's top-one hand-off with a
versioned, complete-evidence Gameboard disposition and a position-bound,
preview-first workbook state machine. Compiler/verifier admission and explicit human
ratification remain the only route to graph mutation. This receipt is Phase 5 only;
Phase 6 and the remaining programme are not complete.

No model was trained and no corpus, model weight or unrelated generated artifact was
regenerated.

## Shared prerequisite

The domain-neutral workbook transition needed to represent a compiler refusal during
ratification was implemented in the shared `dsl` repository as commit
`cc6e31be7da622fd36e2ad5c9f65be10b645a587` (`fix(contracts): record ratification
admission refusal`) and pushed to its authorised branch. The shared contract now
admits `ReadyForRatification -> DryRunRefused` and has an exhaustive transition-matrix
test. BPMN-Lite pins all affected shared dependencies and both lockfiles to that
immutable revision.

The contract remains domain-neutral. No BPMN, Designer or future federation
vocabulary was added to generic shared mechanisms.

## Implemented architecture

- `disposition.rs` implements the versioned `bpmn.game-disposition.information-gain.v3`
  policy over one complete, finite `MoveEvidence` vector per recorded legal move plus
  non-authoritative belief. It produces all ten governed `GameDispositionKind`
  outcomes and rejects incomplete, duplicated or off-board evidence.
- `clarification.rs` chooses at most three recorded legal moves by expected information
  gain across unresolved move, focus and argument dimensions. Questions use only
  admitted pack contrasts, applicability text and argument clarification schemas.
- The application retains the legacy textual disposition only at the explicit
  compatibility boundary. Graph-backed production sessions render the typed game
  disposition through admitted pack resources.
- A workbook is constructed from one selected `LegalMove` and preserves move ID,
  position ID, graph revision, board identity and move-set identity. It is never
  created from an unchecked candidate string.
- Workbook completion uses the public preview facade. The exact canonical operation
  tape and `GraphDeltaPreview` are retained, disclosed before ratification and replayed
  byte-for-byte at ratification.
- Graph, pack, focus, history, policy, position or move-set drift expires or refuses the
  workbook. There is no silent rebase. Preview drift and compiler refusal append a
  typed receipt/audit and leave the authoritative graph unchanged.
- Unknown anchors become typed unknown focus and produce `ChangeFocusOrContext` with a
  governed recovery option instead of being treated as a whole-graph request.
- Every governed disposition path carries a typed attempt receipt when terminal at the
  turn. Successful proposals remain non-terminal until rejection, expiry, refusal or
  ratification, when the terminal receipt is appended to history.
- A request that a previously applied move be corrected resolves current-board legal
  follow-up options. The correction has its own preview, explicit ratification and
  compiler admission, records `correction_of`, and appends a `Corrected` attempt. The
  original `Applied` attempt and its graph effect remain in history.
- `bpmn-lite-store` persists the opaque versioned game disposition beside attempt and
  belief receipts. Storage does not interpret or authorise it.
- No automatic apply route, model authority, belief authority, runtime execution
  semantic change or server/database requirement was introduced into the fuzzable
  kernel.

## Gate 5 evidence

The independent fixture
`utterance-engine/tests/fixtures/gameboard-top3.json` records a gold candidate at rank
three. `utterance-engine/tests/gameboard_disposition.rs` measures both funnel stages:

```text
fixture_count:                1
gold_in_top_three_count:      1
gold_in_top_three_rate:       1.0
clarification_success_count:  1
clarification_success_rate:   1.0
gold_rank:                    3
observed_disposition:         ClarifyMoves
observed_dimension:           Argument
off_board_moves_observed:     0
```

The governed clarification surfaces the third-ranked legal move in one question. Unit
and property tests also prove candidate permutation invariance, complete-evidence
admission, hidden-move non-disclosure and stale/unknown-focus refusal. Server tests
prove that proposals do not mutate before ratification, hostile or invalid transitions
are atomic, graph drift expires the workbook, compiler preview is replayed, and an
already-applied move is corrected only through a linked, previewed, ratified and
compiler-admitted follow-up.

Every unsuccessful disposition fixture returns governed recovery moves, feedback,
focus guidance or an honest escalation/explanation. Validation ensures every named
move belongs to the exact recorded position; hidden and currently illegal moves cannot
enter clarification or feedback.

See `docs/receipts/artifacts/semantic-gameboard-phase5-top3-metrics.json`.

## Focused verification

```text
cargo fmt --all -- --check
  PASS in the protected shared worktree

staged-snapshot focused rustfmt audit
  PASS for every Phase 5 hunk; the snapshot reports only the pre-existing
  adjudication formatting in rest.rs and module-order formatting in lib.rs that
  remain deliberately unstaged with the concurrent work

cargo test -p utterance-engine --all-features --lib --tests
  PASS: 95 unit tests passed, 5 explicitly ignored model/network qualification tests;
        all integration tests passed, including the independent top-three fixture

cargo test -p bpmn-lite-server-designer --all-features --lib
  PASS: 59 passed, 1 explicitly ignored external-model test

cargo test -p bpmn-lite-store --lib
  PASS: 37 passed

the same three test commands against an index-only detached worktree
  PASS with the same counts; no unstaged/concurrent file was needed by Phase 5

cargo clippy -p utterance-engine --no-deps -- -D warnings
cargo clippy -p bpmn-lite-server-designer --no-deps -- -D warnings
cargo clippy -p bpmn-lite-store --no-deps -- -D warnings
  PASS

python3 scripts/check-semantic-gameboard-boundaries.py
  PASS: public API, visibility, dependency direction, facade-consumer and
        compile-fail boundary fixtures across every governed feature surface

cargo run -p xtask -- fuzz list
  PASS: 25 workspace targets discovered; 9 belong to utterance-engine and include
        disposition_workbook_state with 10 committed seeds
```

The dependency-inclusive Clippy baseline still reaches standing unrelated warnings in
`bpmn-lite-kernel/src/lib.rs` (`collapsible_if`) and
`bpmn-lite-compiler/src/lowering.rs` (`match_like_matches_macro`,
`too_many_arguments`). The all-feature utterance-engine warnings-denied invocation
still reaches the pre-existing unused `CapturePipeline` helpers in `capture.rs`
(`on_under_charter`, `charter_ref`, `dataset`). These files and warnings predate Phase
5 and were not changed to make this gate appear green. The changed-package default
`--no-deps` warnings-denied gates are green.

## Fuzz assurance

`disposition_workbook_state` is a public-facade consumer over an explicit bounded byte
operation tape and a compact independent workbook-transition model. Its ten committed
seeds force every disposition. The tape checks after every operation that:

- selected and disclosed moves belong to the recorded position;
- disposition and workbook canonical round trips revalidate;
- illegal transitions do not mutate the workbook;
- workbook position binding remains intact;
- authoritative graph node and edge counts do not change;
- applied-move correction history replays canonically;
- all ten typed attempt outcomes can be constructed and validated.

The refreshed pinned-nightly smoke completed 1,024 runs with no crash, timeout or
artifact. All ten disposition counters and all ten attempt-outcome counters fired.
Final coverage was 6,706 edges and 12,189 features, with 131 live temporary corpus
entries and 550 MiB peak RSS. The committed seed corpus and both committed lockfiles
were not modified by the harness. See
`docs/receipts/artifacts/semantic-gameboard-phase5-fuzz-smoke.json`.

All nine discovered utterance-engine targets also completed their committed-seed
replay from temporary corpus copies; none emitted an artifact.

## Public API and dependency review

The only production public-surface additions are two named operations on the existing
reviewed BPMN capability facade:

```text
decide_bpmn_game_disposition
render_bpmn_game_disposition
```

The real external consumer is the Designer application composition root. The first
operation accepts stable shared position/evidence/belief/attempt contracts; the second
renders only resources already admitted into the board. The state machine fuzzer and
compile-pass fixture consume the same facade. Private clarification, policy and
ranking representations remain private; no module, unchecked constructor, fuzz-only
hook or glob re-export was exposed.

```text
utterance-engine default:             392 -> 394, sha256 508fe9fd2c9a2226ed34995edd80195454f2a4bb71f0756dbd9e87ed5c712fe6
utterance-engine candle-probe:        418 -> 420, sha256 089df67f31ad1d68495eb1fbd374df6b2e5acac6d3b14956aa538dba2dba2190
utterance-engine embed,candle-probe:  428 -> 430, sha256 b42a9bcdd4161c2767d6fcd0ee8d27e53347c9bc01fbe63ca6164664ca59d393
utterance-engine q9-capture:          446 -> 448, sha256 aa4a309a2b145517f689282a389b22e176c5d36020322a73b2b31c5ccabd0e06
server all checked features:             8 ->   8, sha256 8b3ea0f6f1762e702261e1fc8b4dc99dee2ff5fd8d9fb229f8d5a2402ae39576
```

No capability crate depends on an application, fuzzer or `xtask`; `xtask` remains
orchestration-only. Production, test, fuzz and tooling features expose the same
reviewed facade delta.

## Exact phase file ledger

```text
Cargo.lock
Cargo.toml
bpmn-lite-server-designer/src/proposal.rs
bpmn-lite-server-designer/src/rest.rs
bpmn-lite-store/src/store.rs
bpmn-lite-store/src/store_memory.rs
docs/receipts/artifacts/semantic-gameboard-phase5-fuzz-smoke.json
docs/receipts/artifacts/semantic-gameboard-phase5-top3-metrics.json
docs/receipts/semantic-gameboard-phase5-gate5-2026-08-07.md
docs/receipts/semantic-gameboard-phase5-red-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/fixtures/gameboard_api/facade_consumer.rs
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/fuzz/fuzz_targets/disposition_workbook_state.rs
utterance-engine/fuzz/seeds/disposition_workbook_state/arguments.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/clarify.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/compound.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/correction.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/escalate.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/explain.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/focus.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/out_of_scope.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/propose.seed
utterance-engine/fuzz/seeds/disposition_workbook_state/recovery.seed
utterance-engine/src/argument_evidence.rs
utterance-engine/src/bpmn_board.rs
utterance-engine/src/clarification.rs
utterance-engine/src/disposition.rs
utterance-engine/src/legal_moves.rs
utterance-engine/src/lib.rs
utterance-engine/tests/fixtures/gameboard-top3.json
utterance-engine/tests/gameboard_disposition.rs
```

All pre-existing/concurrent formatting, runner, corpus, bundle, training-log,
`.DS_Store`, deleted split-manifest and normative-document changes remain unstaged and
untouched by the phase commit.

## Gate 6 entry condition

Phase 6 may start only from the committed Gate 5 baseline. Its capture schema must
retain the complete game-level turn closure and explicit adjudication distinctions;
wrong, rejected, undone and corrected attempts must not become positive labels. Model
training or corpus regeneration requires the Phase 6 statistical-baseline inputs and
authority and is not implied by this green gate.
