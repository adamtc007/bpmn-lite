# Gate 8 bullet 4 — resource-limit typed-failure testing

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification.

Entry authority: `docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14 ("Gate 8" bullet 4,
"Resource-limit failures are typed and leave the session usable") and the fuzz
governance bullet requiring "explicit limits for decode allocations, graph/move
amplification, history and feedback depth, transition time, Wasm fuel and linear
memory." Carried forward as genuinely open by
`docs/receipts/semantic-gameboard-phase8-gate-2026-08-10.md` ("OPEN, not addressed
this session").

## Survey before implementation

A research pass across `utterance-engine`, `bpmn-lite-server-designer` and the
pinned `semantic-decision-contracts` crate found the gap deeper than "add a test":

- The only typed, resource-limit-specific `Result::Err` variant anywhere was
  `ResolverBoundaryRefusal` (Phase 6's offline-resolver comparison path, a narrow
  downstream consumer guard). Every other limit — history, capture, ledger, motifs,
  proposal slots — failed via `anyhow::bail!` collapsed into a generic
  string-payload variant (`BpmnBoardError::Continuity(String)`,
  `ProposalError::InvalidAnswer(String)`), not pattern-matchable.
- No decode-allocation cap existed in the shared contract crate at all —
  `DesignPosition`, `LegalMove`, `MoveAttemptReceipt` and `ContractText` had no
  vec-count or string-length ceiling. The only floor was axum's unconfigured,
  implicit 2 MiB body default.
- No graph/move amplification cap existed — `legal_moves::enumerate` and
  `GraphDeltaPreview` were both unbounded.
- No transition-time/deadline enforcement existed anywhere.
- No Wasm runtime exists in this stack at all — the "Wasm fuel and linear memory"
  sub-bullet has nothing to check against; it is new infrastructure, not a missing
  check, and is out of scope here.
- The fuzz suite actively dodges this: every decode fuzzer self-caps its input size
  below where a real product limit would even bite, so none of them exercise an
  oversized-input/typed-refusal path.

User ruled (via AskUserQuestion): "Full" scope — design and add the missing caps,
not just type what already existed.

## What changed in `dsl` (`crates/semantic-decision-contracts`, commit `12d5280`)

Added `GameboardContractError::ResourceLimitExceeded { field, limit, actual }`,
distinct from `InvalidContract` so callers can react to a resource refusal (leave
the session usable, do not retry unmodified) without string-matching. Wired five
new bounds, all in `gameboard.rs`:

- `MAX_CONTRACT_TEXT_BYTES` (64 KiB) in `validate_text` — closes the decode
  allocation gap; every `ContractText`-backed field (compiler profile, policy
  identity, provenance, explanation prose, effect codes, ...) now has a length
  ceiling.
- `MAX_MOVE_ARGUMENTS` / `MAX_APPLICABILITY_FACTS` (64 each) in `LegalMove::new`.
- `MAX_LEGAL_MOVES` (512) in `DesignPosition::new` — bounds move-set amplification
  at the contract boundary, independent of any enumeration-time bound a caller
  applies upstream.
- `MAX_DELTA_OPERATIONS` (256) in `GraphDeltaPreview::new`.
- `MAX_VALIDATED_ATTEMPTS` (1024) in `validate_attempt_history` — a generic
  contract-layer safety backstop, distinct from and looser than any tighter
  product-specific policy limit a caller enforces on top (e.g. bpmn-lite's own
  `MAX_HISTORY_ATTEMPTS = 64`). Matters because `GameTurnRecord`'s constructor
  calls `validate_attempt_history` directly on an unbounded
  `related_attempts: Vec<MoveAttemptReceipt>` with no caller-side cap upstream of
  it.

Every existing call site in this crate and in `bpmn-lite` that already propagates
`GameboardContractError` (via `?` or a `#[from]` conversion) became
resource-limit-aware for free — no call-site changes were required for these five
checks to take effect.

Six new unit tests (`gameboard::tests::oversized_*`) prove, for every new bound:
typed refusal at limit+1 with the exact expected `field`/`limit`/`actual`, and
that a legitimate call at or under the limit still succeeds (session usability at
the construction boundary). Full workspace suite green (dsl: 350+ tests, 0
failed). The golden-byte test
(`position_round_trip_is_canonical_and_has_golden_bytes`) is untouched — these are
pure new rejection paths, not content-model changes, so no existing accepted value
changes shape or hash.

Breaking, deliberately: a new variant on a non-`#[non_exhaustive]` public error
enum breaks an exhaustive match, even though no previously-valid input is newly
rejected. Version bumped `0.3.0` -> `0.4.0`, tagged `v0.4.0`, pushed to
`refactor/sem-os-pack-policy`.

## What changed in `bpmn-lite`

- Re-pinned `Cargo.toml` (workspace) and both fuzz sub-workspaces'
  (`utterance-engine/fuzz`, `bpmn-lite-server-designer/fuzz`) `rev` from
  `9cf7cb3ae4661742501897ba82622a9834cf8c7c` to
  `12d5280e59eacbe035959bcc9fa3008d4c4c7a47`. Lockfiles regenerated in all three
  workspaces via `cargo update`.
- **`utterance-engine/src/bpmn_board.rs`**: added a shared
  `pub struct ResourceLimitExceeded { pub field: &'static str, pub limit: usize,
  pub actual: usize }` and `BpmnBoardError::ResourceLimit(#[from]
  ResourceLimitExceeded)`, used by every product-owned (not contract-layer) limit
  below.
- **`utterance-engine/src/history.rs`**: `project()`'s two existing checks
  (`MAX_HISTORY_ATTEMPTS = 64`, `MAX_HISTORY_BYTES = 64 KiB`) were previously
  `anyhow::bail!`, collapsed by the one call site into
  `BpmnBoardError::Continuity(String)`. Retyped: `project()` now returns
  `Result<HistoryProjection, BpmnBoardError>` directly, constructing
  `ResourceLimitExceeded` for both checks; the call site
  (`project_bpmn_attempt_history`) simplified from a `.map_err(...to_string...)`
  wrapper to a plain `?`.
- **`utterance-engine/src/legal_moves.rs`**: `enumerate()` had no amplification
  bound at all — the anchor x candidate nested loop ran unconditionally, including
  the expensive per-candidate compiler preview (`position_bound_move` ->
  `preview_operations`) for every combination, before the contract layer's new
  `MAX_LEGAL_MOVES` could ever reject the resulting set. Added
  `MAX_ENUMERATION_CANDIDATES = 4096` and an early-exit check inside the loop,
  before the board-membership filter and before any compiler work runs for
  candidates beyond the cap. This is the real "graph/move amplification" and
  "cannot cause... repeated compiler work" fix — it is a distinct, tighter bound
  than the contract layer's post-hoc `MAX_LEGAL_MOVES`, because it fires before
  the expensive work happens, not after.
- **`bpmn-lite-server-designer/src/rest.rs`**: added an explicit
  `MAX_REQUEST_BODY_BYTES = 8 MiB` and `.layer(DefaultBodyLimit::max(...))` on the
  router. Axum's unconfigured default is 2 MiB; legitimate whole-graph/session-save
  payloads can exceed that, so this is set generously higher while still being an
  explicit, asserted, product-owned bound instead of an unowned framework default
  nobody chose.

## Tests

- `gameboard.rs`: 6 new unit tests (dsl side, described above).
- `history.rs`: strengthened the existing over-limit test to assert the exact
  typed variant (was previously just `is_err()`), plus a new session-usability
  assertion (a bounded window still projects after the refusal). Added a second
  test that reaches `MAX_HISTORY_BYTES` specifically, with an attempt count well
  under `MAX_HISTORY_ATTEMPTS` so the byte guard — not the count guard — is what
  trips: `receipt()` (the only production constructor) derives rule/feedback
  content from a small fixed catalogue keyed by outcome, so a realistic receipt
  never approaches 64 KiB on its own and the count limit binds first in practice.
  The byte guard is a real, independently reachable code path on `project()`'s
  general `&[MoveAttemptReceipt]` signature, so the test constructs receipts
  directly through the contract constructor (bypassing `receipt()`) with inflated
  `rule_explanations`/`feedback_options` vectors to reach it honestly, rather than
  asserting a placebo `is_ok()` that wouldn't actually exercise the path.
- `legal_moves.rs`: new test builds a 601-node linear chain DAG, requests a
  whole-graph (unanchored) board over it, and asserts `enumerate` returns the
  typed `ResourceLimit` refusal with the exact expected field/limit — then asserts
  a small, legitimate two-node graph still enumerates successfully afterward
  (session usable).
- `rest.rs`: new test posts an 8 MiB + 1 byte body to `/api/dsl/sessions`, asserts
  `413 Payload Too Large` (not a hang, not a decode attempt), then posts a
  legitimate request on the same router and asserts it still succeeds
  (`201 Created`).

## Public API drift — reviewed, not silent

`scripts/check-semantic-gameboard-boundaries.py` failed after this change, as
designed: the new `pub struct ResourceLimitExceeded` and
`BpmnBoardError::ResourceLimit` variant are real, deliberate new public surface (5
new API items via `cargo public-api`, confirmed by diffing before/after output —
nothing else drifted). Baseline
(`scripts/baselines/semantic-gameboard-public-api-v1.json`) updated for all four
`utterance-engine` feature-combination entries with freshly computed
items/sha256. `bpmn-lite-server-designer`'s entries are unchanged (the
`DefaultBodyLimit` layer is internal to `designer_router`'s body, not a signature
change).

## Results

- `dsl` workspace: `cargo build --workspace` and `cargo test --workspace` clean
  (350+ tests, 0 failed); golden-byte test unaffected.
- `bpmn-lite` workspace: `cargo check --workspace --all-targets --all-features`
  clean.
- `cargo test -p utterance-engine --all-features`: all passing, 0 failed.
- `cargo test -p bpmn-lite-server-designer --lib`: 77 passed, 0 failed (was 76;
  +1 new body-limit test).
- Both fuzz sub-workspaces re-pinned; `cargo +nightly check --all-targets` clean
  in both.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, against the
  updated baseline.
- `python3 scripts/check_fuzz_regressions.py`: pass, unaffected (3 governed
  regression cases, unchanged).

## Scope note — what this does not close

This closes Gate 8 bullet 4. It does not touch:

- Bullet 3 (ratified performance budget) — not code work; needs Adam's sign-off
  on numbers against the existing `gameboard_perf.rs` harness.
- Bullet 5 (a dedicated wrong-move-traffic/disposition-loop resource-bound test,
  distinct from the storage-layer bound this session's work rests on) — separate,
  still open.
- Bullet 7 (resource-abuse corpora) — separate, still open. Note the new caps
  added here give the fuzz suite real limits to target; broadening
  `legal_move_enumeration.rs`'s self-imposed tiny graph size (currently ≤4 tasks)
  to actually probe `MAX_ENUMERATION_CANDIDATES`/`MAX_LEGAL_MOVES` is natural
  follow-on work for that bullet, not done here.
- Wasm fuel/linear memory — no runtime exists in this stack; out of scope until
  one does.
