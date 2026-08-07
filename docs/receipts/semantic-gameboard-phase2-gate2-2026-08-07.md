# Semantic Gameboard Phase 2 Gate 2 receipt — 2026-08-07

## Result

Gate 2 is green on branch `codex/bpmn-gameboard-refactor`, entered from
`060273d479ebf6f73ce4734454bd1cf0c5926f97`. The shared semantic contracts remain
exactly pinned to `bc547723e6831cdb46fb8028071db3f537129d77`. This phase did not
train a model, regenerate a corpus, change runtime execution semantics, or introduce
an automatic apply route.

The graph remains authoritative. `PositionalLegality` and the admitted semantic pack
define candidate availability, `DesignerDag::admit` owns preview admission, explicit
human ratification remains the only server route to persistence, and statistical
producers remain evidence-only.

## Architectural decisions

1. Private `game_state` and `legal_moves` modules now enumerate concrete,
   position-bound moves from the same admitted semantic board used by language
   resolution. A whole-graph board expands over canonical BPMN-id anchors; an
   anchored board remains bound to the exact supplied anchor.
   The facade also compares the board revision with an independently supplied
   current graph revision, so a same-anchor stale board fails closed.
2. The first required pack-declared `node_reference` is bound from position. Other
   required values remain typed missing arguments. No value is invented.
3. Fully bound concrete shapes are staged on a clone and passed through
   `DesignerDag::admit`. A compiler-refused shape is excluded from the legal move set
   and retained as a typed diagnostic; authoritative graph state is unchanged.
4. Deterministic materialization for all 19 executable semantic candidates moved from
   the application server into the private capability implementation. Created graph
   identities are content-derived from workbook content and sequence, not ambient
   randomness.
5. The named `bpmn_board` facade exposes validated, read-only bound proposal,
   preview, concrete-position, direct-action identity and governed-guidance
   capabilities. Internal enumeration, mutation tables and reference models remain
   inaccessible.
6. The server's workbook binder is now a thin composition wrapper over the facade.
   Preview uses the exact operation tape later supplied to the existing ratification
   path; the apply and persistence authority boundary was not changed.
7. The palette endpoint and graph-backed language path construct the same concrete
   position. Direct deletion resolves to the same governed `LegalMoveId` where an
   exact move exists.
8. Applicability explanations use admitted pack rule/application text and stable
   typed codes. Recovery choices are bounded and refer only to current legal moves or
   an explicit focus change. Policy-hidden guidance does not name the hidden piece.
9. The plan names `AstMutator`, but the production Designer path is graph-backed:
   `DesignerDag` plus an exact operation tape and `DesignerDag::admit`. Routing Phase
   2 through the legacy textual DSL AST would create a second authority path, so the
   non-mutating preview was implemented at the graph-backed production boundary.
   Preview/apply delta equality preserves the intended invariant without reviving the
   legacy textualisation route.

## Gate 2 evidence

- Palette and language move-set identity: the graph-backed session integration test
  compares anchored and whole-graph palette hashes with the language path and finds
  them identical. Legacy textual sessions fail the gameboard endpoint closed.
- Compiler soundness: every fully bound move offered by concrete enumeration carries
  a compiler-admitted preview. A complete data-object deletion is admitted and maps
  to the direct operation identity; known refused deletion shapes never enter the
  move set.
- Candidate completeness: pack admission proves the 19 executable semantic
  candidates exactly equal the private mutation table. The preview fuzz target
  deterministically materializes a complete typed workbook shape for every one on
  every input.
- Pure enumeration: legal-move construction needs only explicit graph, board,
  revision, compiler profile, policy, focus and history identities. It performs no
  model, server, database, network, clock, random-ID or authoritative mutation call.
- Governed explanations: inapplicable and policy-hidden fixtures return typed
  applicability, pack-derived explanations and legal bounded recovery choices
  without parsing a Rust error string.
- Model-based fuzzing: compact independent models cover generated linear graphs,
  whole/start/task/end focus, move availability, partial and complete binding,
  explicit preview/ratify/refuse/reject transitions, revision advancement and linked
  correction receipts. The models are compared after every tape operation and do
  not call `PositionalLegality`, the compiler, or mutation implementations.
- Semantic coverage: the committed Phase 2 seed emits all four anchor shapes, all 19
  executable candidate shapes, and the Phase 2 outcomes `applied`, `incomplete`,
  `inapplicable`, `stale`, `compiler_refused`, `rejected_by_user` and `corrected`.
  The pinned shared-contract state-machine baseline covers all ten generic attempt
  outcomes and all five disclosure classes, including ambiguous,
  disclosure-safe-refusal and system-failure contracts not produced by a Phase 2
  graph preview transition.
- Boundary enforcement: compile-pass consumers use only the named facade;
  compile-fail fixtures prove `bpmn_pack`, `game_state` and `legal_moves` remain
  inaccessible. No fuzz-only or tooling-only production API exists.

## Verification

The following phase gates passed:

```text
rustfmt --check on every Phase 2 Rust implementation/fuzz file
cargo test -p utterance-engine --no-default-features
  67 unit + 4 integration + 1 doc test passed
cargo test -p bpmn-lite-server-designer --no-default-features
  56 unit tests passed
cargo check -p utterance-engine --features candle-probe
cargo check -p utterance-engine --features embed,candle-probe
cargo check -p utterance-engine --features q9-capture
cargo check -p bpmn-lite-server-designer --features candle-probe
cargo check -p bpmn-lite-server-designer --features embed,candle-probe
cargo check -p bpmn-lite-server-designer --features q9-capture
cargo clippy -p utterance-engine -p bpmn-lite-server-designer \
  --lib --no-default-features --no-deps -- -D warnings
python3 scripts/check-semantic-gameboard-boundaries.py
python3 scripts/check_fuzz_regressions.py
cargo check --manifest-path utterance-engine/fuzz/Cargo.toml --bins
cargo run --quiet -p xtask -- fuzz list --json
python3 -m json.tool \
  docs/receipts/artifacts/semantic-gameboard-phase2-fuzz-smoke.json
git diff --check
```

Feature checks preserved the known pre-existing `q9-capture` dead-code warning for
`CapturePipeline::{on_under_charter,charter_ref,dataset}`. No new warning was added.

The final isolated nightly fuzz smokes used named seeds copied to temporary writable
corpora and a temporary build directory:

```text
legal_move_enumeration: 64 runs, cov 6885, ft 16053, corpus 38, peak RSS 370 MB
preview_compilation:    64 runs, cov 4826, ft 16754, corpus 55, peak RSS 131 MB
```

Both completed successfully. Fuzz discovery increased from 20 to 22 targets. The
machine-readable receipt is
`docs/receipts/artifacts/semantic-gameboard-phase2-fuzz-smoke.json`. Nightly target
discovery will independently execute and receipt both new targets; PR smoke copies
the named seeds to temporary corpus directories before running.

An initial instrumented attempt selected stable Rust and correctly refused nightly
sanitizer flags. A prior ignored cache also contained artifacts from a different
nightly snapshot. Both were infrastructure-only retries; the successful receipt uses
an explicit nightly toolchain and isolated target directory. No committed lockfile,
seed, corpus or artifact was modified by a fuzz run. Generated hash-named files from
an early writable-seed invocation were identified and removed before the final runs;
only the two named seeds remain.

## Public API and dependency review

Phase 1 to Phase 2 public-surface change:

```text
utterance-engine default:             349 -> 385 (+36), sha256 b702df64451b81ab90e40e6109e3f1ebedd00504b8c0eba76e26a30366ac064e
utterance-engine candle-probe:        375 -> 411 (+36), sha256 4b7ba8574ee5001ddc3f05d0dc224b5e696ea92b5eb695cb4783a9909daef4f5
utterance-engine embed,candle-probe:  385 -> 421 (+36), sha256 089f786621b4d692373aaf75c29d443ee57932884811f988cdec4e9c6b3298a3
utterance-engine q9-capture:          403 -> 439 (+36), sha256 ce02b0c7016e090cf81c0d4284a7f95948d60b096c185b19096c8c3fe89b3686
bpmn-lite-server-designer:              8 ->   8 under every checked feature
```

There are no removals and the production addition is feature-invariant. The real
external consumer is the server composition root; the owning facade is
`utterance_engine::bpmn_board`; the stability contract is a validated BPMN adapter
over the shared gameboard v1 contracts. The reviewed reasons are concrete palette
and language construction, deterministic workbook binding/preview, governed
explanations, exact direct-action identity and typed stale-board refusal.
Representations remain private and read-only. There is no public module, glob export,
unchecked constructor, test hook or fuzz-only visibility exception.

Dependency direction remains application-inward. `utterance-engine` depends on the
shared contracts, compiler and `designer-graph`; the server depends on
`utterance-engine`. The fuzz project directly names only the public consumer
dependencies it uses. No capability crate depends on the server, fuzz project or
`xtask`; `xtask` remains orchestration only.

## Exact changed-file ledger

```text
.github/workflows/production-gates.yml
bpmn-lite-server-designer/src/proposal.rs
bpmn-lite-server-designer/src/rest.rs
docs/receipts/artifacts/semantic-gameboard-phase2-fuzz-smoke.json
docs/receipts/semantic-gameboard-phase2-gate2-2026-08-07.md
docs/receipts/semantic-gameboard-phase2-red-2026-08-07.md
scripts/baselines/semantic-gameboard-public-api-v1.json
scripts/fixtures/gameboard_api/facade_consumer.rs
scripts/fixtures/gameboard_api/internal_module_import.rs
utterance-engine/fuzz/Cargo.lock
utterance-engine/fuzz/Cargo.toml
utterance-engine/fuzz/fuzz_targets/legal_move_enumeration.rs
utterance-engine/fuzz/fuzz_targets/preview_compilation.rs
utterance-engine/fuzz/seeds/legal_move_enumeration/linear-anchored.bin
utterance-engine/fuzz/seeds/preview_compilation/insert-after.bin
utterance-engine/src/bpmn_board.rs
utterance-engine/src/bpmn_pack.rs
utterance-engine/src/game_state.rs
utterance-engine/src/legal_moves.rs
utterance-engine/src/lib.rs
```

The phase-scoped commit message is
`feat(designer): enumerate explain and preview concrete legal graph moves`. The
resulting commit identity is reported in the handoff because a commit cannot contain
its own hash.

## Known pre-existing failures and protected work

A broad dependency-inclusive Clippy run remains red only on the existing workspace
baseline: `collapsible_if` in `bpmn-lite-kernel/src/lib.rs` at the previously recorded
3811 and 4113 locations, `match_like_matches_macro` in
`bpmn-lite-compiler/src/dsl/lowering.rs` around 1357, and `too_many_arguments` around
1760. The phase-owned `--no-deps -D warnings` check is green.

Repository-wide Rustfmt is also not a clean baseline: recursive checking reports
pre-existing drift in unrelated `utterance-engine` modules and the existing server
REST file. Phase-owned implementation and fuzz files are formatted; unrelated files
were not rewritten.

All pre-existing `.DS_Store` files, the runner import-order edit, corpus/bundle
outputs, deleted split manifest, untracked normative documents, untracked v3
corpora and training logs remain untouched and unstaged.

## Next phase

Phase 3 is the next phase: make deterministic graph and typed-argument evidence
first-class, introduce complete per-move evidence lanes and governed fusion, and
retain the legal move set as compiler/policy-owned. Its entry conditions are this
Gate 2 receipt, the Phase 2 commit, unchanged authority/ratification invariants, the
admitted semantic pack and exact shared-contract pin. Phase 3 must begin with its own
red receipt and must not train evidence weights.
