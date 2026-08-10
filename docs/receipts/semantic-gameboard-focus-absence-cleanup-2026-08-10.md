# Remove unreachable FocusAbsenceReason / DesignFocus variants

Date: 2026-08-10

Origin: the coverage-audit's dead-surface finding
(`semantic-gameboard-phase8-coverage-audit-2026-08-10.md`) — `FocusAbsenceReason`
had 4 of 5 variants and `DesignFocus` had 1 of 4 variants constructed nowhere in
either `bpmn-lite` or the pinned `semantic-decision-contracts` crate. Investigated
each variant's intended capability (reported to the user separately), then removed
on explicit instruction. User ruled: remove all 5 (`ClearedByUser`, `UnknownReference`,
`PolicyDecision`, `LegacyProjection`, `DesignFocus::Subgraph`), and commit+push the
change in the pinned `dsl` repo rather than staging it locally only.

## Cross-repo mechanics

`semantic-decision-contracts` is a separate, exact-pinned repo
(`github.com/adamtc007/dsl`, pinned by git `rev` in `bpmn-lite`'s `Cargo.toml` per
the settled "cross-pack refs are exact pins, never floors" rule) — not a local
in-repo cleanup. Sequence:

1. Edited and committed in `dsl` (`refactor/sem-os-pack-policy`, commit `9cf7cb3`),
   pushed. Tagged `v0.3.0` (workspace version bumped `0.2.2` → `0.3.0` — a real
   breaking change to a shared crate, not a patch).
2. Re-pinned `bpmn-lite`'s `Cargo.toml` (and the two fuzz sub-workspaces' own
   `Cargo.toml`s, which pin the same crate independently) from `452342e` to `9cf7cb3`.
3. Updated every call site in `bpmn-lite` that the signature/shape change touched.
4. Verified clean builds and full test suites in **both** repos before treating
   either as done.

## What changed in `dsl` (`crates/semantic-decision-contracts`)

- `FocusAbsenceReason`: 5 variants → 1 (`NotProvided` only).
- `DesignFocus::Subgraph { elements }` removed (3 variants remain: `Absent`,
  `Element`, `Unknown`).
- `DesignFocus::Absent`'s `policy_decision: Option<PolicyDecisionId>` field
  removed — it existed only to pair with the now-removed `PolicyDecision` reason;
  keeping it would have meant a field that could only ever be `None`, exactly the
  kind of half-removed residue the working contract forbids.
- `PolicyDecisionId` (a `text_identity!` type) removed entirely — the field above
  was its only use anywhere in the crate.
- `DesignFocus::absent()` simplified from a fallible two-argument constructor
  (`reason, policy_decision) -> Result<Self, GameboardContractError>`) to an
  infallible one-argument one (`reason) -> Self`), since the only cross-field
  validation it existed to enforce no longer has two fields to validate between.
- `DesignFocus::subgraph()` constructor removed.
- `hash_focus` (the function that feeds `DesignPosition::state_id`/`move_set_hash`
  content-addressing) lost its `Subgraph` match arm and the `Absent` arm's
  `"policy"` hash field.
- Manual `Deserialize` impl for `DesignFocus` simplified to match (no more
  `Subgraph` wire variant, `absent()` no longer fallible).
- 19 call sites across the crate's own tests and its facade-boundary fixture
  (`scripts/fixtures/semantic_gameboard_contracts/facade_consumer.rs`) updated to
  the new constructor signature.

## A real, deliberate hash break — not a regression

Removing a field from `hash_focus`'s `Absent` preimage and `Subgraph` from the
match legitimately changes `DesignPosition::state_id()`/`move_set_hash()` and the
position's canonical JSON encoding for any focus, consistent with the "version =
content hash" principle: the content model actually changed, so its hash must too.
One golden-byte test
(`gameboard::tests::position_round_trip_is_canonical_and_has_golden_bytes`) failed
exactly as expected on the first run. Recomputed all three golden values fresh
(temporarily replaced the assertions with `eprintln!`, ran the test, captured the
real output, restored the assertions with those values) rather than guessing or
disabling the check — the rest of the 350+-test `dsl` workspace suite passed
unmodified, confirming this was the only place the hash-shape change was asserted.

## What changed in `bpmn-lite`

- Re-pinned `Cargo.toml` (workspace) and both fuzz sub-workspaces'
  (`utterance-engine/fuzz/Cargo.toml`, `bpmn-lite-server-designer/fuzz/Cargo.toml`)
  `rev` from `452342edffde74164719707a1174bc17fad0f493` to
  `9cf7cb3ae4661742501897ba82622a9834cf8c7c`. Caught by grep, not assumption — the
  fuzz sub-workspaces pin the crate independently of the main workspace and would
  have silently kept building against the old rev otherwise.
- 14 call sites across `bpmn-lite-server-designer/src/rest.rs`,
  `utterance-engine/src/{bpmn_board,capture,resolver_comparison}.rs`, and 4 fuzz
  targets updated to the new `DesignFocus::absent(reason)` signature.
- Two exhaustive `match` statements over `DesignFocus` in `rest.rs`
  (`validate_pending_position`'s focus-anchor resolution, an `assert!(matches!(...))`
  in a test) lost their `Subgraph` arm.
- **`utterance-engine/src/graph_features.rs::locality`** — a real, previously
  unfound consumer. This function (graph-position evidence scoring, feeding move
  ranking) exhaustively matched `DesignFocus` including a `Subgraph { elements } =>
  elements.contains(anchor)` arm. My original coverage-audit claim ("`Subgraph` is
  constructed nowhere") was accurate on construction but incomplete on consumption
  — I had grepped for the `DesignFocus::subgraph(` constructor call and the
  `DesignFocus::Subgraph` pattern only within `utterance-engine/fuzz/fuzz_targets`,
  not the full `src/` tree. Found only when rustc's real compiler diagnostics (not
  a stale editor cache) surfaced it during this cleanup. The arm was reachable in
  the type system but never actually reachable at runtime — nothing ever
  constructed a `Subgraph`-focused `DesignPosition` to feed it — so removing it
  changes no real behavior, but the miss itself is worth naming rather than
  quietly folding into "already checked."

## Results

- `dsl` workspace: `cargo build --workspace --all-targets` clean;
  `cargo test --workspace` — 350+ tests across all member crates, 0 failed (one
  golden-byte test updated deliberately, not silenced).
- `bpmn-lite` workspace: `cargo check --workspace --all-targets` and
  `--all-features` both clean after re-pinning.
- `cargo test -p utterance-engine --all-features`: 115 passed, 0 failed, 5 ignored
  — no golden-hash assertions broke here (none hardcode a focus-derived hash).
- `cargo test -p bpmn-lite-server-designer --lib`: 76 passed, 0 failed.
- Both fuzz sub-workspaces (`utterance-engine/fuzz`, `bpmn-lite-server-designer/fuzz`):
  re-pinned, `cargo +nightly check --all-targets` clean. Smoke-ran the four fuzz
  targets that construct `DesignFocus`/`FocusAbsenceReason`
  (`legal_move_enumeration`, `evidence_fusion`, `history_belief_state`,
  `model_boundary`) for 5s each: 0 crashes.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, unchanged —
  confirms `DesignFocus`/`FocusAbsenceReason`'s shape isn't part of the tracked
  public-API boundary surface for `utterance-engine`/`bpmn-lite-server-designer`.

## Scope note

This closes the dead-surface finding from the coverage audit. It does not revisit
whether `PolicyDecision`'s underlying capability (an audited guardrail against
silent default-focus selection, named explicitly in the plan doc) should be
rebuilt later if that feature ever gets designed — removing the placeholder now
doesn't preclude adding a typed reason back when there's a real producer for it.
