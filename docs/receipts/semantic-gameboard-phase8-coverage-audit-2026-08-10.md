# Semantic gameboard Phase 8 — full 15-target coverage audit

Date: 2026-08-10

Phase: 8 — property, fuzz, differential and performance qualification
(`docs/todo/EOP-PLAN-BPMN-GAMEBOARD-001.md` §14, Gate 8 bullet 6: "Every target
has a completed receipt; semantic coverage includes every move kind, attempt
outcome, disposition, disclosure class and correction lifecycle or records a
reviewed unreachable justification.")

This was left explicitly open by `semantic-gameboard-phase8-gate-2026-08-10.md`
("out of scope this session" — only the 5 new targets' own receipts existed;
no audit had cross-checked the other 10 pre-existing targets, or the
canonical enum/registry definitions, against this bullet's five named
categories). This receipt closes that gap by reading all 15
`utterance-engine/fuzz/fuzz_targets/*.rs` files and the pinned
`semantic-decision-contracts` crate's own enum definitions directly, then
cross-referencing by `grep`, not by re-trusting per-target receipts' prose.

## Method

For each of the five named categories, found the canonical enum/registry
definition (in `dsl/crates/semantic-decision-contracts/src/gameboard.rs`, or
`utterance-engine/src/legal_moves.rs` for the candidate registry), then
grepped every fuzz target for every variant construction — not just variant
*mentions* in a `match`, which can be exhaustive without ever driving a real
value through. Where a variant was never constructed by any fuzz target,
searched the entire `bpmn-lite` + `dsl` workspace (`find ... | xargs grep`,
excluding `/target/`) for any producer at all, to distinguish "fuzzing missed
it" from "nothing in this codebase produces it yet."

## 1. Move kind (candidate registry) — CLOSED, 20/20

Canonical registry: `utterance-engine/src/legal_moves.rs` declares exactly 19
`op.*`/`prod.*` candidate ids, plus the one reserved abstention candidate
(`ABSTENTION_CANDIDATE_ID`) = 20 legal-move-eligible kinds total. (Other
`"op.*"` / `"prod.*"` string literals found elsewhere in `bpmn_board.rs` —
e.g. `op.attach_rollback_guard`, `op.call_subprocess`, `op.create_race` — are
motif-catalog/doc-reference strings, not legal-move candidate ids; confirmed
absent from `legal_moves.rs`'s registry.)

- `preview_compilation.rs`'s `EXECUTABLE_CANDIDATES` constant is the same 19
  ids verbatim. `exercise_all_materializers` runs every one through
  `materialize_bpmn_workbook` every single fuzz execution (not
  input-dependent) and asserts `reached.len() == EXECUTABLE_CANDIDATES.len()`
  — this is a hard assertion, not an observed-coverage log; the target cannot
  pass without hitting all 19.
- Abstention: `legal_move_enumeration.rs` asserts exactly one abstention move
  is present in every generated position; `disposition_workbook_state.rs`
  looks it up and drives evidence into it directly (scenarios 5/6/8:
  "explain"/"escalate"/"out of scope").

20/20 candidate kinds are constructed and exercised by the fuzz suite.

## 2. Attempt outcome (`MoveAttemptOutcome`, 10 variants) — CLOSED as plumbing coverage, with a named nuance

All 10 variants (`Applied`, `Incomplete`, `Ambiguous`, `Inapplicable`,
`DisclosureSafeRefusal`, `Stale`, `CompilerRefused`, `RejectedByUser`,
`Corrected`, `SystemFailure`) are constructed and round-tripped (receipt
construction, serde round-trip, history projection) by at least one of
`disposition_workbook_state.rs`, `move_attempt_feedback.rs`, and
`preview_compilation.rs` — confirmed by grepping every
`MoveAttemptOutcome::Variant` construction site across all 15 targets, not
just the 5 new ones.

**Nuance worth naming explicitly, not silently folded into "closed":** two of
the ten — `SystemFailure` and `DisclosureSafeRefusal` — have **zero
producers anywhere in either the `bpmn-lite` or `dsl` workspace**, outside of
fuzz targets and the contract crate's own unit tests. Read
`utterance-engine/src/disposition.rs::decide_game` (the live policy that
`bpmn_board.rs::decide_bpmn_game_disposition` actually calls) end to end:
it only ever *constructs* receipts with `MoveAttemptOutcome::Inapplicable`
internally; it only *reads* `SystemFailure`/`DisclosureSafeRefusal` back out
of prior history (for the 3-recent-failures escalation count, and to route
`ExplainAttempt` after a `CompilerRefused | Stale | Inapplicable |
DisclosureSafeRefusal | SystemFailure` tail). `history.rs`, `fusion.rs`, and
`funnel.rs` are the same shape — each pattern-matches these two variants to
derive a rule code, an evidence-history score floor, or a compiler-admission
tally, but none of them ever assigns the variant to a fresh receipt. Grepping
the full two-repo tree for `MoveAttemptOutcome::SystemFailure` /
`::DisclosureSafeRefusal` construction sites turns up exactly: 3 fuzz
targets, and 2 lines in the contract crate's own test module.

This means these two outcomes are architecturally reserved for a caller that
doesn't exist yet in this codebase — presumably a system-error handler
upstream of `record_bpmn_attempt` (for `SystemFailure`, e.g. a panic caught
during materialization) and a disclosure-policy refusal upstream of workbook
preview (for `DisclosureSafeRefusal`, the outcome tag that would pair with
`DisclosureClass::PolicyHidden`'s rule code in `history.rs`). No fuzz target
can exercise a producer that isn't wired into any caller — this is exactly
the "reviewed unreachable justification" the bullet's own wording allows for,
not a fuzzing gap. Flagging because it's a real, previously-undocumented
architectural fact (these two outcomes are consumer-only today), not because
it blocks this bullet.

## 3. Disposition (`GameDispositionKind`, 10 variants) — CLOSED, 10/10

`disposition_workbook_state.rs` (pre-existing, not new this session) drives
10 named scenarios (`propose`, `clarify`, `request arguments`, `recover`,
`correct`, `explain`, `escalate`, `change focus`, `out of scope`, `insert
before; insert after`) through the real `decide_bpmn_game_disposition` policy
and asserts, via an exhaustive `match` assigning each of the 10
`GameDispositionKind` variants a distinct observed-counter bit, that every
kind is reached at least once per fuzz run. `clarification_policy.rs` (new
this session) separately fuzzes the `ClarifyMoves` branch specifically, with
its own evidence-banding generator, for deeper coverage of that one kind.
10/10, both breadth (all kinds reached) and depth (the highest-scrutiny kind
independently fuzzed).

## 4. Disclosure class (`DisclosureClass`, 5 variants) — CLOSED, 5/5

`rule_explanation_decode.rs` (new this session) constructs all 5
(`Public`, `Authenticated`, `Restricted`, `PolicyHidden`, `Technical`)
directly and fuzzes `filter_rule_explanations`'s allow-list behavior across
all 32 subset masks over the 5 classes — already receipted in the
fuzz-target-tranche receipt; reconfirmed here by grep against the canonical
enum in `gameboard.rs` rather than re-trusting the prior receipt's count.

## 5. Correction lifecycle — CLOSED, with real depth

All 3 `CorrectionKind` variants (`Undo`, `Replacement`, `FollowUp`) are
constructed: `Undo` only by `correction_history.rs` (new this session — no
pre-existing target ever constructed it); `Replacement` by
`correction_history.rs`, `evidence_fusion.rs`, `preview_compilation.rs`;
`FollowUp` by `disposition_workbook_state.rs`, `move_attempt_feedback.rs`,
`history_belief_state.rs`.

Lifecycle *stages*, not just the tag: no-correction, single correction,
self-correction (refused at `MoveAttemptReceipt::new` construction time, per
`correction_history.rs` scheme 2), a forward reference that is invalid at
append time and becomes valid once its target is later appended (scheme 3,
the order-independent resolution semantics this session's own fuzz-target
work discovered), a phantom target that never resolves (scheme 4, permanently
invalid), and genuine multi-hop correction *chains* — `preview_compilation.rs`
case 5 corrects the immediately-preceding applied receipt each time across a
32-step tape, so successive corrections chain arbitrarily deep, and
`correction_history.rs`'s own reference model walks up to `len + 1` hops to
stay correct against chains of any depth. This is real lifecycle-stage
breadth, not just tag presence.

## Adjacent finding, surfaced but out of this bullet's literal scope

While tracing every enum construction site for the audit above, found that
`FocusAbsenceReason` (5 variants: `NotProvided`, `ClearedByUser`,
`UnknownReference`, `PolicyDecision`, `LegacyProjection` — the reason
attached to `DesignFocus::Absent`) is **overwhelmingly single-variant in
practice**: every fuzz target, every `utterance-engine` unit test, and every
`bpmn-lite-server-designer` call site constructs `FocusAbsenceReason::NotProvided`
and nothing else, except one single unit test inside the contract crate itself
(`gameboard.rs:6368`) that uses `LegacyProjection`. `ClearedByUser`,
`UnknownReference`, and `PolicyDecision` are constructed **nowhere in either
repository** — not fuzzed, not unit-tested, not produced by the REST
application layer. Separately, `DesignFocus::Subgraph` (one of the 4
`DesignFocus` variants, alongside `Absent`/`Element`/`Unknown`) is likewise
never constructed anywhere.

This is not one of Gate 8 bullet 6's five named categories (move kind /
attempt outcome / disposition / disclosure class / correction lifecycle), so
it isn't claimed as a gap against this bullet — but it's the same kind of
fact this audit exists to surface, so it's named here rather than dropped.
Whether `ClearedByUser`/`UnknownReference`/`PolicyDecision`/`Subgraph` are
genuinely reachable today (and just untested) or are aspirational surface for
a UI/application layer that doesn't exist yet is an open question for
whoever owns the focus/UI wiring — not decided here.

## Disposition

Gate 8 bullet 6 — **CLOSED**. All five named categories verified at 100%
variant/kind construction coverage across the 15-target suite, cross-checked
against canonical definitions rather than re-trusting per-target receipt
prose. The one real nuance (`SystemFailure`/`DisclosureSafeRefusal` having no
producer in this codebase) is named as a reviewed-unreachable fact, matching
the bullet's own escape clause, not asserted as full closure by omission.

`semantic-gameboard-phase8-gate-2026-08-10.md` bullet 6 is updated from
"PARTIALLY closed" to CLOSED, pointing here.

## Results

No code was changed — this is a read-only audit (file reads + grep
cross-reference). No build/test re-run required; nothing here could regress
what compiled before.
