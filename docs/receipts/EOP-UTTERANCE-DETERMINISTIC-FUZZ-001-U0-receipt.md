# EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 — U0 receipt

Baseline reviewed: `efda5ad`. Current revision: `be9a86f` (branch
`codex/bpmn-gameboard-refactor`) — `efda5ad` is a genuine ancestor;
between them sits the entire EOP-PLAN-CRATE-HYGIENE-001 tranche
(`89ae3e6`..`be9a86f`), which added `bpmn-lite-compiler/fuzz:plan_deserialize`
and `dmn-lite-bridge/fuzz`, both accounted for below. **Tier: CAREFUL. No
production code changed — U0 forbids code changes; this tranche is
evidence and target-contract only.**

Pre-execution fact-check (independent Explore-agent pass over §1–§4 of the
plan, prior to this tranche) confirmed the plan's core technical claims
against the live repo, with two findings folded into this receipt:
§2's existing-coverage table names 9 of 15 `utterance-engine/fuzz` targets
(work item 1 below completes the list), and `history_belief_state.rs`
overlaps substantially with U1's proposed scope (work item 2 below records
Adam's ruling: extend it, not build fresh).

---

## Work item 1 — target list, corpus size, PR/nightly policy

### Full target inventory (corrects plan §2's 9-of-15 table)

`utterance-engine/fuzz/Cargo.toml` declares 15 `[[bin]]` targets, all present
as files and all building:

| Target | Corpus | Regressions | Seeds | In plan §2? |
| --- | --- | --- | --- | --- |
| `semantic_board_decode` | 3072 | 0 | 1 | no |
| `phrase_index` | 163 | 0 | 1 | yes |
| `workbook_transition` | 118 | 0 | 1 | no |
| `v3_route_admission` | 162 | 0 | 1 | yes |
| `legal_move_enumeration` | 335 | 0 | 1 | yes |
| `preview_compilation` | 451 | 1 | 1 | yes |
| `evidence_fusion` | 266 | 0 | 7 | yes |
| `history_belief_state` | 514 | 0 | 4 | no |
| `disposition_workbook_state` | 244 | 0 | 10 | yes |
| `model_boundary` | 399 | 1 | 0 | yes |
| `clarification_policy` | 29 | 0 | 0 | yes |
| `move_attempt_feedback` | 35 | 0 | 0 | no |
| `correction_history` | 450 | 0 | 0 | no |
| `rule_explanation_decode` | 2602 | 0 | 0 | no |
| `game_turn_replay` | 180 | 0 | 0 | yes |

`bpmn-lite-compiler/fuzz/Cargo.toml` declares 2 targets, both plan-cited:

| Target | Corpus | Regressions | Seeds |
| --- | --- | --- | --- |
| `dsl_compile` | 8465 | 0 | 2 |
| `plan_deserialize` | 4184 | 0 | 1996 |

`plan_deserialize` was added in `89ae3e6`, strictly after the plan's stated
`efda5ad` baseline — citing it as "existing coverage" is accurate as of
current HEAD but the plan's baseline line should be bumped to `be9a86f` (or
later) before U1 starts, since it's implicitly relying on post-baseline
state. `dmn-lite-bridge/fuzz` (also added in `89ae3e6`) fuzzes an unrelated
DMN decision-table boundary — confirmed correctly out of this plan's scope,
not an omission.

**New finding, not in the plan or the pre-execution fact-check**: the
pinned external dependency `semantic-decision-contracts` (git rev
`1d039d958a91620ab15374f05176bdfac4c872d1`, sibling repo
`/Users/adamtc007/dev/dsl/crates/semantic-decision-contracts`) has its own
`fuzz/` project with 8 targets: `design_position_contract`,
`design_belief_contract`, `game_disposition_contract`, `legal_move_contract`,
`attempt_receipt_contract`, `rule_explanation_contract`,
`feedback_option_contract`, `semantic_pack_admission`. Read
`game_disposition_contract.rs`: it constructs `GameDisposition` via
`arbitrary::Arbitrary` (not through the real board→position→evidence→belief
pipeline) and round-trips/hostile-decodes the type in isolation. This is
**contract-level type fuzzing, not pipeline-composition fuzzing** — it
stresses `GameDisposition::validate_for_position`
(`semantic-decision-contracts/src/gameboard.rs:2162`) and friends against
arbitrary byte-derived values, not against a position genuinely produced by
`build_bpmn_design_position`. It does not close this plan's gap (no target
anywhere composes the real pipeline through to disposition) but it is
adjacent existing coverage worth citing in §2 so a future reader doesn't
conclude `GameDisposition`'s own validation logic is entirely unfuzzed. It
also lives in a separate repo/CI, outside this plan's and this workspace's
gates — not something U1 can or should assume runs.

### PR-smoke vs. nightly-discovery policy — confirmed

`nightly-fuzz.yml`'s `discover` job runs `cargo run -p xtask -- fuzz list
--json` as the matrix source — genuinely automatic `[[bin]]` discovery, no
target enumerated by name. A new `[[bin]]` (or an existing one gaining new
scope) is picked up automatically for the nightly 20-minute run without any
workflow edit. Confirmed live: `xtask/src/fuzz.rs` discovers
per-crate `fuzz/` projects by walking `[[bin]]` entries, not a hardcoded
list.

`production-gates.yml`'s PR-time smoke (`fuzz-regressions` job) is *not*
automatic — it names 9 individual targets explicitly by
`cargo fuzz run <target>` invocation: `v3_route_admission`,
`legal_move_enumeration`, `preview_compilation`, `evidence_fusion` (of the
utterance-engine targets), plus `designer_operation_apply`, `dmn_lite_parse`,
`yaml_workflow_parse`, `zeebe_bpmn_import`, `owner_metadata_decode` (other
crates). `history_belief_state` — U1's extension target — is **not** in
PR-time smoke today; only nightly discovery exercises it. U2 work item 1
("add the target to the existing PR-time smoke discipline") must add an
explicit `cargo fuzz run history_belief_state ...` step, since PR-smoke
inclusion is manual, not discovered.

## Work item 2 — P1–P7 mapped to existing coverage; history_belief_state ruling

**Ruling (Adam, this session): U1 extends `history_belief_state.rs` in
place — adds the missing disposition/closure step and the missing text/
hostile-axis dimensions — rather than building a new
`deterministic_discovery_pipeline.rs` that duplicates its board/position/
evidence/belief setup.** This changes U1's Work item 1 from "add
`deterministic_discovery_pipeline.rs` and its `[[bin]]` entry" to "extend
`history_belief_state.rs`, no new `[[bin]]` entry." The target's existing
name stays; the plan's references to `deterministic_discovery_pipeline` as
a literal filename are superseded by this ruling and should be read as "the
core target (`history_belief_state`)" throughout U1/U2.

Read `history_belief_state.rs` in full (306 lines) to ground this mapping —
not inferred from the target's name. **Correction (post blind-review):**
the first version of this table only cross-checked `history_belief_state.rs`
against P1–P8 and materially overclaimed the gap for P2/P3/P4/P6 as a
result — an independent reviewer caught that `evidence_fusion.rs`,
`disposition_workbook_state.rs`, and `game_turn_replay.rs` already assert
most of these, against real pipeline-generated positions, not the
`Arbitrary`-constructed values the contracts-crate fuzz suite uses. This is
exactly the failure mode the plan's own U0 work item 2 warns against ("do
not duplicate an existing target merely because its name is similar") —
the corrected table below is grounded in reading all four targets, not one:

| Invariant | Existing coverage | Gap → U1 work |
| --- | --- | --- |
| **P1** Graph non-mutation | Structural only, in every target: `apply_production` takes `&DesignerDag`, returns a candidate. No target — including `evidence_fusion.rs`, `disposition_workbook_state.rs`, `game_turn_replay.rs` — asserts explicit before/after content-hash equality of the *whole* sequence. Genuinely uncovered. | Add an explicit before/after `DesignerDag` content-hash equality assertion spanning the full board→...→disposition sequence, including on hostile/refused paths. |
| **P2** Replay determinism | **Already asserted, in full, in `evidence_fusion.rs`**: double-call `finalize_bpmn_move_evidence` equality (lines 295-298, 299-310) *and* the rank-order-permutation-invariance sub-clause specifically (`reordered.ranking.reverse()` at line 261, asserted identical result at 295-298) — this is the exact sub-clause the first version of this table called "not present today." `history_belief_state.rs` separately already asserts the same double-call pattern for `project_bpmn_attempt_history` (247-253) and `update_bpmn_design_belief` (262-273), just not yet for `decide_bpmn_game_disposition`. | Only the residual: add the double-call determinism pattern around `decide_bpmn_game_disposition` specifically (not previously exercised anywhere in this crate). The rank-permutation clause does **not** need re-proving — `evidence_fusion.rs` already owns it for `finalize_bpmn_move_evidence`; extending `history_belief_state.rs` should call that boundary the same way it already does, not re-add a redundant permutation test. |
| **P3** Closed-world evidence | **Already fully asserted in `evidence_fusion.rs:273-294`**, against a real `build_bpmn_design_position`-derived position: exactly-one-entry-per-legal-move (273), all move IDs on the position (274-279), all lane/final/probability scores finite (280-288), probabilities sum to 1 within `1e-12` (289-294). Contracts-crate `game_disposition_contract.rs` is genuinely adjacent-not-overlapping (Arbitrary-constructed, not pipeline-derived) but was never the reason this was uncovered — `evidence_fusion.rs` already closes it. | **No new U1 work required for P3 itself.** `history_belief_state.rs`'s own `fused.move_evidence` (from its one `finalize_bpmn_move_evidence` call at line 181) already passes through the same, already-tested function `evidence_fusion.rs` asserts P3 against — extending the target should reuse/assert this already-proven property inline (cheap), not treat it as new design work. |
| **P4** Position-bound decision | **Already fully asserted in `disposition_workbook_state.rs:299-311,329-332`**: real `decide_bpmn_game_disposition` call, `disposition.validate_for_position(&position).unwrap()`, and an explicit assertion every selected move is on `position.legal_moves()`. `game_turn_replay.rs:267-274` additionally fuzzes an off-board-move hostile axis (`chosen_move_off_board`) at the *game-turn-closure* boundary (`GameTurnRecord::new`), a different but adjacent boundary from `decide_bpmn_game_disposition` itself. | **No new U1 work required for the valid-tape half of P4** — reuse `disposition_workbook_state.rs`'s pattern when adding the `decide_bpmn_game_disposition` call to `history_belief_state.rs`. The hostile off-board-move axis at the *evidence* level (as opposed to game-turn-closure level) is still genuinely new — see P5/work item 4. |
| **P5** Fail closed | Partial, per-axis, not composed: `FiniteScore::new` rejects non-finite (`evidence_fusion.rs:220-221` asserts this directly); `history_belief_state` itself already asserts one hostile axis (receipt-count > 64 → `project_bpmn_attempt_history` errors, lines 240-245); `disposition_workbook_state.rs` scenario 7 already covers "stale/unknown focus" (line 194, `unknown_focus`); `game_turn_replay.rs` covers off-board-move at the game-turn-closure boundary; `model_boundary.rs` covers `ResolverBoundaryRefusal::NonFiniteScore` at a different boundary. | New U1 work, narrowed by the above: foreign board hash, omitted/duplicate candidate, off-board candidate *at the evidence-finalisation boundary specifically*, invalid correction reference, exact-match-equivalent-vs-not (see P6 — actually already covered), unresolvable compound span (server-owned, U3 not U1). The stale-focus and off-board-move axes should be *ported* from the sibling targets' existing pattern into the extended target, not redesigned from scratch — see work item 4 below, revised. |
| **P6** Governed text equivalence | **Already fully asserted in `evidence_fusion.rs:327-341`**: compares `"insert after"` vs `"  INSERT   AFTER  "`, asserts identical `move_evidence` — this is P6, proven, against a real position, not the "not covered" claim in the first version of this table. | **No new U1 work required for P6 itself.** `history_belief_state.rs` should reuse the same normalisation-equivalence check on its own fixed `observed_intent` string when it's made variable, rather than treating this as novel design. |
| **P7** Server composition | Out of scope for U1 — belongs entirely to U3, a separate crate, separate decision, separate target (if any). | No U1 work. |
| **P8** Graph/DSL equivalence | Blocked, no bridge exists (confirmed in pre-execution fact-check: no `to_dsl`/`to_source`/`WorkflowSource`-construction path from any `DesignerDag`). | No work until U4 unblocks. |

**Corrected net effect of the ruling**: U1's real incremental work is
smaller than the first version of this receipt stated. Genuinely new:
P1's full-sequence content-hash assertion, wiring `decide_bpmn_game_disposition`
into `history_belief_state.rs` with its own P2 double-call and P4
validate-for-position checks (both *ported* patterns, not new design), and
P5's residual hostile axes (foreign board hash, omitted/duplicate
candidate, evidence-level off-board candidate, invalid correction
reference). P3 and P6 need no new design — `history_belief_state.rs`'s
existing evidence/intent values already flow through the same, already-P3/P6-proven
`finalize_bpmn_move_evidence` boundary; extending the target should assert
those already-proven properties inline rather than re-litigate them. This
still supports the original ruling (extend, don't build fresh) but the
actual diff is narrower than first estimated — worth stating precisely so
U1 doesn't over-build.

## Work item 3 — bounded DAG fixtures and utterance families

**Fixture catalogue**: retain `history_belief_state.rs`'s existing
`graph(shape: u8)` three-way fixture (`shape % 3`): shape 0 = empty DAG
(no legal moves — exercises the "position with zero candidates" edge, must
still produce a valid empty-evidence closure, not a panic); shape 1 =
active 3-node linear DAG (start → `ServiceTask("review")` → end); shape 2
= shape 1 plus an attached timer guard and a second end (5 nodes). All
three are within the plan's 8-node resource envelope (work item 4 of §3),
confirmed generous relative to every existing fuzz fixture in the suite —
`evidence_fusion`/`legal_move_enumeration` use 2–5 nodes, `preview_compilation`
similar. No new fixture shape is required to reach the 8-node ceiling; if
U1's disposition/P4 work needs a fourth shape (e.g. an OR-gateway
named-subset topology, to exercise a genuinely different legal-move shape
than the linear/guarded cases already covered), that is a U1 implementation
decision, not a U0 one — flagged here as headroom, not committed.

Each of the three shapes already produces at least one legal move except
shape 0 by design (that is the point of including it — it is the
"position with zero candidates" case P3's "exactly one entry per legal
move" assertion must handle as the trivial/empty case, not skip).

**Utterance family**: the plan's six families (exact operation, duration,
count, node reference, compound delimiter, abstention) should draw from
the same governed-phrase-corpus discipline `evidence_fusion.rs`'s seed
files already name by shape (`shape-exact`, `shape-duration`,
`shape-count`, `shape-node-reference`, `shape-negative-contrast`,
`shape-abstention`) rather than inventing a new phrase vocabulary — reusing
an already-reviewed shape taxonomy is lower-risk than a new one. Exact
byte-tape encoding of these families onto `history_belief_state`'s existing
tape layout (which today reads `data[0]` for graph shape, `data[1]` for
revision byte, `data[2]` for ranking seed, `data[3..]` for the outcome
loop) is U1's own implementation task per the plan's own two-tier split
(U0 = target contract, U1 = implementation) — U0 confirms the source
vocabulary to draw from and confirms it fits the resource envelope; it does
not fix the exact byte offsets.

## Work item 4 — hostile axis → expected refusal API

**Second correction (found during U1 implementation start, before any
fuzz-target code was written against the claims below):** three more rows
in the original table were wrong in ways only empirical verification of
the actual function bodies caught — not just misclassified as new
(work item 2's blind-review finding), but actually asserting a refusal
that doesn't happen, or missing a dedicated existing target entirely:

- **"Foreign board hash" does not error at `finalize_bpmn_move_evidence`.**
  Read `fusion.rs:632`: `result.board_hash` is never validated against
  `board.board_hash` — the output is unconditionally stamped with the
  real board's hash, the input field is ignored. This axis does not exist
  at this boundary as originally claimed.
- **"Stale/unknown focus" is not a refusal at all.** Read
  `disposition_workbook_state.rs:201-203`:
  `DesignFocus::unknown(GraphElementRef::new("missing-focus").unwrap())`
  is passed to `build_bpmn_design_position` and **succeeds**
  (`.unwrap()`, not `.unwrap_err()`) — it's a valid, typed representation
  of "the referenced element isn't on the position," not an error. This
  was never a hostile axis; dropped from P5 entirely.
- **"Invalid correction reference" already has its own dedicated,
  exhaustive fuzz target**, missed by both the original table and the
  work-item-2 correction: `correction_history.rs` (one of the 6 targets
  omitted from the plan's §2 table, per work item 1) fuzzes every
  correction-chain malformation — none, backward, self-cycle
  (refused at `record_bpmn_attempt` construction, line 186), forward
  (may resolve later), and phantom/missing target — against an
  independent acyclic-graph reference model
  (`reference_valid`, lines 90-111), plus the `MAX_HISTORY_ATTEMPTS`
  resource bound as a separately-asserted second dimension. This is
  exhaustive existing coverage, not a gap.
- **The real `MAX_HISTORY_ATTEMPTS` cap is a named production constant,
  not a fuzz-target-local choice**: `utterance-engine/src/history.rs:19`,
  `pub const MAX_HISTORY_ATTEMPTS: usize = 64`, re-exported at
  `lib.rs:83` and used by both `history.rs`'s own production check and
  `correction_history.rs`. This strengthens (does not change) the
  cap ruling above — 64 is not an arbitrary existing-target choice, it's
  the actual governed bound.
- **`evidence_fusion.rs` already covers two of the three malformed-ranking
  sub-cases**, not zero as the original table implied: read
  `evidence_fusion.rs:222-249` — `malformed == 1` duplicates a candidate
  id (line 225), `malformed == 2` omits one via `.pop()` (line 227), both
  asserted `.is_err()` against `finalize_bpmn_move_evidence` with a P1-style
  legal-moves-unchanged check after refusal (lines 240-247). Only
  **off-board candidate** (injecting a foreign id, never exercised —
  `evidence_fusion.rs` only ever duplicates or removes, never inserts a
  foreign id) remains genuinely uncovered.

| Hostile axis (§4) | Expected refusal, at | Existing precedent |
| --- | --- | --- |
| Off-board candidate (inject a foreign id into the ranking) | `finalize_bpmn_move_evidence`, same `anyhow::bail!` at `fusion.rs:243` that duplicate/omitted already hit | duplicate/omitted already covered (`evidence_fusion.rs:222-249`); **off-board injection is the one genuinely new sub-case** — port the pattern, add the third variant |
| Foreign/stale board revision | `build_bpmn_design_position` — `board.graph_revision != current_graph_revision` → `Err(BpmnBoardError::StaleBoardRevision)`, `bpmn_board.rs:339-344` | **genuinely new** — confirmed via `grep -rl StaleBoardRevision utterance-engine/fuzz` returning zero fuzz-target hits; only production code and its own unit tests reference it |
| Rank-order permutation, identical scores | must **not** refuse — must reproduce the identical canonical result (this is P2, not P5) | **already covered**, `evidence_fusion.rs:260-271,295-298` — port, don't re-add |
| Resource-limited history (> receipt cap) | `project_bpmn_attempt_history` | **already covered**, `history_belief_state.rs:240-245` and exhaustively by `correction_history.rs` |
| Exact-match-equivalent vs. non-equivalent formatting | governed text-resolution boundary, inside `finalize_bpmn_move_evidence` | **already covered**, `evidence_fusion.rs:327-341` — port, don't re-add |
| Invalid correction reference (all schemes) | `record_bpmn_attempt` (self-cycle) / `project_bpmn_attempt_history` (missing/forward/phantom) | **already exhaustively covered**, `correction_history.rs` — no U1 work |
| Foreign board hash | — | **not a real axis at this boundary** — dropped |
| Stale/unknown focus | — | **not a refusal** — dropped |
| Compound delimiter, unresolvable span | `resolve_compound_chain` / `resolve_hypothetical_chain` — server-owned (bpmn-lite-server-designer), not reachable from `utterance-engine/fuzz` | out of scope for U1; belongs to U3 |

**Corrected residual P5 work for U1, final**: exactly two genuinely new
hostile axes — off-board-candidate injection (extend the existing
malformed-ranking pattern with a third variant) and foreign/stale board
revision (new). Everything else in §4's hostile-axis list is either
already covered elsewhere in the crate or was never a real refusal to
begin with.

Non-finite scores are confirmed already rejected by `FiniteScore::new` and
exercised directly in `evidence_fusion.rs` (not `model_boundary.rs`,
contrary to the plan's ambiguous unattributed "the existing boundary
target" phrasing in §4 — `model_boundary.rs` covers a related but distinct
resolver-boundary refusal path via `ResolverBoundaryRefusal::NonFiniteScore`).
Both are real; `evidence_fusion` is the one with the literal assertion. U1
should not re-assert this, only avoid smuggling a non-finite score around
`FiniteScore::new` via unsafe/deserialisation construction, per §4's own
instruction.

**Discrepancy found**: §3 work item 7 proposes a 16-history-receipt cap;
`history_belief_state.rs`'s existing, already-asserted cap is 64 (line
240). Extending the existing target means either lowering its resource
envelope to 16 (shrinking already-passing seed/corpus coverage and
potentially invalidating existing regression inputs, since the target's
`[[bin]]` name and boundary are unchanged) or keeping 64 and treating §3
item 7's "16" as the *new-axis* budget rather than a hard cap on every tape
field. This is a genuine, small conflict between the plan's proposed
envelope and the target being extended — flagged for peer-review ruling at
the U0 gate, not decided here.

---

## Blind peer-review findings and dispositions

An independent reviewer (no prior context on this session) verified this
receipt against the live repo: independently recounted every corpus/
regressions/seeds cell for all 17 targets across both fuzz crates (all
matched exactly), confirmed the `semantic-decision-contracts` sibling-repo
finding by reading `game_disposition_contract.rs` and its `support.rs`
directly, confirmed the `history_belief_state.rs` line citations (one
trivial miscount: receipt said 307 lines, file is 306), confirmed the
PR-smoke and history-cap-conflict claims exactly, and confirmed `git
status` showed no code changes (U0's own constraint held).

**One substantive finding, disposed by rewriting the receipt (not by
argument):** the reviewer rejected the original work item 2 P1–P8 table —
it materially overclaimed the residual gap for P2 (rank-permutation
sub-clause), P3 (closed-world evidence), P4 (position-bound decision), and
P6 (governed text equivalence), all of which are already asserted against
real pipeline-generated positions in `evidence_fusion.rs`,
`disposition_workbook_state.rs`, and `game_turn_replay.rs` — sibling
targets in the same crate that the first pass of this work item never
cross-checked, reading only `history_belief_state.rs`. I verified the
reviewer's specific line-numbered citations directly (re-read
`evidence_fusion.rs:255-350` and `disposition_workbook_state.rs:290-335`
myself) before accepting the finding. Work item 2's table and work item
4's hostile-axis table above are both now the corrected versions; the
originals are not preserved separately since the correction is what
carries forward into U1. This is exactly the failure mode the plan's own
U0 work item 2 warns against, caught before the gate closed rather than
after U1 built on the wrong estimate.

No other discrepancy found; work items 1, 3, and the PR-smoke/cap-conflict
findings in work item 4 were independently confirmed accurate as
originally written.

## Rulings (Adam, this session)

1. **History-receipt cap**: keep `history_belief_state.rs`'s existing,
   already-asserted 64-receipt cap. Plan §3 item 7 amended in place to
   record this rather than the original proposed 16.
2. **Plan-doc text update**: `docs/todo/EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001.md`
   amended in place — §2's coverage table, §3 item 7, and the U1 section's
   Work list all now say "extend `history_belief_state.rs`," not "add
   `deterministic_discovery_pipeline.rs`."
3. **Narrower U1 scope**: accepted. U1's Work section is amended to route
   through the ported-pattern approach (P2/P4 from `evidence_fusion.rs`/
   `disposition_workbook_state.rs`, P3/P6 reused not re-derived) rather
   than the original, wider estimate.

## STOP-gate decision: accepted

Gate U0 is closed. U1 may begin under the amended plan-doc scope above.

No production code has been changed in this tranche.
