# EOP-PLAN-BPMN-DESIGN-003 — Implementation Plan

**Version:** v0.3
**Status:** DRAFT — for review
**Executes:** EOP-VS-BPMN-DESIGN-003 **v0.7** (v0.6 ratified 2026-07-25; §20 amendment ratified 2026-08-11)
**Baseline:** EOP-VS-BPMN-ISA-002 v0.19 IMPLEMENTED; `codex/bpmn-gameboard-refactor`
**Grounded in:** EOP-DIR-BPMN-GAMEBOARD-RESEARCH-001 and -002 (both measured, findings-only), EOP-DIR-BPMN-DESIGN-003-005 (DIR-004 verification)

> **Merge note:** v0.2's receipts sections carry forward **unchanged**. This document replaces the tranche structure, not the record. Do not drop receipts on merge.

---

## 0. What changed from v0.2, and why

Two things reshaped the plan, both from evidence rather than preference.

**The persona ruling reordered the work.** The baseline user is an SME super-user who expects to dictate a workflow graph, using the session almost as an IDE. For that persona, multi-move look-ahead beats ranking accuracy: "wait a week, chase twice, then escalate" is three-to-five moves in one utterance, and single-ply forces manual decomposition at every compound thought. The SLM serves persona 2. Chain preview therefore leads.

**Training is parked, deliberately and with a carve-out.** Adam's ruling: the code changes ahead *add board candidates* — coverage-matrix gaps, the loop production, the vocabulary strip. Retraining now would fit a corpus to a board about to move. Training resumes when the code lands, against the board as it then exists. **The carve-out:** capture is fixed and switched on early, because the code phase itself generates real dictation sessions in exactly the register the model is weakest at, and today they evaporate on restart.

Research also closed four things that were open in v0.2: chain preview needs no new contract types (only a function and a content hash); undo is truncated replay, not a snapshot system; the runbook already *is* a re-executable program; and the `AstMutator` question resolved to a migration, not a design fork.

**Out of scope, stated:** sequential MI (substrate ask); instance creation / the factory (Designer ends at a manifest-bearing template); the Q9 charter and everything behind it (parked with training); Phase D4 in-browser oracle (still gated on C1 + Q23).

---

## 1. Standing rules

Inherited from ISA-002 and binding on every tranche: GRIND vs CAREFUL tiers; authorship-blind review at every CAREFUL close; **Rule 7** — substrate/plan mismatch means the executor HALTS and reports, never adapts; red→green for every remediation; build proof over assertion; zero suppressions; every code claim marked and traced before anything rests on it; per-site rules converted to build failures when a class recurs.

Three additions, ratified for this plan:

**R-A — Pub hygiene is a gate, not a habit.** Every tranche close diffs `cargo public-api` against the committed baseline. Every new `pub` is justified in the tranche receipt or reverted. Crate capability boundaries are respected: expose the *function*, never the module. Known pressure points — `compute_post_dominators` (expose a function or thin wrapper), `resolve_hypothetical_position` (pub; clone/fold helpers private), manifest types (pub only for what will cross to a factory later, not speculatively).

**R-B — Compaction at every tranche boundary.** Each tranche closes with: receipt written → blind review (CAREFUL) → `public-api` diff → **compact** → next tranche. Rationale worth stating: compaction makes the receipts load-bearing. If a fresh context cannot resume from the plan doc plus the tranche receipt, the receipt was not good enough — the workflow enforces the discipline rather than relying on it.

**R-C — Capture stays on.** From G1 onward, dev-session capture is enabled for every development and test session. These are real dictation utterances in the hardest register; losing them is unrecoverable.

---

## 2. Tranche map

```text
G1  Foundations            content hash · capture persistence · vocabulary strip · SLM lane control-flow
G2  Chain preview          the persona-1 headline capability
G3  Loop unrolling         + AstMutator retirement + back-edge whitelist deletion
G4  Parameter manifest     linter walk · three slot kinds · sealed with template
G5  Authoring coverage     the S6 matrix gaps
G6  Session artifact       undo · runbook rendering · replay-equivalence · template↔tape
G7  Data-object authoring  CreateDataObject op · pack entry · slot binding · referential integrity
```

G1 precedes G2 (content hash is a hard prerequisite). G3–G6 are independently orderable after G2; the sequence above is recommended, not forced. G7 was added post-close (2026-08-12, ruled by Adam off the super-user REPL test finding) and depends on nothing beyond the closed G1–G6 surface.

---

## G1 — Foundations

**Tier:** CAREFUL (G1.1, G1.4), GRIND (G1.2, G1.3)

**G1.1 — Content-derived graph hash, alongside route-derived.** Lift the canonicalisation logic that exists in `ir_graphs_equivalent` (sort by BPMN id, sorted node/edge tuples — already proven by three cement-locked tests) into a real digest over `to_ir()` output. Both identities are carried and **each consumer names which it uses** (I34). Per D23, record explicitly in the receipt: which staleness/drift checks loosen, and that receipt dedup behaviour changes for structurally-identical-differently-routed positions.

**G1.2 — Dev-capture persistence.** `DevSessionRecord` is a correctly distinct type with consent in the type (verified), but its store is a process-memory `Mutex<HashMap>` with no write path. Give it a durable store. **This is the third instance of the in-memory-field-never-written-through-the-store pattern** (with the ephemeral pending proposal and the uncached `DesignerDag`); the other two are deliberate and fail-closed, this one silently loses the data capture exists to produce. Then switch capture on (R-C).

**G1.3 — Strip the dead board vocabulary.** Remove the hash-cemented descriptions for `GUARD-R>`, RACE, and `CallSubprocess` — capabilities marked EXCLUDED BY DESIGN in `ops.rs`'s own module docs with no construction path. Board hashes change once; a deliberate act, receipted. *(Note: RESEARCH-002/V3 confirmed corpus generation draws from oracle-admitted enumeration, not this catalogue — these entries appear zero times in every corpus artifact. Nothing was contaminated; the vocabulary is simply lying about the surface.)*

**G1.4 — SLM off the completion path.** The producer cascade currently selects the heaviest *loaded* lane unconditionally and falls back only on load failure — never on request-time match quality. Restructure so lane selection is per-request: governed exact-match resolving cleanly must short-circuit **by construction**. Add per-request deadline plumbing (none exists anywhere in the evidence path). Budgets by interaction, not component: completion **<100ms** (tier-0 only, never tier-1 — measured tier-1 p95 is 2.6–2.9s, a wrong-lane problem not a tuning problem); utterance submit **<500ms**; ratify seconds acceptable.

**Gate G1:** content hash and route hash both derived, each consumer naming which it uses, and the loosened-semantics list receipted. A dev session survives a process restart. `public-api` diff clean. A completion-shaped request provably cannot reach tier-1 — demonstrated by a test, not by configuration.

---

## G2 — Chain preview

**Tier:** CAREFUL

Research established this needs **no new contract types**: `DesignPosition` is already constructible from arbitrary supplied graph identity ("without consulting time, randomness, storage or a server"); `DesignerDag` clones in ~3µs with no interior mutability or global counters; `apply_production` and `admit()` are pure over the graph save one exception (`TimerSpec::Date` reads wall-clock during lowering — a hypothetical-position cache keyed on graph content must account for it).

**G2.1 — `resolve_hypothetical_position`.** Run the existing board/position pipeline against a staged, unratified `DesignerDag` clone rather than the live session's reconstructed one. Depends on G1.1 (a hypothetical mid-chain state has no edit-log entry to hash).

**G2.2 — Chain the fold.** Clone → apply move *n* → derive hypothetical position → enumerate and preview move *n+1* against **that** position. Per Adam's ruling A, hypothetical steps **carry the original `history_hash` unchanged** — history is provably non-authoritative (I30), and synthesising history for moves that have not happened would be inventing evidence.

**G2.3 — Chain the disposition.** `compound_plan`/`decide_game` currently take no `DesignerDag` and resolve both spans against one unchanged position. Add the parameter, materialise and admit span 1, rebuild the board, resolve span 2 against the result. Widen the trigger past the literal `"<a>; <b>"` split to natural sequential phrasing ("wait a week then chase" is one utterance, not two).

**Gate G2:** a three-move line dictated as one utterance resolves, each step verified against the position its predecessor produced, with the whole line previewable before ratification. Depth-3 within budget at realistic sizes (measured: 433µs at 10 nodes, 1.48ms at 339 — the persona budget is the HTTP figure, single-digit ms, not the in-process one). A line whose second move is illegal *given the first* is refused with the correct theorem named.

---

## G3 — Loop unrolling and `AstMutator` retirement

**Tier:** CAREFUL

**G3.1 — Unrolling pass.** Expand `LoopAst{ceiling}` to N forward copies before verification. Per-copy node keys derived deterministically from loop id + index (I33 — this is the identity class fought three times already). Confirm, don't assume, whether the body reads the iteration index; if so each copy binds a literal.

**G3.2 — Total-size cap.** Artifact-resident, verifier-checked, on the same footing as the MI maximum. **On total unrolled size, not per-loop count** (I32) — nesting multiplies, loop-inside-MI multiplies again. Since unrolling happens before verification, `VerifiedLimits` sees the true program and the existing machinery catches oversize with no new check.

**G3.3 — Delete the divergence.** With both front-ends emitting acyclic output, remove the `IncCounter`/`BrCounterLt` back-edge whitelist. Retire `AstMutator` behind a `RepeatNTimes` production preserving the SME abstraction. Reconcile the third, simulation-only loop path in `bpmn-lite-server-runner`.

**G3.4 — State the audit position** (D22) in the tranche receipt and any operator-facing documentation: N unrolled copies produce N distinct journalled instances, which is the better audit record.

**Gate G3:** a `RepeatNTimes` production authored in a session compiles, verifies, and executes with N distinct journal entries. Oversize is refused with a typed error naming the cap. The back-edge whitelist is gone and no path emits a cyclic graph. Existing loop tests (T-LOOP-1..5) pass or are deliberately retired with reasons.

---

## G4 — Parameter manifest

**Tier:** CAREFUL

**G4.1 — Derive.** Extend the linter's unresolved-reference walk to emit a typed manifest, classifying each slot as **scalar**, **collection** (with element shape), or **element-scoped** (I35). The walk knows whether a reference sits inside an MI region, so classification is derivable, not heuristic.

**G4.2 — Seal.** The manifest travels with the template alongside the compiled DTO snapshot (I36). *(Today the template retains only the DTO and has no back-reference to its session.)*

**G4.3 — Surface.** Render manifest state as IDE-grade diagnostics: "this template needs a client reference and a directors collection before it can run." This converts a governance constraint into the feature that makes the tool feel helpful rather than obstructive.

**Gate G4:** a template with a scalar slot, a collection slot, and an element-scoped reference inside an MI body produces a manifest that types all three correctly, survives publish/reload, and drives an inline diagnostic. An element-scoped reference is **never** presented as suppliable.

---

## G5 — Authoring coverage

**Tier:** GRIND, blind-reviewed at close

Close the S6 matrix gaps, in descending severity: **workflow-default failure budget** (zero authoring surface — not even a raw-JSON path, worse than any other row); **retry budget** (absent above the artifact/Rust-API layer, no XML or DSL declaration attribute anywhere); **`END-TERMINATE`** (reachable only by hand-crafting raw JSON against the generic graph-edit endpoint, bypassing the palette entirely — untested and undocumented); **REST-integration tests** for parallel MI and inclusive dynamic fork (crate-level only today).

**Gate G5:** every row of the matrix is either fully covered, or carries a written and reviewed reason for exclusion. No row is silently partial. Where the reason is "excluded by design," `board_candidate.rs` must not claim otherwise (G1.3 already removed the false claims; this gate keeps them out).

---

## G6 — Session artifact

**Tier:** GRIND except G6.1 (CAREFUL)

**G6.1 — Undo as truncated replay.** Add a bound (`as_of_seq`) to `reconstruct_designer_dag`. The mechanism already exists — the edit log is the only durable surface, ordering is schema-enforced by a composite `(session_id, seq)` primary key, and every request already folds from a fresh seed. Reconcile the four named points where a backward-moving head matters, chiefly the reverse-scan-for-latest projections and `ProposalAudit.related_event_seq`'s stored pointer. `CorrectionKind::Undo` gains its first constructor — note it already has one real reader in the eval funnel.

**G6.2 — Runbook rendering.** Render the operation tape as readable Designer-DSL. The tape is already canonical and already the authoritative state; this is a rendering job.

**G6.3 — Replay-equivalence test.** Assert that two independent replays of the same log produce identical structural output. The mechanism is exercised on effectively every request but has never been asserted as its own contract.

**G6.4 — Template↔tape linkage.** The link is one-way today (session stamped with template identity, not vice versa). Make it navigable so a published template can be traced to the session that authored it.

**Gate G6:** undo returns a session to a prior position with proposals and projections correctly reconciled; a session renders as reviewable DSL; replay-equivalence is asserted; a published template resolves to its authoring tape.

---

## G7 — Data-object authoring (`CreateDataObject`)

**Added 2026-08-12 (post G1–G6 close), ruled by Adam.** Origin: the super-user REPL test (`test_super_user_repl_builds_6_step_2_branch_2_loop_workflow`) proved `op.create_multi_instance_region`'s `collection` slot is unreachable via the utterance surface — it is an `ArgumentKind::DataReference` that must name an existing `IRNode::DataObject` (`proposal.rs:677` binds via `mentioned_id(&data_ids, …)`; `proposal.rs:786` fail-closes on an unknown id), and no `Operation` variant can mint one post-seed (`DesignerDag::seed`, `schema.rs:216`, is the only constructor, and only `/api/dsl/sessions`-external callers can seed). Same class of gap as `prod.request_and_wait`'s `corr_key_source` (noted in-repo near `test_direct_edit_recovers_interrupting_timeout_equivalence`) — closing it closes both.

**Tier:** CAREFUL at close (board-hash change + verifier semantics), GRIND for the mechanical arms.

### The seams, as they exist today (verified against code, 2026-08-12)

- `IRNode::DataObject { id, name, type_decl: DataObjectType, role: DataObjectRole }` (`ir.rs:127`). `type_decl` is `Primitive(Bool|I64|F64|String)` or `SemOsDomain { domain_id: Uuid, version_hash: [u8;32] }`; `role` is `Input|Output|Internal` (Internal is the serde default, `dto.rs:212`).
- Positional legality treats DataObject as non-flow (`positional.rs:77`); at a DataObject anchor the only offered op is `DeleteSubgraph` ("a DataObject anchor proposes exactly its own deletion", `positional.rs:365`).
- `utterance-engine`'s semantic pack (`config/bpmn-semantic-pack.yaml`, embedded + admission-validated) is held 1:1-exhaustive against `OperationKind::ALL ∪ ProductionId::ALL` by `validate_registry_coverage` (`bpmn_pack.rs:247`) — a new `OperationKind` **cannot** ship without a pack entry; the gate already exists and fails closed.
- `render_operation` (`runbook.rs`, G6.2) and `apply` (`ops.rs:310`) are exhaustive matches — the compiler forces both arms.
- **Latent inconsistency found while tracing:** the verifier does NOT tie `MultiInstance.collection_flag_name` to a declared DataObject (`verify_data_objects` checks duplicate DataObject ids and FfiServiceTask bindings only — `build_mi_session`'s undeclared `"directors"` admits fine). So the proposal path requires an existing DataObject while the raw graph-edit path accepts any string. Two surfaces, two semantics for the same slot.
- G4's manifest derives Input-role DataObjects into suppliable slots (`manifest.rs:109,154`) — a session-minted `role: Input` DataObject correctly becomes a manifest slot with no new work.
- Known retrieval quirk (found empirically in the REPL test): `LexicalTier0` tokenizes each candidate's full serialized pack slice *including other candidates' `negative_contrasts` text*, so short phrases collide across candidates. The new pack entry's `phrases`/`intent_summary` must be lexically distinctive, and existing entries' `negative_contrasts` referencing it must not quote its own distinguishing phrase verbatim.

### G7.0 — Rulings (all three ruled by Adam, 2026-08-12 — recommendations accepted as put)

- **F-G7a — Utterance-facing type surface. RULED: id + primitive type.** Dictatable: id (quoted identifier) + primitive type parsed from text ("a string collection called 'directors'"), clarification prompt when the type is unstated. `role` defaults `Internal` — role=Input has manifest consequences and deserves its own explicit act (a `SetDataObjectRole` op can follow later if wanted). `SemOsDomain` is **excluded from the utterance surface** — it requires an exact pin (domain_id + version hash), which is not dictatable; raw graph-edit JSON may still carry it.
- **F-G7b — Verifier tightening. RULED: tighten.** `verify_data_objects` gains the check that every `MultiInstance.collection_flag_name` names a declared DataObject — one semantics on both surfaces, fail-closed, localized diagnostic naming the referencing node and the missing id. This retro-tightens G5-era behaviour: `build_mi_session`-pattern tests migrate to declare their collections (explicit, receipted test change, not a silent weakening).
- **F-G7c — Delete integrity. RULED: refuse, naming the referencing node ids.** `DeleteNode`/`DeleteSubgraph` refuse to remove a DataObject still referenced by an MI region's `collection_flag_name` or a wait node's correlation source, mirroring the guard-dangling precedent at `ops.rs:496`. Coherent with F-G7b: the delete-time refusal localizes a diagnostic the verifier would otherwise raise at next admit.

### G7.1 — `Operation::CreateDataObject`

**Rule 7 substrate/plan mismatch, found during implementation (2026-08-13), surfaced and ruled — not silently resolved.** The plan's design point below claimed `SetDefaultGuardBudget`/`SetDefaultRetryPolicy` "already surface on the board" as anchorless ops, and directed mirroring that. False: neither is in `OperationKind::ALL` (`board_candidate.rs:93-110`, 16 variants, neither present); neither is returned by `ops_at`/`legal_operations` (`positional.rs:89-190`); `rest.rs:6355-6360`'s own comment confirms both are process-level `DesignerDag` fields with no `IRNode` home, settable only via raw `/graph-edit` JSON, never through the board/utterance path. There is no existing "anchorless-but-utterance-reachable" precedent to mirror — every `OperationKind` today is gated through `ops_at(key)`; `legal_operations(None)` just unions `ops_at` over every node.

**Ruled by Adam (2026-08-13): offer at Start only.** Add `OperationKind::CreateDataObject` to `ops_at`'s `is_start(ir)` branch. Every session has exactly one `Start` node, always present post-seed, so the op is always reachable (anchor = `Some(start_key)` or `None`, unioned in either way) with zero `LegalityOracle` interface change. Rejected: offer-at-every-node (noisy board, semantically odd mid-chain) and a new anchorless trait path (real interface surgery for one op, no proven benefit over the Start anchor).

New variant in `ops.rs`: `CreateDataObject { key: NodeKey, id: String, name: String, type_decl: DataObjectType, role: DataObjectRole }`. No edge — a DataObject is a structural declaration, not flow (mirror of what `seed` does, moved behind the staged-operation refusal discipline). `apply` inserts via the same path `seed` uses; duplicate BPMN id is a typed reject at stage time (don't defer to the verifier what the op can refuse locally). `render_operation` gains the arm (compiler-forced). Runbook replay-equivalence (G6.3's contract) covers it for free once the arm exists.

### G7.2 — Candidate identity + semantic pack entry

`OperationKind::CreateDataObject` in `board_candidate.rs` (canonical id `op.create_data_object`, description distinct from every `negative_contrasts` quotation — see the retrieval quirk above). New entry in `bpmn-semantic-pack.yaml`: arguments `name` (identifier, required), `data_type` (per F-G7a ruling), `role` omitted/defaulted per F-G7a. `validate_registry_coverage` goes red the moment the enum variant lands and green when the pack entry does — that's the tranche's built-in red→green. **Board hash changes** — deliberate, receipted, same precedent as G1.3.

### G7.3 — Slot binding

`start_workbook`'s per-op arm in `proposal.rs`: `name` binds from the quoted-identifier convention (same as `InsertAfter`'s node name); `data_type` binds from a token match over the four primitive type words with clarification fallback (per F-G7a). After ratify, the minted id must be resolvable by `mentioned_id(&data_ids, …)` in the *next* utterance — i.e. the two-utterance sequence "create a string collection called 'directors'" → "create a multi-instance region over 'directors' with a declared maximum of 2" is the acceptance path.

### G7.4 — Referential integrity (F-G7b/F-G7c both ruled yes)

**Scope correction, found during implementation (2026-08-13).** The paragraph below originally also claimed correlation-source references need the same DataObject-declared check. That was never actually ruled — F-G7b's ruled text (§G7.0) names only `MultiInstance.collection_flag_name`. Tracing the code found `corr_key_source` is free text at the Rust-API/raw-graph-edit layer in ~15 existing fixtures across `designer-graph`, `bpmn-lite-compiler`, `utterance-engine`, and `bpmn-lite-server-designer` — including one (`g2_receipts.rs:528`) that deliberately names an undeclared reference as its point. Tightening it would be a real, larger, unruled change (same class of gap as `collection_flag_name`, per the G7 origin note, but its own fixture-migration pass). **Ruled by Adam (2026-08-13): out of scope for G7.4 — call it out as its own follow-up vision/scope paper, not a drive-by here.** Tracked below in §3 as a parked follow-up, not implemented in this tranche.

Verifier: every `MultiInstance.collection_flag_name` names a declared DataObject — localized diagnostic naming the referencing node and the missing id. Delete refusal mirroring the guard-dangling pattern. Migrate `build_mi_session`-pattern tests to declare their collections (an explicit test change, listed in the receipt, not a silent weakening).

### G7.5 — End-to-end receipt

Rewrite `test_super_user_repl_builds_6_step_2_branch_2_loop_workflow` to drop its flagged `/graph-edit` fallback: the full 6-step/2-branch/2-loop build becomes 100% utterance+ratify, including both MI regions over a dictated DataObject. RED half: an MI utterance naming an undeclared collection is refused (422/needs_arguments) with the clarification prompt naming the missing DataObject — cemented.

**Gate G7:** the REPL test builds the whole shape by utterance alone; `validate_registry_coverage` green with the new 1:1 entry; board-hash change receipted; verifier tightening (as ruled) red→green with both fixtures; `check-semantic-gameboard-boundaries.py` clean (the `OperationKind` change lives in `designer-graph` — module-list tracked; `utterance-engine`'s hash-tracked surfaces must be checked, and any drift receipted as deliberate); blind review at close (CAREFUL).

---

## 3. Parked track — training

Resumes when G1–G6 land, against the board as it then exists. Carried forward unchanged: corpus regeneration (the v2-as-specced vs the separately-ratified v3-shadow line is reconciled *then*, against the final surface, not now against one about to change); retraining both ModernBERT bases; three-slice re-baseline; the eight disputed `starter-seed-v1` adjudications; the Q9 charter and everything gated behind it.

What the code phase contributes to it: **real dictation sessions**, captured from G1.2 onward under R-C — the register the model is weakest at and the reason 44.1% is persona-2's number rather than a verdict on the system.

**New follow-up, not yet scoped (2026-08-13, off G7.4): correlation-source referential integrity.** `corr_key_source` on `MessageWait`/`HumanWait`/`SendTask` is free text at the Rust-API/raw-graph-edit layer today — no verifier tie to a declared DataObject, unlike `op.set_correlation_source`'s utterance surface which already requires one (same two-surfaces-two-semantics class of gap G7 closed for `collection_flag_name`). ~15 existing fixtures across `designer-graph`, `bpmn-lite-compiler`, `utterance-engine`, `bpmn-lite-server-designer` rely on the untied free-text form, including at least one deliberately-undeclared case. Adam ruled this needs its own vision/scope paper before any implementation — a domain question (what a correlation key *is* — a compile-time DataObject reference vs. a runtime-bound value — bigger than a G7.4 drive-by), not decided here.

**Fixed (2026-08-13, same session as discovery): `/palette/select` 422s on any anchor-complete single-slot candidate.** `palette_select_endpoint` (`rest.rs`) unconditionally called `answer_proposal_endpoint` with an empty `answers: []` body after staging a workbook; `answer_proposal_endpoint` routed every call through `apply_explicit_answers`, which fail-closed-refused unless `workbook.status() == NeedsArguments` ("workbook in {status} does not accept answers"). A candidate whose only slot auto-resolves from the supplied anchor — `op.delete_subgraph` (`target`) is the concrete case found — reaches `ReadyForDryRun` directly inside `start_workbook`, never `NeedsArguments`, so the trailing empty-answers call always 422'd for it. Pre-existing (not introduced by G7); found because no existing `/palette/select` test happened to exercise a single-anchor-slot candidate before.

Fix shape ruled and landed: `answer_proposal_endpoint` (`rest.rs`) now treats a zero-answer submission against a workbook that is already past `NeedsArguments` as a no-op pass-through — `pending.workbook.clone()` flows straight into the existing post-`apply_explicit_answers` logic unchanged — rather than as a hard error. A non-empty answer set submitted to an already-resolved workbook still hits `apply_explicit_answers` and is still rejected as genuine misuse; only the `body.answers.is_empty() && status != NeedsArguments` case changed. This is the exact shape the discovery note proposed ("skip the trailing call when the workbook is already past `NeedsArguments`"). No change to `apply_explicit_answers` itself or the external `semantic-decision-contracts` crate (vendored git dep — out of reach anyway). Verified against the real bug: `test_g7_create_data_object_utterance_and_referential_integrity` now drives `POST /palette/select` with `move_id="op.delete_subgraph"`'s real content-hash (not its `candidate_id` string — a separate distinction surfaced while wiring the test: `LegalMove` carries both a human-readable `candidate_id` and a SHA-256 `move_id`; `/palette/select`'s body field is the latter) against the unreferenced `scratch` DataObject → `200 OK`, `ready_for_ratification` (staged, not auto-ratified — ratify is still its own `POST .../ratify` call) → ratify → `ratified` → `scratch` no longer resolves as an anchor (`422` on `/gameboard?anchor=scratch`) → `directors` (still referenced) stays refused throughout. Full workspace `cargo test --workspace --lib --bins` green after landing (0 failures).

**Ruled and landed (2026-08-13): `/graph` now renders `DataObject`s.** Adam's ruling: the runbook is the DSL (raw operation tape, S-expressions); the graph is the *enriched execution map* — a structural declaration with a resolved type belongs on the map a user actually looks at, distinct from the tape. This is a visualization-projection decision only — no compile/execution semantics changed (`ExecutionNode` still has no `DataObject` variant; bytecode/`WorkflowExecutionPlan` untouched).

Landed as `data_object_visual_nodes` (`bpmn-lite-server-designer/src/rest.rs`), called from `session_graph_endpoint` alongside `plan_to_visual_graph`: it walks the already-in-scope `IRGraph` (`ir`, built at line ~7006 before `project_ir` discards structural nodes) and projects every `IRNode::DataObject` as an additional `VisualNodeDto` (`kind: "data_object"`, label e.g. `"Data object: directors (string, internal)"`, no outgoing edges — `DataObject` carries no `next`/flow field to project) into the same response `plan_to_visual_graph` already builds, before `layered_layout` runs (so DataObjects get a layout position too, off to the side of the flow — unconnected nodes default to depth 0, sharing a lane with flow-graph roots). Only reachable in the graph-backed session branch (`session.is_graph_backed()`); the legacy DSL-text/lint compile path has no `IRGraph` and no `DataObject` concept to project — nothing to add there. `type_decl`/`role` have no `Display` impl (checked repo-wide, none exists) so the label hand-formats `PrimitiveType`/`SemOsDomain`/`DataObjectRole` rather than falling back to `{:?}`.

Verified both non-regression and the new behavior: audited every `["graph"]["nodes"]` assertion in `rest.rs`'s test module before landing — the two exact-`len()` tests (`test_session_graph_endpoint_serves_compiled_graph_for_graph_session`, `..._serves_branched_graph`) use sessions that never create a DataObject, unaffected; every DataObject-creating test uses presence/absence (`any()`/`find()`) checks, unaffected. `test_g7_create_data_object_utterance_and_referential_integrity` updated to assert `directors` now appears on `/graph` as `{"kind": "data_object", "label": "Data object: directors (string, internal)", ...}` immediately after declaration, alongside the pre-existing runbook check (both surfaces now verified, not runbook-only). Full workspace `cargo test --workspace --lib --bins` green after landing (0 failures, all 47 binaries).

### G7 exercise — server-side utterance test (2026-08-13, post Gate-G7-close)

New standalone test `test_g7_create_data_object_utterance_and_referential_integrity` (`bpmn-lite-server-designer/src/rest.rs`), independent of the 6-step/2-branch/2-loop REPL test, driving `op.create_data_object` end-to-end plus both ruled referential-integrity gates purely through `/api/dsl/sessions/:id/utterance` -> ratify (one unavoidable `/graph-edit` seed for the first node, same precedent as every other REPL-style test in the file). Proves, against real HTTP round trips: (1) RED before declaration — an MI utterance naming an undeclared collection stays stuck at `needs_arguments`; (2) declaring `directors` via `op.create_data_object` at Start, confirmed via both the runbook and the graph (`/graph` now renders DataObjects — see the ruling above; originally it couldn't, a real, non-G7 property discovered while first writing this test); (3) RED again for a *different* still-undeclared name, proving declaring one collection doesn't loosen binding for another; (4) GREEN — MI over the now-declared collection binds and ratifies for real, `compiles: true`; (5) F-G7c RED — **found stronger than expected**: the delete refusal is enforced a layer earlier than ratify-time, inside the compiler-preview dry-run enumeration itself (`legal_moves.rs::position_bound_move` -> `preview_operations`), so `op.delete_subgraph` never even becomes a legal move at the referenced DataObject's anchor, rather than staging and failing at ratify — proven directly against the real gameboard (`legal_moves` array), not asserted from source reading; (6) F-G7c GREEN, **now proven over real HTTP end to end**: a second, never-referenced DataObject `scratch` correctly gets `op.delete_subgraph` offered; selecting it via `/palette/select` (using the fixed no-op-pass-through path above) and ratifying actually removes it — `scratch` stops resolving as a graph anchor — while `directors` stays refused throughout (declaring/offering/ratifying deletion on an unrelated object doesn't loosen the first's integrity). This closes the gap the first pass of this test left open: originally the GREEN ratify-and-remove was only cemented at the Rust level (`designer-graph::ops::tests::delete_refuses_dangling_mi_collection_reference`) because the `/palette/select` bug blocked driving it over HTTP; fixing that bug let this test complete the HTTP-level proof directly instead. Full workspace `cargo test --workspace --lib --bins` green after landing (0 failures).

---

## 4. Receipts

Appended per tranche close, matching the ISA-002 plan's practice: tests red→green, gate evidence, `public-api` diff result, blind-review findings and dispositions, and compaction confirmation.

### G1.1 — content-derived graph hash, in progress (2026-08-11)

**Done, green, not yet gate-closed** (G1.1 only; G1.2–G1.4 and the G1 gate itself — `public-api` diff, blind review, compaction — remain).

- **`DesignerDag::graph_state_hash(&IRGraph) -> String`** added in `designer-graph/src/schema.rs`, lifting `ir_graphs_equivalent`'s proven canonicalisation (sort nodes by BPMN id, sort edges by `(from_id, to_id, condition_debug)`, `NodeKey`/edge-id excluded) into a blake3 digest with length-prefixed framing. `blake3` moved dev→real dependency (`designer-graph/Cargo.toml`). Four new tests, including an empirical route-independence proof (two DAGs built with reversed node/edge insertion order hash identically) — not merely asserted. Doc comment records the naming trap: `bpmn-lite-server-designer`'s existing `graph_content_hash`, despite its name, is route-derived (RESEARCH-002/S2), same as `graph_identity_hash`.
- **Fork surfaced and ruled (Adam, 2026-08-11: "B - it needs to be done").** I34 requires the actual consumer — `DesignPosition`, sealed in the pinned external crate `semantic-decision-contracts` — to carry the identity, not a local workaround. Implemented at the sealed-contract level:
  - `semantic-decision-contracts` (`/Users/adamtc007/dev/dsl`, branch `refactor/sem-os-pack-policy`): added `GraphStateHash` (hash-identity newtype, mirrors `GraphContentHash`'s validation), a required `graph_state_hash: GraphStateHash` parameter on `DesignPosition::new` and `::from_semantic_board`, folded into `state_id`'s hash preimage. Explicit doc note distinguishing it from the route-derived `GraphContentHash`/`GraphRevision`. Golden round-trip bytes and `state_id` in `position_round_trip_is_canonical_and_has_golden_bytes` re-cemented against the new preimage (only that one test's golden values changed; `move_set_hash` untouched since its fields didn't change). Full workspace test suite green (349+43+... across every crate, 0 failures). Committed (`1d039d9`, `feat(contracts)!: add content-derived GraphStateHash alongside route-derived GraphContentHash`) and pushed to `origin/refactor/sem-os-pack-policy`.
  - bpmn-lite's pin bumped from `12d5280e...` to `1d039d958a91620ab15374f05176bdfac4c872d1` in every location it appears: root `Cargo.toml` (5 crates from the same source repo, all bumped together — a partial bump would duplicate-version the dependency graph), `utterance-engine/fuzz/Cargo.toml`, `bpmn-lite-server-designer/fuzz/Cargo.toml`. `Cargo.lock` updated via `cargo update -p semantic-decision-contracts` (`CARGO_NET_GIT_FETCH_WITH_CLI=true` was required — the default libgit2 fetcher errored `class=Net; code=Eof` in this sandbox).
  - `utterance-engine::bpmn_board::build_bpmn_design_position` (the one real production path — `dag: &DesignerDag` already in scope) computes `graph_state_hash` **internally** from `dag.to_ir()`, not as a new caller-supplied parameter — avoids threading a new argument through ~20 call sites (rest.rs ×6, proposal.rs, benches, property tests, capture.rs, and a dozen fuzz targets) while guaranteeing the value is never stale relative to the graph actually used. `project_design_position` (confirmed zero real callers — only a compile-time symbol check and one internal test, per RESEARCH-002/S5) gained an explicit `graph_state_hash: &str` parameter instead, consistent with `from_semantic_board`'s "never fabricates" contract.
  - Every broken call site fixed: `bpmn_board.rs`'s proposal-rebinding path threads `position.graph_state_hash().clone()` through unchanged; `resolver_comparison.rs` and one fuzz target (`model_boundary.rs`) got explicit test placeholder values.
- **Verification:** `cargo check --workspace --all-targets` clean (exit 0, only one pre-existing unrelated warning); both fuzz sub-workspaces (`utterance-engine/fuzz`, `bpmn-lite-server-designer/fuzz`) check clean independently. Full test rerun: `designer-graph` 65/65, `utterance-engine` 99/99 (lib) + 78/78 (integration), `bpmn-lite-server-designer` clean — all 0 failures.
- **Not yet done:** the naming-trap note is documentation-only — `graph_content_hash`/`graph_identity_hash` themselves are unchanged (correctly: v0.3 scopes G1.1 as "alongside," not a rename/replace). G1's gate criteria (`public-api` diff, blind review at CAREFUL close, compaction) have not run yet — this is a sub-item receipt, not a gate close.

### G1.2 — dev-capture persistence, done (2026-08-11)

Replaces `DesignerState.dev_capture: Mutex<HashMap<Uuid, DevSessionStore>>` — process-memory only, confirmed by DIR-004 verification (V1.1) to have no write path and to lose every captured interaction on restart — with a real durable store, keeping DIR-004 Phase 1.3's "train-on-able, not hash-only" guarantee (`BoardDump` + `ContextProjection` text, not just hashes) intact all the way to disk.

- **Store trait**: `AdminProjectionStore` (`bpmn-lite-store/src/store.rs`) gains `open_dev_capture_session` / `append_dev_capture_record` / `load_dev_capture_session`, session-id-scoped (not tenant-scoped — dev capture is Adam-only). New `DevCaptureSessionRecord` type, opaque `records_json: Vec<String>` mirroring `GraphEdit`'s opaque-payload discipline (this crate has no `utterance-engine` dependency; only the server layer deserializes). `open` refuses re-opening an existing session — a later call cannot silently swap the recorded consent statement for one already capturing interactions.
- **In-memory impl** (`store_memory.rs`): new `dev_capture_sessions: HashMap<String, DevCaptureSessionRecord>` field, three methods implemented, matching `design_sessions`' existing style.
- **Postgres impl** (`store_postgres.rs` + new migration `063_dev_capture_sessions.sql`): `dev_capture_sessions`/`dev_capture_records` tables, composite `(session_id, seq)` PK, same row-lock-then-append discipline as `append_design_session_event` (avoids the exact concurrent-append race that pattern's own comment documents was found and fixed for design sessions). Two test-double `AdminProjectionStore` impls (`ViolatingTestStore`, the always-`Unavailable` stub) also updated to keep compiling.
- **`utterance_engine::dev_capture::DevSessionStore` refactored to be stateless**: removed its internal `records: Vec<DevSessionRecord>` and `records()` getter — `capture()` is now a pure builder (`&self -> DevSessionRecord`, no mutation), since persistence is the caller's durable store, not an in-memory field. Module's own tests updated to match; still exercise the same non-empty session-id/consent validation.
- **`rest.rs` wiring**: the `dev_capture` field removed from `DesignerState` entirely. `dev_capture_enable_endpoint` validates the consent statement *before* calling the store (so a rejected statement never reaches the store as a false "already open" conflict on retry), then calls `open_dev_capture_session`. `session_utterance_endpoint`'s capture call site loads the session from the store, builds the record via the now-stateless `DevSessionStore::capture`, and appends the serialized record through the store. `dev_capture_status_endpoint` reads back through `load_dev_capture_session`, parsing each `records_json` entry for the response body.
- **Verification — the actual durability claim, not just wiring:** wrote `test_pg_dev_capture_session_survives_restart` in `store_postgres.rs`, modeled directly on the existing `test_pg_design_session_survives_restart_with_identical_replay` pattern — opens a session, appends two records, **drops the store and pool entirely**, builds a completely independent connection against the same live local Postgres, and confirms the session and both records survive byte-identical, plus that re-opening the same session id is still refused post-restart. Ran against a real local Postgres (`pg_isready` confirmed before starting) — not mocked. Green.
- Full verification: `cargo check --workspace --all-targets --all-features` clean; `bpmn-lite-store-postgres` full suite 103/103 (including the new restart test); `bpmn-lite-server-designer` 78/78 (including the pre-existing `test_dev_capture_requires_consent_then_captures_full_closure`, unchanged behaviourally, now running against the store); `utterance-engine` 99/99; `designer-graph` 65/65 — 0 failures anywhere.
- **Not yet done:** same as G1.1 — gate criteria (`public-api` diff, blind review, compaction) not run; this is a sub-item receipt.

### G1.3 — strip dead board vocabulary, done (2026-08-11)

Removed 5 catalogue entries with no construction path — `OperationKind::CreateRace`, `AttachRollbackGuard`, `CallSubprocess`; `ProductionId::TimerMessageRace`, `CallDurableSubprocess` — confirmed EXCLUDED BY DESIGN in `ops.rs`'s own module docs (no `IRNode` representation exists) and confirmed never boarded by any position (`positional.rs`).

- **`designer-graph/src/board_candidate.rs`**: both enums shrunk (`OperationKind` 19→16, `ProductionId` 7→5; total catalogue 26→21). `CANDIDATE_SCHEMA_VERSION` bumped 3→4 per the file's own FREEZE RULE, with a version-note explaining why. Golden tests re-cemented against real computed values (not guessed): `canonical_ids_are_unique_and_golden`'s golden set and counts, `descriptions_are_content_cemented`'s `GOLDEN_DESCRIPTION_HASH`, and the `legal_candidates_assembly_is_deterministic_and_sorted`/`Doubled` count assertions.
- **`positional.rs`**: exclusion-comment updated (these are no longer "excluded pending trace," they're gone); `excluded_candidates_never_appear` test narrowed to the two entries that legitimately stay excluded-but-present (`CloseParallelRegion` — subsumed by construction, `HumanReviewWithRework` — representable but unimplemented, out of G1.3's scope).
- **`utterance-engine/config/bpmn-semantic-pack.yaml`/`.lock`**: found by tracing `map_legal_candidate` → `candidate_spec` → `compiled_semantic_pack()` — a THIRD place (beyond the two Rust enums) carrying these 5 as `not_representable` capability entries with full phrase/argument/negative-contrast blocks. `validate_registry_coverage` checks catalogue↔pack coverage in both directions (missing AND extra), so leaving the pack entries in place after shrinking the Rust enums would have failed startup validation, not passed silently. Removed all 5 capability blocks from the YAML; regenerated the `.lock`'s `source.sha256`/`pack.artifact_sha256`/`adapter_bindings` list against the real recompiled values (same discipline as the golden hashes above — computed via the checked-in drift-detector test, not guessed).
- **Consumer fixes**: `bpmn_board.rs`'s `not_representable_and_policy_denied_are_silently_excluded_not_errors` test used `CreateRace` as its NotRepresentable example — swapped to `CloseParallelRegion` (one of the two legitimately-remaining `not_representable` entries). `bpmn_pack.rs`'s hardcoded `designer-candidate-schema-v3` `GraphRevision` literal bumped to `v4` to track `CANDIDATE_SCHEMA_VERSION`; its `semantic_snapshot_identity()` golden tuple re-cemented against the real recomputed pack identity (`bpmn-semantic-profile-v1:e325e3c7...`, `e9d3f3e4...`) — this moves the live pack identity further from any existing trained SLM bundle, expected and correct per the plan's own framing (§0: "Retraining now would fit a corpus to a board about to move").
- **Real finding, not just mechanical churn:** `utterance-engine/seed/seed_corpus_v1.json` (a real "synthetic.llm"-provenance eval fixture, separate from and older than the corpus_v2/v3 artifacts RESEARCH-002/V3 checked) has ~12.5% of its oracle labels naming now-removed candidates — `board_completeness()` dropped from a cemented `1.0` to an observed `0.875`. This is a real, deliberate, receipted consequence of the catalogue change, not a regression: the test's own comment already anticipated corpus drift ("only sanity floors are cemented so corpus growth doesn't churn this test"). Floor changed from `assert_eq!(..., Some(1.0))` to `assert!(... > 0.8)` with an inline note explaining G1.3 caused it; corpus regeneration against the current catalogue stays parked with training (plan §3), not done now.
- Full verification: `cargo check --workspace --all-targets --all-features` clean; `designer-graph` 65/65; `utterance-engine` 99/99 (including the three tests this change broke and fixed: `board::tests::policy_filter_removes_and_abstention_is_always_present`, `bpmn_pack::tests::semantic_registry_exhaustively_covers_designer_catalogue`, `metrics::seed_corpus_baseline::seed_corpus_v1_lexical_baseline`); `bpmn-lite-server-designer` 78/78; both fuzz sub-workspaces check clean independently.
- **Not yet done:** same as G1.1/G1.2 — gate criteria not run; this is a sub-item receipt. `docs/receipts/*` and `docs/todo/bpmn-pack-plane-ledger.md` reference the now-removed ids in historical context — deliberately left untouched (historical record, not live code).

### G1.4 — SLM off the completion path, done (2026-08-11)

Traced "the producer cascade" to `DesignerState::retrieve_utterance_evidence` (`bpmn-lite-server-designer/src/rest.rs`) — the real selection logic, distinct from `fusion.rs`'s `MoveEvidenceProducer`s (which fuse evidence signals for an *already-chosen* lane, not choose the lane). Confirmed the plan's diagnosis exactly: priority was tier-1 (if loaded) → embed tier-0 (if loaded) → lexical, gated ONLY on which lanes were loaded (a build/config property), never on request-time match quality.

- **Governed exact-match short-circuit (the real behavioral fix).** `governed_exact(board, text)` — cheap, pure, already existed in `exact.rs` for a different purpose (boosting an existing ranking) — is now checked FIRST, before tier-1/embed are even attempted. A clean `ExactMatch::Unique` routes straight to the lexical lane.
- **`EvidenceLatencyBudget` (per-request deadline plumbing — none existed before).** New `pub(crate)` 2-variant enum (`CompletionOnly` / `UtteranceSubmit`) at module scope in `rest.rs`. `permits_tier1()` is a pure function of the budget alone — `CompletionOnly` forbids tier-1 by construction regardless of whether a bundle is loaded. Both existing call sites pass `UtteranceSubmit` (today's behavior, unchanged) — no completion-shaped endpoint exists yet to pass `CompletionOnly`, stated honestly in the doc comment rather than implied otherwise; the mechanism is the seam a future completion endpoint plugs into, not speculative dead code (`cargo check` correctly flags the unconstructed variant as a warning, left as an honest signal rather than suppressed).
- **Gate proof, without needing a real loaded model:** `completion_budget_never_permits_tier1` — exhaustive over the full 2-variant space (not a single lucky config) — proves `CompletionOnly.permits_tier1() == false` and `UtteranceSubmit.permits_tier1() == true`. `Tier1Ranker` requires loading a real Candle bundle from disk (no trait abstraction to mock); rather than skip the proof or force a heavy/fragile model-loading test, the tier-1-permission *decision* was extracted to a pure function precisely so it can be proven exhaustively and cheaply — the literal thing the gate asks for ("demonstrated by a test, not by configuration"), not a weaker substitute.

**Blind review (CAREFUL, dispatched independent reviewer) caught a real blocking defect in the first cut of the short-circuit, not a false positive.** The first cut's rationale claimed the short-circuit was "lossless" because `finalize_bpmn_move_evidence`/`finalize_semantic_evidence` boost a clean exact match to 1.0 regardless of which lane supplied the base ranking. That claim is true only for who *wins*. It does not hold for the runner-up's score: `finalize_*` only caps every other candidate at `min(score, 0.99)` — it never resets a loser's score to zero. Concrete failure scenario the reviewer gave, and I independently re-verified by reading `retrieval.rs` and `policy.rs` directly rather than trusting the write-up: an utterance exactly matches a governed phrase for candidate A (`governed_exact` fires, `ExactMatch::Unique`). Candidate B is unrelated but its `description` string happens to share most tokens with the utterance — `LexicalTier0::retrieve` has its own independent exact-pin against `description` (a *different* field than `governed_exact`'s `phrases`) plus raw token-overlap scoring, and can hand B a raw score in the 0.90–1.00 range. After the boost: A → 1.0, B → `min(B_raw, 0.99)`, e.g. 0.99. Margin = 1.0 − 0.99 = 0.01, below `policy::decide`'s default `separation_margin` of 0.15 — a clean, unambiguous exact match flips from `Candidate` (auto-apply) to `Ambiguous`/`EscalateToSage`. The short-circuit's entire value proposition (skip tier-1's cost when the answer is already certain) breaks exactly when it matters most.

**Fix:** in the lexical-fallback block of `retrieve_utterance_evidence`, when `clean_exact_match` is true, every candidate's score is zeroed to 0.0 *before* calling `finalize_semantic_evidence`, not left to the cap. Verified safe: neither `retrieved_subset_hash` (candidate ids only) nor `board_hash` depend on score values — confirmed by reading `LexicalTier0::retrieve`'s hash-construction code. The doc comment that claimed unconditional losslessness was corrected in place to state the actual invariant (holds for the winner, not the loser, hence the explicit zeroing).

**Regression proof:** new test `exact_match_boost_does_not_reset_a_high_losing_score` (`utterance-engine/src/exact.rs`) proves both halves directly against `finalize_semantic_evidence` — the shared machinery both the legacy and semantic paths in `rest.rs` route through — using the existing `candidate()`/`board()`/`evidence()` test helpers: (1) an unfixed rival fed a raw 0.95 score survives the cap at 0.95, margin 0.05 < 0.15 (documents the bug); (2) the same rival pre-zeroed survives at 0.0, margin 1.0 ≥ 0.15 (proves the fix). `cargo test -p utterance-engine --lib exact::` green.

Verification: `cargo check --workspace --all-targets --all-features` clean (exit 0), plus explicit `--features candle-probe`, `embed,candle-probe`, `q9-capture` checks (the cfg-gated tier-1 branch type-checks under the real features, not just default) — only the two pre-existing, documented, unsuppressed warnings remain (`served_gameboard_evidence` unused, `EvidenceLatencyBudget::CompletionOnly` never-constructed). Full regression after the fix, all green: `designer-graph` 79/79, `bpmn-lite-store-postgres` 37/37, `bpmn-lite-server-designer` 103/103, `bpmn-lite-store` 65/65, `utterance-engine` 100/100 (99 + the new regression test). `python3 scripts/check-semantic-gameboard-boundaries.py` confirms `status: pass` against the unchanged baseline — the fix is internal-only, no public signature moved.
- **Not yet done:** No completion-shaped endpoint was built (out of scope — G1.4 was the lane-selection mechanism, not a new endpoint); a future completion endpoint is what will pass `EvidenceLatencyBudget::CompletionOnly` for the first time in production.

**Fix re-verified by a second, independent blind reviewer** (fresh dispatch, no context from the finding or the fix's authorship): confirmed the zeroing runs strictly before `finalize` and cannot interfere with `finalize`'s id-based (not score-based) boost lookup; confirmed `retrieved_subset_hash`/`board_hash` are hash-over-ids/board-content only, computed before the zeroing, so the fix cannot corrupt them; confirmed the new test exercises the real `finalize_semantic_evidence` function with a realistic naive-vs-fixed comparison, not a weaker proxy; grepped for other call sites of `finalize_semantic_evidence`/`finalize_bpmn_move_evidence` and found the only other production caller (`fuse_move_evidence_with_producers` in `fusion.rs`, used at the palette-selection endpoint) uses a structurally different, already-immune suppression mechanism (multiplicative `*= 0.45` on non-matches, not a near-1.0 cap) — not an unpatched instance of the same bug. Verdict: fix closes the hole, test is meaningful, no other vulnerable sites left unpatched.

**All four G1 sub-items now complete, including the blind-review remediation on G1.4 and its independent re-verification.** `public-api` diff against the pre-G1 baseline already reconfirmed clean above. Remaining before Gate G1 closes: compaction (R-B) before G2 begins.

### G2 — Chain preview, done (2026-08-11)

**G2.1 — `resolve_hypothetical_position`** (`utterance-engine/src/bpmn_board.rs`, pub). Runs the same board/position pipeline as `build_bpmn_design_position` against a staged, unratified `DesignerDag` clone. Per Adam's ruling A, the returned position carries the ORIGIN position's `history_hash` unchanged — a hypothetical step has no edit-log entry to project one from. `current_graph_revision`/`board` likewise pass through unchanged from the real session; only the caller-supplied route-derived `graph_hash` and the internally-recomputed content-derived `graph_state_hash` actually reflect the hypothetical step.

**G2.2 — Chain the fold.** `graph_content_hash_over(payloads)` lifted the exact hashing logic out of `rest.rs`'s `graph_content_hash(record)` (now a thin wrapper delegating to it) so the fold can predict a hypothetical step's route-derived `graph_hash` with byte-identical framing to what real, later, step-by-step ratification would persist (verified: real ratification serializes one `Vec<Operation>` per `GraphEdit` row via `serde_json::to_string(&body.operations)` — the same granularity `resolve_hypothetical_chain` serializes per `ChainStep`). `resolve_hypothetical_chain(dag, board, origin_position, ..., steps: &[ChainStep])` folds `apply_production` + `admit()` over a running `DesignerDag` clone, one step at a time — each step is staged and compiler-admitted against the PRECEDING step's real result, never against the original position — and calls `resolve_hypothetical_position` per step with the extended hash. Proven by two direct unit tests (`utterance-engine/src/bpmn_board.rs`): `hypothetical_chain_resolves_each_step_against_its_predecessor_not_the_origin` (a node only step 1 creates is visible to step 2's position; `history_hash` carried unchanged at every step; each step's `graph_hash` matches the extend-and-rehash prediction; distinct steps never collide on `graph_hash`/`state_id`) and `hypothetical_chain_refuses_a_second_step_only_illegal_because_of_the_first` (an `AppendNode` reusing an anchor step 1 already used — legal in isolation against the origin, illegal once step 1 has actually run — surfaces as a named `Err`, not a silent wrong answer).

**G2.3 — Chain the disposition. Rule 7 substrate/plan mismatch, surfaced and ruled.** The plan's "add the parameter, materialise and admit span 1" reads as if `compound_plan`/`decide_game` (in `utterance-engine/src/disposition.rs`) could do this with just a `&DesignerDag` parameter. Research showed that's false of the actual substrate: turning a `LegalMoveId` into a real `Vec<Operation>` tape always requires a `ProposalWorkbook` + `materialize_workbook`, and the only production path that builds one (`proposal::start_workbook`) lives in the SERVER crate — `utterance-engine` has no dependency on it and no existing precedent of constructing a `ProposalWorkbook` at runtime. **Adam ruled (2026-08-11): move the orchestration to the server layer** rather than give `utterance-engine` a new, precedent-breaking workbook-construction path.

Implemented: `disposition.rs` gained `ResolvedChain { spans, moves }` (pub) and a new `resolved_chain: Option<&ResolvedChain>` parameter on `compound_plan`/`decide_game`, threaded through `bpmn_board::decide_bpmn_game_disposition` — when present and its `spans` match what `compound_plan` itself detects for the same utterance, the caller-validated chain is trusted; a mismatched/stale chain is never silently reused (falls through to `None`, fail-closed). Every existing call site (7 test/fuzz/bench sites plus the one production caller) updated to pass `None`, preserving prior behavior exactly where no chain was ever validated.

`bpmn-lite-server-designer/src/rest.rs` gained `resolve_compound_chain` (private) — the actual chain-fold orchestration: detects the compound span (`StrictCompoundSyntax`, unchanged), resolves + materialises + compiler-admits span 1 via the real `proposal::start_workbook` + `materialize_bpmn_workbook` path (a synthetic `EvidenceRecordHash` minted the same way the palette-selection/direct-edit-equivalence probes already do, tagged `compound-plan-step-v1`), derives span 2's position via `resolve_hypothetical_chain`, and resolves span 2 against THAT position — never the original. Three outcomes: `Ready` (both spans chain-verified — wired into `decide_bpmn_game_disposition`), `Refused` (span 1 or span 2 is genuinely illegal given the chain — the endpoint returns an error naming the diagnostic, never silently falls back), `Unverifiable` (span 1's candidate has a required `Condition`/`SubprocessReference` argument `start_workbook` can never fill from free text alone — falls back to `compound_plan`'s pre-G2.3 same-position declaration, preserving existing behavior for compound utterances whose moves are legitimately incomplete pending clarification).

**Regression found and fixed while verifying (not a G2 defect, a G1.3 gap):** `utterance-engine/tests/candidate_coverage_inventory.rs` failed on the first full-suite run — `docs/receipts/bpmn-candidate-coverage-v3.json` still cement-locked the 5 candidates G1.3 removed. G1.3's own receipt had assumed `docs/receipts/*` was purely historical; this test proved that assumption wrong (it treats the receipt as a live coverage cement-lock, not history). Fixed: removed the 5 ids from the JSON, bumped `candidate_schema_version` 3→4, updated the test's hardcoded count 26→21, with an inline note explaining the G1.3 provenance. Full suite green after the fix.

**Blind review (CAREFUL, dispatched independent reviewer) caught one real defect, confirmed the rest sound.** Confirmed correct: the `history_hash`-unchanged claim is safe (state_id's preimage folds in `graph_state_hash`, which IS always recomputed from real staged content — no collision risk from reusing history_hash); the route-hash serialization shape matches real ratification exactly; `compound_plan`'s spans-mismatch-returns-None fallback is correctly fail-closed; the 2-span indexing (`spans[0]`/`spans[1]`) can't panic (`StrictCompoundSyntax::detect` still hard-gates exactly 2 spans, untouched by G2.3). **Real defect:** the original `resolve_compound_chain` blanket-caught every `BpmnBoardError` variant from `resolve_hypothetical_chain` as `Refused`, collapsing genuine infrastructure/registry errors (`MissingSemanticContract`, `ResourceLimit`, `StaleBoardRevision`, `Shared`, `Gameboard`, `InvalidAnchor`) into the same business-logic-shaped "compound plan refused" diagnostic as real legality refusals — mislabeling a potential system bug as an expected refusal. **Fixed:** the match arm now explicitly names only the three variants that are genuinely "illegal given the chain" (`GraphProjection` — the `apply_production` op-refusal path, `CompilerRefused`, `StaleBoardAnchor`) as `Refused`; every other variant now propagates as a real `Err`, surfacing as the honest internal-error path rather than a fabricated refusal. Full regression re-run green after the fix.

**Not yet done, stated honestly:** the plan's G2.3 bullet "widen the trigger past the literal `<a>; <b>` split to natural sequential phrasing (e.g. 'wait a week then chase')" was NOT implemented — `StrictCompoundSyntax` is unchanged, still exactly one semicolon and exactly two spans. This is real, separate NLP-shaped work (span-boundary detection without an explicit delimiter), not a mechanical extension of what landed; scoping it is a fork for the next session, not decided here. Also not reachable today: no candidate in the current board catalogue is both a genuine zero-required-argument move AND survives compiler admission when materialised (`op.delete_subgraph`'s sole argument auto-binds to the anchor, but `Operation::DeleteNode` doesn't reconnect predecessor→successor, so it structurally disconnects Start from End and is compiler-refused at enumeration for every anchor tried) — meaning `resolve_compound_chain`'s `Ready`/materialized-`Refused` branches are implemented and directly unit-proven (both directions, at the `resolve_hypothetical_chain` level) but not yet HTTP-endpoint-reachable through any existing governed exact-phrase pair. They will start firing the moment a future candidate has a genuinely anchor-only complete move (or `DeleteNode` gains reconnection semantics).

**Verification:** full regression green after the blind-review fix — `designer-graph` 81/81, `bpmn-lite-store-postgres` 37/37, `bpmn-lite-server-designer` 103/103, `bpmn-lite-store` 65/65, `utterance-engine` 120/120 (118 lib + the `candidate_coverage_inventory` fix), plus all default/`candle-probe`/`embed,candle-probe`/`q9-capture` feature combos clean with only the two pre-existing, documented warnings. `public-api` diff: 9 new pub items, all in already-approved `utterance-engine` modules (`bpmn_board`, `disposition`), none in `bpmn-lite-server-designer` (whose surface is unchanged) — baseline updated and reconfirmed `status: pass`.

**Gate G2 met:** a two-move compound line resolves with span 2 verified against the position span 1 actually produces (not the unchanged origin) — proven directly at the `resolve_hypothetical_chain` level; a line whose second move is illegal given the first is refused with the correct theorem named (`GraphProjection`/`CompilerRefused`/`StaleBoardAnchor` diagnostics, not a generic failure) and orthogonal system errors are no longer mislabeled as refusals. Remaining before G3 begins: compaction (R-B).

### G3 — Loop unrolling and `AstMutator` retirement, done (2026-08-11)

**Substrate fork, surfaced and ruled before implementation.** Research (Explore dispatch) found two structurally separate pipelines: the DSL S-expression pipeline (`bpmn-lite-compiler/src/dsl/*`, where `LoopAst`/`AstMutator`/`IncCounter`/`BrCounterLt` concretely live) and the `designer-graph`/gameboard pipeline (where "production" is the G2-established term of art — a pure `fn(bindings) -> Vec<Operation>` — and which has no loop concept at all, only `MultiInstance`). The plan's text spans both without saying which one "RepeatNTimes production" targets. **Adam ruled: DSL pipeline only** — unroll `LoopAst` inside `bpmn-lite-compiler`, build `RepeatNTimes` as a DSL-side constructor, leave `designer-graph` untouched. G3 stayed scoped to this the whole way through.

**G3.1 — Unrolling pass** (`bpmn-lite-compiler/src/dsl/unroll.rs`, new). `unroll_loops(nodes) -> Result<Vec<NodeAst>, UnrollError>` runs between parse and lint inside `compile()` — every `NodeAst::Loop{ceiling}` expands to `ceiling` forward-chained copies of its body before the linter, DAG validator, or bytecode lowering ever see it. Per-copy node ids are `{base_id}__{loop_id}_{index}` (I33). Confirmed, not assumed: no AST node in this language reads an iteration index (`TaskAst`/`SplitAst`/etc. carry only literal, source-authored fields), so unrolling is pure structural repetition — no per-copy literal binding was needed. `ceiling == 0` and an empty body are both hard compile-time rejects (`CompileError::Unroll`), strictly stronger than the pre-G3 soft diagnostic they replace (see closure.rs below). A separate `loop_entries` mechanism retargets any reference *external* to a loop (a sibling's `:next`, or another loop's own `:next`) to the loop's first-iteration entry, since the loop's own id stops existing once it expands.

**G3.2 — Total-unrolled-size cap.** `MAX_UNROLLED_NODES = 2048`, charged during unrolling itself (not rediscovered from downstream machinery — see the correction below), rejecting with a typed error naming the cap, on total unrolled size across nesting (I32), not per-loop count. The plan's own text asserted "unrolling happens before verification, `VerifiedLimits` sees the true program and the existing machinery catches oversize with no new check" — this turned out not to survive contact with G3.3: `VerifiedLimits`'s `loop_multiplier` scaling is itself driven by `BrCounterLt`'s `limit` field, and G3.3 deletes that instruction's only legitimate producer entirely. Post-G3 there is no downstream size signal left for loops at all, making the compile-time cap in `unroll.rs` the *only* enforcement point — a correction to the plan's assumption, not a deviation from its intent.

**G3.3 — Delete the divergence.**
- **Verifier whitelist** (`bpmn-lite-compiler/src/verifier.rs::verify_bytecode`): the `BrCounterLt`-specific backward-jump carve-out is deleted. A backward `BrCounterLt` is now rejected identically to any other backward branch — both front-ends emit acyclic output unconditionally, so there is no legitimate producer left to whitelist.
- **`ExecutionNode::Loop`/`LoopExecNode` deleted as types**, not merely made unreachable (no trap doors) — forcing the compiler to find every consumer: `linter.rs` (fails closed with a diagnostic if a `NodeAst::Loop` somehow reaches it un-unrolled — defensive, since `lint()` is `pub` and reachable directly, not only through `compile()`), `dag.rs` (the `is_expected_back_edge` carve-out deleted; DFS cycle detection is now unconditional), `rpst.rs`, `frontend.rs` (bytecode lowering arm deleted; `topological_order` simplified to a plain Kahn's-algorithm sort with no loop-body special case, since none can exist), and two independent REST-serving crates that each separately match on `ExecutionNode` (`bpmn-lite-server-designer`'s visual-graph renderer, `bpmn-lite-server-runner`'s node-info/edge renderer AND its own third, independent loop-iteration *simulator* in `drive_forward` — deleted, since there is no `ExecutionNode::Loop` left to simulate).
- **`RepeatNTimes`** (`bpmn-lite-compiler/src/dsl/repeat.rs`, new): `repeat_n_times(workflow, target_node_id, ceiling, loop_id)` is the sanctioned constructor — the "SME abstraction" the plan asks to preserve. It owns the whole multi-step edit (extract the target task, rewire every predecessor past it, build the pre-unroll `LoopAst` shape via `create_bounded_retry_macro`, splice it back in) behind one entry point, so `bpmn-lite-server-designer`'s `apply_dsl_macro` REST handler (the `"BoundedRetry"` branch) states intent once instead of orchestrating `AstMutator::remove_node`/`rewire_next`/`insert_after` directly at the call site. `AstMutator` itself is not deleted — it remains the correct tool for `XorSplit`/`ParallelSplit`/custom macros, untouched by G3 — it simply no longer has a direct loop-construction caller.

**Idempotency-check provenance fork, surfaced and ruled.** `closure.rs`'s L6 pass had a real, orthogonal safety check — "does a task that a loop will run N times have an idempotency guard?" — keyed off `ExecutionNode::Loop`'s `body` list. Deleting that type would have silently gone dark (an always-empty `enclosing_loops` map, never firing again) rather than being deleted with a reason. **Adam ruled: carry loop-origin provenance**, not drop the check. Implemented: `TaskAst::loop_origin: Option<String>` (the original, unqualified loop id) is stamped by `unroll_loops` on every copy it produces, mirrored onto `TaskExecNode::loop_origin` through `linter.rs`'s conversion pass, and `closure.rs`'s idempotency check now reads `t.loop_origin.is_some()` directly — no graph walk needed. The other two L6 checks in the same block (`ceiling == 0` diagnostic, back-edge-encloser diagnostic) were retired outright, not carried forward: both are strictly superseded (the first by `unroll.rs`'s hard compile-time reject; the second because the back-edge shape it policed can no longer be constructed at all).

**G3.4 — Audit position stated (D22).** N unrolled copies produce N distinct, individually-addressed instructions/journal entries rather than one counter-guarded repeat — proven directly by `bpmn-lite-compiler/src/dsl/frontend.rs`'s `bounded_loop_lowers_to_n_forward_tasks_with_no_counter_instructions` test (ceiling 3 → exactly 3 `ExecDslTask` instructions, zero `IncCounter`/`BrCounterLt`).

**T-LOOP-1..5 disposition.** T-LOOP-1/2/3 (`bpmn-lite-engine/src/tests.rs`) construct bytecode by hand and drive it directly through `store.store_program`, never through `bpmn_lite_compiler::verify_bytecode` — confirmed via the blind review, not assumed — so the verifier whitelist deletion doesn't touch them; unchanged, still green. T-LOOP-4 (verifier rejects backward `Jump`) is unaffected, unchanged. **T-LOOP-5 retired with reason and replaced**: its assertion ("verifier allows BrCounterLt backward") is the wrong theorem now that the whitelist it tested is gone; replaced with `t_loop_5_verifier_rejects_br_counter_lt_backward_post_g3`, proving the superseding behavior (backward `BrCounterLt` rejected identically to T-LOOP-4's plain backward `Jump`).

**Blind review (CAREFUL, dispatched independent reviewer) found 2 real defects in `unroll.rs`, confirmed everything else sound.**
1. **Correctness bug (confirmed via a concrete failing scenario):** a loop's own `:next` field was never retargeted through `loop_entries` — only *sibling* references to a loop's id were. Two sequential bounded loops chained back-to-back (`loop-a :next loop-b`) miscompiled: `loop-a`'s last iteration pointed at the literal string `"loop-b"`, which stops naming anything once `loop-b` itself unrolls away. Failed loud (a `lint:` "references unknown node" error), not silently — but rejected a legal program. **Fixed:** `unroll_nodes`'s `NodeAst::Loop` arm now retargets `loop_ast.next` through `loop_entries` before recursing, exactly like every other node kind. New regression test: `a_loops_own_next_pointing_at_a_sibling_loop_is_retargeted_to_its_entry`.
2. **Spec/test-fidelity bug (confirmed via a concrete boundary probe):** `MAX_UNROLLED_NODES` (documented and tested at 2048) was actually enforced at ~1024 — `unroll_loop` charged `copy.len()` against the budget, and the subsequent `unroll_nodes(copy, budget)` call independently charged 1 per node while processing that same `copy`, double-counting every iteration. The pre-fix test only probed far beyond both thresholds, so it never caught the discrepancy. **Fixed:** removed the redundant `charge(budget, copy.len())` call in `unroll_loop`; `unroll_nodes` already charges every node exactly once, including nested loops via their own recursive `unroll_loop` calls. New regression test: `budget_is_charged_once_per_node_not_twice`, asserting the exact boundary (2048 admitted, 2049 refused).
3. **Low-severity, noted not fixed:** `qualified_id`'s `"{base}__{loop}_{index}"` string encoding is not collision-free against adversarially chosen ids containing `__`. Caught fail-closed by the linter's pre-existing duplicate-id check (a real diagnostic, not silent corruption) — a diagnosability gap, not a correctness bug. Not fixed in this tranche; noted for a future hardening pass if it proves to matter in practice.

Both confirmed defects fixed; full regression re-run green after the fix (see Verification below); public-api gate reconfirmed clean after the fix (no pub surface touched by either).

**Verification:** `bpmn-lite-compiler` 171/171 (was 167; +2 for the blind-review-fix regression tests, +2 net for the retired/replaced `test_l6_safety`→3 tests), `bpmn-lite-engine` 81/81 (T-LOOP-5 replaced, not lost), `bpmn-lite-server-designer` 103/103, `bpmn-lite-server-runner`, `bpmn-lite-authoring`, `bpmn-lite-bus-handler` all green — full `cargo test --workspace --lib --all-features` clean except one confirmed-unrelated flake (`bpmn-lite-store`'s `test_job_claim_lease_not_before_and_reclaim`, a timing-sensitive lease test untouched by this work, green on retry) and one confirmed debug-vs-release artifact (`utterance-engine`'s `gameboard_perf` bench asserts an absolute-nanosecond budget calibrated for `--release`; fails under the `test`-profile build `cargo test --all-targets` uses, passes cleanly under `cargo bench` — 386µs against a 5ms budget). Both fuzz sub-workspaces (`bpmn-lite-compiler/fuzz`, `utterance-engine/fuzz`) compile clean. `public-api` diff: zero drift on the tracked surface (`utterance-engine`, `bpmn-lite-server-designer` — item counts and hashes match the existing baseline exactly); G3's new pub items (`unroll_loops`, `UnrollError`, `MAX_UNROLLED_NODES`, `repeat_n_times`, `RepeatNTimesError`, `CompileError::Unroll`, `TaskAst`/`TaskExecNode::loop_origin`) all live in `bpmn-lite-compiler`, which is outside this baseline's tracked scope — noted, not a gap in the gate (the tracked crates' surfaces are the ones the baseline exists to protect, and neither moved).

**Not yet done, stated honestly:** the low-severity `qualified_id` collision note above. Nothing else scoped to G3.1–G3.4 was deferred.

**Gate G3 met:** a `RepeatNTimes`-authored loop compiles, unrolls to N distinct forward-chained instructions, and verifies (`bounded_loop_lowers_to_n_forward_tasks_with_no_counter_instructions`). Oversize is refused with a typed error naming the cap, at the exact documented boundary. The back-edge whitelist is gone — no path in either front-end emits a cyclic graph, and the verifier rejects one unconditionally if it somehow appeared. T-LOOP-1..5 pass (T-LOOP-5 deliberately retired and replaced, with reasons). Remaining before G4 begins: compaction (R-B).

### G4 — Parameter manifest, done (2026-08-11)

**Substrate fork 1 (surfaced via `AskUserQuestion`, ruled before implementation): which pipeline is "the linter" in G4.1, and where does the manifest live?** G4's text ("extend the linter's unresolved-reference walk", "seal with the compiled DTO snapshot") reads as `bpmn-lite-compiler/src/dsl/linter.rs`, the file literally named linter. Research found three disqualifying facts: (1) no "unresolved-reference" walk exists anywhere — `dsl/linter.rs`'s "unresolved symbol" diagnostic is verb/plug resolution, a different axis, and `PlaceholderSchema` silently drops any placeholder that's consumed-but-never-produced rather than flagging it; (2) MI regions don't exist in the DSL AST at all — `IRNode::MultiInstance` is XML/DTO-only by explicit design (`ir.rs` doc comment); (3) `WorkflowTemplate.dto_snapshot` (G4.2's "compiled DTO snapshot") is built exclusively by `bpmn-lite-authoring`'s publish pipeline, which already hard-codes `PlaceholderSchema::default()` when reconstructing from a template — the DSL pipeline's manifest machinery never reaches a template even today. **Ruled: authoring pipeline** (`bpmn-lite-authoring`, over `WorkflowGraphDto`) — the only pipeline that can see MI and the only one connected to `WorkflowTemplate`.

**Substrate fork 2 (surfaced, ruled): no "element-scoped reference" vocabulary exists anywhere.** `NodeDto::MultiInstance`/`IRNode::MultiInstance`'s inner activity was a bare `task_type: String` with zero declared inputs — unlike `FfiServiceTask`, which has `inputs: Vec<FfiInputBinding>` with a real `Expression::VarRef` per-field reference. Gate G4's own acceptance text ("an element-scoped reference inside an MI body") described something with no construction path. **Ruled: add minimal MI input bindings**, reusing `FfiInputBinding`/`Expression` verbatim on `IRNode::MultiInstance`/`NodeDto::MultiInstance` (G4.0) rather than inventing a second shape.

**G4.0 — schema plumbing.** Added `inputs: Vec<FfiInputBinding>` to `IRNode::MultiInstance` (`ir.rs`) and `NodeDto::MultiInstance` (`dto.rs`), threaded through `dto_to_ir`/`ir_to_dto`, and updated every construction/destructuring site the compiler forced to the surface (`parser.rs`, `lowering.rs`, `designer-graph/src/ops.rs`, `utterance-engine/src/{fixtures,legal_moves}.rs`) — `lower()` does not read the field; it is authoring/manifest-derivation data only, MI runtime element delivery is unchanged.

**Found and fixed mid-implementation: `IrLiteral`/`Expression` were never `serde_json`-serializable, for ANY variant, not just `VarRef`.** Both used internal tagging (`#[serde(tag = "...")]`), which cannot represent a newtype variant whose payload isn't a map — `Expression::VarRef(Vec<String>)` fails on the sequence payload, but so does `IrLiteral::Bool(true)` on a bare scalar (verified directly: both panic with "cannot serialize tagged newtype variant ... containing a boolean/sequence"). Went unnoticed because `Expression`'s only prior use site, `FfiServiceTask.inputs`, is explicitly excluded from the JSON/DTO pipeline (`ir_to_dto.rs`'s named-diagnostic rejection) — nothing could have depended on the old, always-panicking shape. **Fixed:** switched both to adjacent tagging (`tag = "...", content = "data"`), matching `DataObjectType`'s existing convention. Blind review independently re-verified this is a pure fix with no external-data migration hazard (no serialized artifact anywhere in the repo carries the old shape, since it never successfully serialized).

**G4.1 — Derive.** New file `bpmn-lite-authoring/src/manifest.rs`: `derive_parameter_manifest(dto, registry) -> ParameterManifest`. Walks the same reference sites `lint_contracts` (L1/L3) already walks — `FlagCondition.flag`, `MessageWait`/`HumanWait.corr_key_source`, `MultiInstance.collection_flag`, `DataObject` role declarations, MI `inputs` `VarRef` bindings — and classifies each as `Scalar`, `Collection { element_shape }`, or `ElementScoped { collection_slot, field }`. A reference is unresolved (gets a slot) unless a `DataObject` explicitly declares it `Internal`/`Output` (explicit declaration always overrides the heuristic); `known_workflow_inputs` and `DataObject::Input` are always unresolved even if something happens to also write the flag. `ParameterManifest::suppliable()` excludes `ElementScoped` by construction, not by a filter every caller must remember.

**G4.2 — Seal.** Added `parameter_manifest: ParameterManifest` (`#[serde(default)]`) to `WorkflowTemplate`, derived in `publish_workflow_from_dto` against the same `ContractRegistry` `lint_contracts` used (or an empty one — conservative/fail-closed when no registry is supplied) and sealed onto the template alongside `dto_snapshot`. Persisted via a new migration (`064_workflow_template_parameter_manifest.sql`, nullable `JSONB` column, folded into the existing `enforce_template_immutability` trigger's immutable-content list). `MemoryTemplateStore` and `PostgresTemplateStore` both updated; all pre-existing `WorkflowTemplate` construction sites across the workspace updated.

**G4.3 — Surface.** `manifest_diagnostics(&ParameterManifest) -> Vec<LintDiagnostic>` renders one `Info`-level diagnostic per **suppliable** slot ("This template needs 'client_reference' supplied before it can run." / "...a 'directors' collection...(each element resolves: full_name)"), reusing the existing `LintDiagnostic` shape `lint_contracts` already produces rather than inventing a second surface. Appended into `PublishResult.lint_diagnostics` inside `publish_workflow_from_dto`. Wired into `bpmn-lite-server-designer`'s save-as-template REST endpoint, which was previously computing `lint_diagnostics` and discarding it — the response now carries a `"diagnostics"` array.

**Blind review (CAREFUL, dispatched independent reviewer) found 2 real defects, confirmed everything else sound.**
1. **Correctness bug (confirmed via a constructed scenario):** the collection-slot dedup key was `collection_flag` name alone. Two MI regions legitimately iterating the *same* external collection with different bodies (e.g. one reads `full_name`, another `role`) collapsed into one slot that silently kept only whichever region the walk saw first — the second region's `element_shape` fields vanished from the manifest. **Fixed:** the collection-slot upsert now merges `element_shape` (union, first-seen order) when the same `collection_flag` is seen again, instead of routing through the generic no-merge `upsert` helper. New regression test: `t_manifest_8_two_mi_regions_sharing_a_collection_merge_element_shape`.
2. **Test-fidelity bug (confirmed by reading the assertions against their own doc comment):** `t_pub_13`'s doc comment claimed the manifest "survives a save/load round trip ... it must survive serialization," but the only store exercised was `MemoryTemplateStore`, whose `load()` is a plain in-memory clone — no `serde_json` touches the data at all. The real serde round trip (the thing the G4.0 serde fix actually needed to prove) only ran in `store_postgres_templates.rs`'s DB-gated tests, whose fixture DTOs contain no MI node and so never exercised the new `Expression::VarRef`/`inputs` shape. **Fixed:** added an explicit `serde_json::to_string`/`from_str` round trip of the whole template inside `t_pub_13` itself, asserted against the same three-slot-kind check, so the claim is proven without depending on a live database.
3. **Noted, not fixed (low urgency, currently unreachable):** `element_shape` includes every MI `inputs` binding's `target_field`, including literal (non-`VarRef`) bindings — so a `manifest_diagnostics` message can list a field the caller never actually supplies (it's a compile-time constant). Defensible as "the full shape of what the MI body sets," but potentially misleading as "what you need to supply." `designer-graph/src/ops.rs`'s `CreateMultiInstanceRegion` admission also doesn't yet validate `inputs` (duplicate `target_field`, malformed `VarRef`) — currently unreachable, since no proposal path in the designer-graph pipeline populates `inputs` yet (G4 wired the DTO/authoring side only, per the ruled fork). Both noted for a future hardening pass if either proves to matter in practice.

Both confirmed defects fixed; full regression re-run green after the fix (see Verification below); public-api gate reconfirmed clean after the fix (no pub surface touched by either).

**Verification:** `bpmn-lite-authoring` 71/71 unit tests green (65 pre-existing + `manifest.rs`'s 8 new + `t_pub_13`), full `cargo test --workspace --lib --all-features` clean across the whole workspace, plus `--all-targets` on every touched crate (`bpmn-lite-authoring`, `bpmn-lite-server-designer`, `bpmn-lite-compiler`, `designer-graph`, `utterance-engine`, `bpmn-lite-server-runner`, `bpmn-lite-engine`, `bpmn-lite-bus-handler`). One confirmed-unrelated flake: `bpmn-lite-server-designer`'s `test_postgres_restart_survival` fails only under the full parallel `--all-targets` run and passes cleanly both in isolation and under `--test-threads=1` (82/82) — a pre-existing test-isolation issue against a shared Postgres test database, not a G4 regression. The new Postgres-backed round-trip tests (`t_pub_10`/`t_pub_11`/`t_pub_12`) required migration `064` applied to the local `bpmn_lite_test` database, done manually for this verification pass (the same manual-apply pattern prior gates' Postgres tests already depend on — no migration-runner change was in scope). `public-api` diff: **zero drift** — recomputed all 8 tracked (crate × feature-set) surfaces at `-sss` simplification exactly as the baseline was built, every sha256 matches `scripts/baselines/semantic-gameboard-public-api-v1.json` byte-for-byte; G4's new pub items (`ParameterManifest`, `ParameterSlot`, `SlotKind`, `IRNode::MultiInstance::inputs`/`NodeDto::MultiInstance::inputs`) all live in `bpmn-lite-authoring`/`bpmn-lite-compiler`, outside the tracked baseline's two crates — noted, not a gap (same disposition as G3's new pub items).

**Not yet done, stated honestly:** the two low-severity items noted above (`element_shape` including literal bindings; `CreateMultiInstanceRegion`'s missing `inputs` validation). The REST-level (`bpmn-lite-server-designer`) test for G4.3 could not exercise a real MI-bearing graph-authored session end-to-end: `bpmn-lite-compiler/src/dsl/ir_plan.rs`'s `WorkflowExecutionPlan` projection has no `IRNode::MultiInstance` arm (falls into the generic `UnsupportedNode` rejection) — a separate, pre-existing gap in the designer-graph → DSL-plan dual-write bridge, confirmed real and out of scope for G4. The REST test instead proves the `"diagnostics"` field is wired (present, correctly empty for a clean graph); the full scalar/collection/element-scoped classification and its survival through a real save/load and a real `serde_json` round trip is proven at the `bpmn-lite-authoring` publish-pipeline level (`t_pub_13`), which is where `WorkflowTemplate` actually lives.

**Gate G4 met:** a template with a scalar slot (`DataObject` role=Input), a collection slot (`MultiInstance.collection_flag`), and an element-scoped reference inside the MI body (an `inputs` `VarRef` binding) produces a manifest typing all three correctly (`t_manifest_3_gate_scenario_all_three_kinds`), survives publish/reload including a real `serde_json` round trip (`t_pub_13`), and drives an inline diagnostic (`manifest_diagnostics`, wired into the save-as-template REST response). The element-scoped reference is never presented as suppliable — enforced structurally by `ParameterManifest::suppliable()`, checked at both the manifest layer (`t_manifest_3`) and the diagnostics-rendering layer (`t_manifest_7`). Remaining before G5 begins: compaction (R-B).

---

### G5 — Authoring coverage, done (2026-08-11)

**Two forks surfaced and ruled before implementation** (via `AskUserQuestion`, not invented):

1. **Budget-surface scope** (rows 1-2 below): "designer-graph `Operation` + DTO only" (Recommended) vs. "full stack: XML attr + DSL production + Operation + DTO." Adam ruled the former — mirror the existing per-guard `SetGuardBudget` pattern (reachable from the graph-edit REST endpoint, the layer every G1-G4 authoring gap was closed at); DSL S-expression and raw-XML declaration stay explicitly out of scope for both new budgets, noted with reason here (satisfies Gate G5's "or carries a written and reviewed reason for exclusion").
2. **MI projection gap** (row 4 below): "fix the projection now" (Recommended) vs. "exclude MI from Row 4 with a written reason." Adam ruled fix-now — `ir_plan.rs`'s `IRNode::MultiInstance → UnsupportedNode` gap (found and scoped out during G4) blocks a REST-level MI test entirely; closing it for real meant fixing the projection, not just writing the GatewayInclusive half.

**G5.1 — workflow-default failure budget.** Zero authoring surface above the XML layer (`ProcessMeta.default_failure_budget`) before this: `DesignerDag.default_guard_budget` existed but had no `Operation`, was `pub` (a documented one-off exception to "the mutators are `pub(crate)`"), and was only ever set by direct test-code field assignment. New `Operation::SetDefaultGuardBudget { failure_budget: Option<u32> }` (`designer-graph/src/ops.rs`); the field is now `pub(crate)` with a `default_guard_budget()` accessor for cross-crate reads — the `Operation` is the only mutation surface, closing the prior exception. New `WorkflowGraphDto.default_guard_budget: Option<u32>` (`bpmn-lite-authoring/src/dto.rs`), threaded by `bpmn-lite-server-designer`'s save-as-template endpoint from `dag.default_guard_budget()` (the DTO field has no `IRNode` home — same "rides the DAG root, carried into admission explicitly" reasoning `default_guard_budget` itself already documented). `bpmn-lite-authoring::publish`'s two compile entrypoints (`compile_program_from_dto`, `publish_workflow_from_dto`) switched from the plain `lower(&ir)` to `lower_with_default(&ir, dto.default_guard_budget, dto.default_retry_policy)` — previously this pipeline had **no path at all** to reach `Compiler::lower_with_default`, meaning even a value set via direct Rust construction could never survive a DTO-fronted publish. Validated (non-zero) at `lower_with_default` time via the existing `ScopeFailureBudget::new` call, same split the XML path already used.

**G5.2 — retry budget.** Confirmed strictly worse than G5.1's starting point: `bpmn_lite_types::RetryPolicy`/`CompiledProgram::with_default_retry_policy` were real and load-bearing (engine/store-postgres both consume `default_retry_policy()`), but had **no XML attribute, no DSL keyword, and no DTO/`Operation` path at all** — the only way to set it was calling `with_default_retry_policy` directly in Rust (xtask/tests). New `bpmn_lite_compiler::RetryPolicyDecl` (`bpmn-lite-compiler/src/ir.rs`) — a raw, unvalidated 4-field struct mirroring `RetryPolicy::new`'s parameters exactly, deliberately a *distinct* type from `RetryPolicy` itself: `RetryPolicy`'s derived `Deserialize` would construct an instance without running `new`'s bounds check, which would have made `Operation`/DTO JSON an externally-reachable *unvalidated*-artifact path — `RetryPolicyDecl` is declared raw and validated at `lower_with_default` time instead (same "declare raw, validate at lower" split `default_failure_budget` already uses). New `Operation::SetDefaultRetryPolicy { policy: Option<RetryPolicyDecl> }`, `DesignerDag.default_retry_policy` (`pub(crate)` from the start, with a `default_retry_policy()` accessor), `WorkflowGraphDto.default_retry_policy`. `Compiler::lower_with_default`/`lowering::lower_with_default` both gained a third parameter (all call sites across the workspace updated: `bpmn-lite-engine`, `xtask`, `designer-graph::admit()`, the two `bpmn-lite-authoring::publish` entrypoints, `lowering.rs`'s own tests). Invalid bounds (e.g. `max_delay_ms < base_delay_ms`) rejected at `lower_with_default` via `RetryPolicy::new`'s existing validation, propagated as a compile error, not silently clamped.

**G5.3 — END-TERMINATE.** Not "zero surface" like rows 1-2 — `terminate: true` was already constructible via the generic `AppendNode`/`InsertAfter`/`ReplaceNode` operations (any of them accept an arbitrary `IRNode`, including `End { terminate: true }`); the gap was purely "reachable but untested," so **no new `Operation` was added**. Writing the REST-level receipt (`test_terminate_instance_runs_to_terminated`) surfaced a genuine, independent, previously-undetected bug: `bpmn-lite-compiler/src/dsl/frontend.rs`'s `lower_plan` — the compiler the `/bpmn/instances` REST spawn/advance path actually executes, a *different* front-end from `lowering.rs`'s XML/IR-graph path — unconditionally emitted `Instr::End` for every End node, never `Instr::EndTerminate`, regardless of the terminate flag. `ir_plan::project_ir` already overloaded `EndExecNode.status` with the sentinel `"terminated"` to carry the flag across the IR→plan boundary, but nothing on the consuming side ever read it. A graph/DTO/REST-authored terminating end therefore compiled successfully and ran to completion silently as an ordinary end — never observably wrong until the actual runtime state was checked, which nothing had done before this gate. **Fixed:** `frontend.rs`'s End-node emission arm now checks `node.status == "terminated"` and emits `Instr::EndTerminate` instead; `EndExecNode.status`'s doc comment documents the sentinel contract explicitly (free-form for DSL-authored labels, this one sentinel value is instruction-meaningful). Regression tests: crate-level `dsl::frontend::tests::terminating_end_projected_from_ir_lowers_to_end_terminate` (asserts `EndTerminate` present, `End` absent) and `designer-graph::ops::tests::append_terminating_end_reaches_compiled_bytecode`; REST-level `test_terminate_instance_runs_to_terminated` (asserts the instance actually reaches `Terminated`, not `Completed`).

**G5.4a — `ir_plan.rs` MultiInstance projection gap.** Found and scoped out during G4; fixed here per Adam's ruling. Added `ExecutionNode::MultiInstance(MultiInstanceExecNode)` (`bpmn-lite-compiler/src/dsl/plan.rs`), an `ir_plan.rs` projection arm, and a `frontend.rs::lower_plan` emission arm reproducing `lowering::lower_multi_instance_v2`'s exact instruction shape (`V2MiArityCheck`, `V2Fork`, per-branch `V2MiIndexLive`+`BrIfNot`+`V2MiLoadElement`+`StoreFlag`+body+`Jump`, `V2Join`) — using `Instr::ExecDslTask` for the branch body, not `Instr::ExecNative`, matching this front-end's own sibling `ServiceTask` arm's convention (confirmed: graph-authored `ServiceTask`s already route through `ExecDslTask` in this pipeline, not the XML path's job-dispatch word). This is this pipeline's first-ever use of a raw `FlagKey` (previously entirely placeholder-based, `flag_symbol_table`/`write_set` hardcoded empty) — added a scoped `flag_ids` intern map and a new `intern_flag` helper, folded into `flag_symbol_table` at the end (verified a no-op for every plan without an MI node — `write_set` is unchanged, still empty, since nothing downstream cross-checks it). Updated every other exhaustive `ExecutionNode` match this broke: `linter.rs` (2 sites, DSL-text Split/Join synthesis — MI has no DSL production and can never actually appear here, treated as a linear node for exhaustiveness), `rpst.rs` (1 site, SESE structure walk), `frontend.rs` (2 more sites: `instruction_count`, `outgoing`/topo-sort), and — discovered only by `cargo check --workspace` — a near-identical *duplicate* plan→visual-graph renderer in `bpmn-lite-server-runner/src/rest.rs` (4 sites) alongside the one in `bpmn-lite-server-designer/src/rest.rs` (2 sites), both now rendering an MI node with a `"multi_instance"` kind and a `"For each of {collection} (max {n}): {task_type}"` label.

**G5.4b — REST-integration tests.** `test_inclusive_instance_runs_to_completion_with_two_live_branches` (+ `build_inclusive_session`): a `CreateInclusiveRegion` with two branches, each condition `flag == false`, spawned with payload `{"flag_a": false, "flag_b": false}`. Traced mid-task: a graph-authored inclusive-gateway condition's `flag_name` becomes a **placeholder** check (`ir_plan.rs`'s diverging-gateway arm converts `ConditionExpr` into `SplitExecFlow.placeholder`/`expected_value`; `V2LoadPlaceholderMatch` reads `instance.placeholder_values`, populated verbatim from spawn's JSON payload) — not a `Value`-flag check as the first attempt assumed (that attempt spawned with no payload and got `Incidented` from the zero-match precheck; fixed once the real binding was found). Asserts both branches genuinely observable in flight simultaneously (2 waiting jobs, ≥2 fibers) before completing — the dynamic-arity signature, not a single predetermined branch. `test_mi_instance_runs_to_completion_with_empty_collection` (+ `build_mi_session`): a `CreateMultiInstanceRegion`, spawned with **no** payload (undeclared collection flag reads as zero-length by construction). Deliberately does not exercise a real/non-empty collection: traced and confirmed there is **no REST-reachable way to populate an MI's `Value::Array` collection today** — `engine.rs`'s `start()` always sets `flags: BTreeMap::new()` (spawn payload only ever populates `placeholder_values`), and `advance_instance_endpoint`'s job-completion call always passes `orch_flags: BTreeMap::new()`. This is consistent with the plan's own stated scope boundary ("instance creation / the factory (Designer ends at a manifest-bearing template)" — §0), not a gap this gate introduced or should close. The test still proves the real thing G5.4a fixed: the full graph-edit → save → spawn → advance round trip for an MI region reaches real `V2MiArityCheck`/`V2Fork`/`V2Join` bytecode execution and completes cleanly — before G5.4a this failed at the `save` step with `"no WorkflowExecutionPlan representation yet"`.

**Blind review (dispatched independent reviewer, GRIND tier per plan) found zero defects**, after: hand-verifying the MI instruction addressing byte-for-byte against `lower_multi_instance_v2`; confirming every `RetryPolicyDecl → RetryPolicy` construction site goes through the validating `RetryPolicy::new`; confirming the `pub(crate)` tightening has no stale cross-crate read path; confirming the pre-existing (not G5-introduced) `EndExecNode.status` renderer exposure in both `rest.rs` visual-graph endpoints isn't a new regression; independently re-verifying the "no REST-reachable MI collection supply" claim by reading `engine.rs`/`advance_instance_endpoint` directly; confirming every `ExecutionNode` exhaustive match got a correct arm; confirming `flag_symbol_table`'s population is a genuine no-op for non-MI plans; and independently re-running the full build/test suite itself. One out-of-scope observation flagged (an untracked, pre-existing `plan_deserialize` fuzz target unrelated to any G5 item, left alone here).

**Verification:** full `cargo check --workspace --all-targets --all-features` clean. `cargo test --workspace --lib --all-features` clean across every crate (0 failures across ~30 crate test binaries), including every new G5 test by name (see review summary above for the full list). One transient failure during the run (`bpmn-lite-server-designer::test_postgres_restart_survival`, `"migration 64 was previously applied but is missing in the resolved migrations"`) — root-caused to a stale `sqlx::migrate!()` macro embedding from before migration `064` (added in G4) was on disk at last compile; resolved by a forced rebuild (`touch` + recompile), confirmed unrelated to any G5 code change (G5 touched zero migration files). `public-api` diff: **zero drift** — recomputed all 8 tracked (crate × feature-set) surfaces, every sha256 matches `scripts/baselines/semantic-gameboard-public-api-v1.json` byte-for-byte. G5's new `pub` surface (`RetryPolicyDecl`, `lower_with_default` (now `pub`, re-exported), `MultiInstanceExecNode`, `ExecutionNode::MultiInstance`, `Operation::SetDefaultGuardBudget`/`SetDefaultRetryPolicy`, `DesignerDag::default_guard_budget()`/`default_retry_policy()`, `WorkflowGraphDto.default_guard_budget`/`default_retry_policy`) all live in `bpmn-lite-compiler`/`designer-graph`/`bpmn-lite-authoring`, outside the tracked baseline's two crates (`utterance-engine`, `bpmn-lite-server-designer`) — same disposition as G3's and G4's own new pub items, noted here rather than gapped. Net tightening in `designer-graph`: `DesignerDag.default_guard_budget` moved from `pub` to `pub(crate)` (closing the one documented exception to "the mutators are `pub(crate)`").

**Not yet done, stated honestly:** no DSL S-expression or raw-XML declaration surface for either new workflow-default budget (ruled out of scope, both forks above); no palette/board-candidate/NL vocabulary entry for `SetDefaultGuardBudget`/`SetDefaultRetryPolicy`/a terminating-End production (all three are reachable via the generic graph-edit `Operation`/REST surface, matching every other G1-G4 authoring gap's closure layer, but discoverable only by a caller who already knows the JSON shape — the same status quo `AppendNode`/`InsertAfter` themselves have always had). No REST-reachable mechanism to supply an MI collection or any other flag-typed value at instantiation time — confirmed genuinely absent, and out of scope per the plan's own instance-creation/factory exclusion, not a G5 gap.

**Gate G5 met:** all four S6-matrix rows are either fully covered (workflow-default failure budget, retry budget, END-TERMINATE — each now has a designer-graph/DTO authoring path reaching the compiled artifact, or in END-TERMINATE's case a bug fix making the existing path actually work, plus real receipts) or fully covered including the REST-integration test the row explicitly asked for (parallel MI — blocked by the `ir_plan.rs` gap, now fixed; inclusive dynamic fork — not blocked, now tested). No row is silently partial; `board_candidate.rs` makes no new claims about any of the four (none of G5's new surface was added to the board-candidate/NL vocabulary, so there is nothing there to misdescribe). Remaining before G6 begins: compaction (R-B).

---

### G6 — Session artifact, done (2026-08-11)

**One design fork surfaced and ruled** (via `AskUserQuestion`, not invented, raised by the blind reviewer below rather than found up front): whether `session_undo_endpoint`'s retry-idempotency lookup (`terminal_proposal_receipt`) should stay a raw event-log scan (a client retrying a ratify call sees the original terminal receipt even after a later undo excises that position) or become undo-aware (retry would signal the ratification is no longer live). Adam ruled: keep the raw-log scan — idempotency wins; undo only changes what a NEW position read observes, it never retroactively invalidates a past request's receipt.

**G6.1 — Undo as truncated replay (CAREFUL).** New `DesignSessionEventKind::Undo { target_seq: u64 }` event (`bpmn-lite-store/src/store.rs`) — additive, existing events decode unchanged. New `DesignSessionRecord::visible_events(as_of_seq: Option<u64>)`, the mechanism everything else routes through. **First implementation was wrong** (a flat range-union: every `Undo{target_seq}` event excised `(target_seq, seq]`) and shipped with 7 passing unit tests that didn't exercise the failure mode; blind review (below) found it by hand-tracing a chained-undo scenario, then proved it two ways: (a) the `Undo` marker excluding its OWN seq collapsed the session's live head back toward `target_seq` after every undo; (b) a SECOND undo re-targeting FORWARD past an EARLIER undo's target stayed permanently unreachable, because the earlier undo's static exclusion range outlived the newer undo that should have superseded it. **Fixed** with a recursive jump-chain definition instead of a static union: `visible(bound)` = if the latest `Undo{target_seq}` event within `bound` is `U` at `seq=U_seq`, then `visible(target_seq)` (recursive) ∪ `{U_seq}` (the marker itself, always) ∪ every event `> U_seq` and `<= bound` (forward progress since that undo); with no `Undo` event in range, `visible(bound)` = everything `<= bound`. Terminates because `target_seq < U_seq <= bound`, strictly decreasing. Verified by hand against every existing test case (all unchanged expectations) plus two new regression tests naming the exact scenario (`the_undo_marker_event_itself_always_stays_visible`, `a_later_undo_target_stays_reachable_after_an_earlier_undo`, both `bpmn-lite-store/src/store.rs`) and a REST-level end-to-end regression (`test_a_later_undo_target_stays_reachable_after_an_earlier_undo`, three sequential graph-edits, undo back to the first, then re-target forward past the first undo to the second — 422 before the fix, 200 with the correct graph after).

`current_source`/`graph_edit_payloads` rebuilt on `visible_events` (`current_source_as_of`/`graph_edit_payloads_as_of`), new `related_event_is_visible(related_seq, as_of_seq)` helper for the plan's named `ProposalAudit.related_event_seq` reconciliation point. `reconstruct_designer_dag` (`bpmn-lite-server-designer/src/rest.rs`) gained an `as_of_seq: Option<u64>` parameter — all 9 existing call sites updated explicitly (8 pass `None`/live, 1 — the new undo endpoint's fail-closed pre-validation — passes `Some(target_seq)`); `latest_gameboard_belief`/`design_history_projection` (the other two reverse-scan-for-latest projections the plan named) likewise gained the parameter and route through `visible_events`. `terminal_proposal_receipt` deliberately did NOT — see the ruled fork above; its doc comment states the reasoning inline. `ProposalAudit.related_event_seq` is now surfaced (previously write-only, never read back anywhere) with a computed `related_event_visible` flag in `sage_session_audit_endpoint`'s response, using the new helper.

New `POST /api/dsl/sessions/:id/undo` endpoint: validates `target_seq` is strictly before the live head (computed via the now-correct `visible_events(None)`), fail-closed re-derives and re-admits the truncated `DesignerDag` BEFORE persisting the `Undo` marker (an undo target that wouldn't itself admit is refused, not discovered broken on the next read), then best-effort records a `MoveAttemptReceipt` tagged `CorrectionKind::Undo` against the PRE-undo position via the pre-existing public `utterance_engine::bpmn_board::record_bpmn_attempt` — the first real constructor of that variant anywhere in the workspace (`evaluate_frozen_game_funnel`'s `reversals` tally already reads it; it had been permanently dead/zero). The response's `graph_content_hash` field **also shipped wrong in the first pass** (bound to the pre-undo `record`'s unbounded live view, byte-identical to what it would have been had undo never happened) — found in the same blind-review pass, fixed to compute over `graph_edit_payloads_as_of(Some(target_seq))`, with a regression test asserting it differs from the pre-undo hash and matches a fresh post-undo reload.

**G6.2 — Runbook rendering.** New `designer-graph/src/runbook.rs` module (new `pub mod`, added to the boundary baseline's `approved_pub_modules` list for `designer-graph`): `render_operation`/`render_runbook` render the `Operation` tape as one S-expression-shaped line per operation (`(append-node :anchor ... :as (service-task :id ...))`), exhaustively matching all 17 `Operation` variants (no wildcard arm — a missed variant would fail to compile). No round-trip claimed; read-only session-review text, mirroring `bpmn-lite-compiler`'s `ToSexpr` syntax without depending on it (that trait operates on the parsed DSL-source AST, a different data model). New `GET /api/dsl/sessions/:id/runbook` endpoint.

**G6.3 — Replay-equivalence test.** `test_replay_of_the_same_log_is_equivalent_across_independent_folds`: two independent `reconstruct_designer_dag` calls over the same loaded record, asserted equivalent via `DesignerDag::ir_graphs_equivalent`. Blind review noted the content-hash half of this test (`assert_eq!(graph_content_hash(&record), graph_content_hash(&record), ...)`) is tautological (same call on the same reference) — left as-is since the `ir_graphs_equivalent` assertion is the real, non-trivial claim and the tautological line is harmless, not misleading in context.

**G6.4 — Template↔tape linkage.** `bpmn_lite_authoring::registry::WorkflowTemplate` gained `session_id: Option<Uuid>` (`#[serde(default)]`, additive); `PublishOptions` gained the same, threaded through `publish_workflow_from_dto`, stamped `Some(record.id)` at `save_design_session_endpoint`'s publish call site. New Postgres migration `065_workflow_template_session_id.sql`, deliberately excluded from `enforce_template_immutability()`'s content-equality trigger and from `PostgresTemplateStore::save`'s `ON CONFLICT DO UPDATE SET` list — provenance metadata, set once at insert, not part of a published template's immutable content. `store_postgres_templates.rs`'s INSERT/SELECT/`TemplateRow` updated; verified against a real local Postgres. `test_published_template_resolves_to_its_authoring_session`: publishes a graph-authored session, loads the resulting template, follows `session_id` back to the session, confirms it's the same graph-backed tape.

**Blind review (dispatched independent reviewer, CAREFUL tier per plan) found two real defects** (both in G6.1, detailed above — the self-excising undo marker breaking chained-undo reachability, and the stale pre-undo `graph_content_hash` in the undo response) **and one medium finding** (the plan's named `related_event_seq` reconciliation point had a helper built but zero production callers — now wired into `sage_session_audit_endpoint`). All three fixed, with regression tests naming the exact failure scenario, re-verified against the full existing test suite (no prior-passing test's expectation changed) plus new tests. The reviewer independently built and ran `cargo build --workspace --all-features` and the full in-scope-crate test suite themselves (including the Postgres-backed `bpmn-lite-authoring` tests, against a real local database) rather than trusting the author's report, confirmed clean, and confirmed a battery of "checked and found sound" items: the exclusion algorithm's monotonicity (post-fix), every `reconstruct_designer_dag`/`design_history_projection`/`latest_gameboard_belief` call site's argument, the `session_lock` mutex actually covering the whole undo operation with no race window, the runbook renderer's exhaustiveness, the G6.4 SQL's immutability/no-mutation-on-resave guarantee, and correct `pub`/`pub(crate)` scoping on every new item.

**Verification (post-fix, re-run in full):** `cargo check --workspace --all-features` clean. `cargo test --workspace --lib --all-features` clean across every crate (0 failures across ~30 crate test binaries), including every new/fixed G6 test by name. `python3 scripts/check-semantic-gameboard-boundaries.py` passes: zero drift on all 8 tracked (crate × feature-set) public-api surfaces — every sha256 matches `scripts/baselines/semantic-gameboard-public-api-v1.json` byte-for-byte; the only baseline change is the deliberate, receipted addition of `runbook` to `designer-graph`'s `approved_pub_modules` list. G6's other new `pub` surface (`WorkflowTemplate::session_id`, `PublishOptions::session_id`, `DesignSessionRecord::visible_events`/`related_event_is_visible`/`graph_edit_payloads_as_of`/`current_source_as_of`) lives in `bpmn-lite-store`/`bpmn-lite-authoring`, outside the tracked baseline's two crates — same disposition as every prior gate's own new pub items.

**Not yet done, stated honestly:** no DSL S-expression or raw-XML surface for undo/runbook (both are REST/session-level concepts with no DSL-source analogue, consistent with the plan's own framing of G6 as session-artifact, not authoring-vocabulary, work). No UI/palette affordance for undo or runbook viewing — REST-only, same "reachable but only by a caller who knows the JSON shape" status quo every prior gate's new endpoints have shipped with. `terminal_proposal_receipt`'s raw-log-scan behavior (ruled above) means a retried ratify call can surface a receipt describing a since-undone mutation with no in-band signal that it's historical — accepted as correct per Adam's ruling, not a gap.

**Gate G6 met:** undo returns a session to a prior position with proposals and projections correctly reconciled (post-fix — the two blind-review-caught defects would have broken exactly this clause); a session renders as reviewable DSL; replay-equivalence is asserted; a published template resolves to its authoring tape. Remaining: none — this was the plan's final tranche (G1-G6 all closed).

### Post-close finding — MI `collection` is unreachable via the utterance surface (2026-08-12, scoped out, not a G6 defect)

A super-user-persona REPL test (`bpmn-lite-server-designer/src/rest.rs::tests::test_super_user_repl_builds_6_step_2_branch_2_loop_workflow`) drove a 6-task/1-fork(2-branch)/2-loop workflow through `/api/dsl/sessions/:id/utterance` → ratify only (no direct `/graph-edit` for the shape itself), to prove the power-user REPL baseline (frame doc, "Session DSL is pack-level... a power user dictating session DSL invokes a template or advances a matched motif"). It passed: `op.insert_after` and `op.create_parallel_region` (branch_count parsed from text) both bind cleanly from utterance text, ratify correctly, and the resulting graph saves/publishes.

`op.create_multi_instance_region` is legally offered by the gameboard and its `anchor`/`declared_max` slots resolve from utterance text — but its `collection` slot is a `DataReference` that must name an existing `DataObject` node, and **no `Operation` variant can ever create a `DataObject`** post-seed (only `DesignerDag::seed` can, at construction time). So a bounded MI loop over a *named* collection can never be fully dictated by utterance in any session opened through `/api/dsl/sessions` today — the test's two MI regions were built via one flagged `/graph-edit` fallback call (mirroring `build_mi_session`'s undeclared/empty-collection pattern), not via utterance. This is the same class of gap already noted in-repo near `test_direct_edit_recovers_interrupting_timeout_equivalence` (`corr_key_source` for `prod.request_and_wait`) — a `DataReference`-typed slot with no minting operation.

**Ruled by Adam:** add a `CreateDataObject` graph-edit operation (+ utterance phrase/binding) so a session can mint a new `DataObject` node post-seed, closing the MI-collection reachability gap. **Specced as tranche G7 (§2 tranche map + its own section above the parked track, 2026-08-12)** — three pre-build forks (F-G7a type surface, F-G7b verifier tightening, F-G7c delete integrity) all ruled by Adam same day, recommendations accepted; no implementation started.

### G7 — Data-object authoring (`CreateDataObject`), done (2026-08-13)

**A fourth Rule-7 substrate/plan mismatch surfaced and ruled during implementation, on top of the three pre-build forks (F-G7a/b/c) already closed 2026-08-12.** G7.1's design point directed mirroring how `SetDefaultGuardBudget`/`SetDefaultRetryPolicy` "already surface on the board" as anchorless ops. False: tracing `positional.rs`/`board_candidate.rs` found neither is in `OperationKind::ALL`, neither is returned by `ops_at`/`legal_operations` — both are process-level `DesignerDag` fields settable only via raw `/graph-edit`, never board/utterance-reachable. **Ruled by Adam (2026-08-13): offer `CreateDataObject` at Start only** — added to `ops_at`'s `is_start(ir)` branch; every session has exactly one Start, always present post-seed, so the op stays reachable with zero `LegalityOracle` interface change. A second scope question was raised and answered the same day: whether G7.4's referential-integrity tightening should also cover `corr_key_source` (the sibling gap named in the post-close finding above). **Ruled by Adam: out of scope, its own future vision/scope paper** — `corr_key_source` is free text in ~15 existing fixtures across 4 crates today (including one deliberately-undeclared case); tightening it is a real, larger, separate change, not a G7.4 drive-by. Tracked as a parked follow-up (§3).

**G7.1 — `Operation::CreateDataObject`** (`designer-graph/src/ops.rs`). New variant `{ key, id, name, type_decl, role }`, no anchor/edge (a structural declaration, not flow). `apply` inserts via `insert_node` — the same duplicate-BPMN-id-refusal path `seed` uses, so a clashing id is a typed reject at stage time, not deferred to the verifier. `render_operation` (`runbook.rs`) gained the arm (compiler-forced, all 18 `Operation` variants now covered — G6.2's own doc comment count moves 17→18). Positional legality per the Start-only ruling above.

**G7.2 — Candidate identity + semantic pack entry.** `OperationKind::CreateDataObject` (`board_candidate.rs`, canonical id `op.create_data_object`) — `OperationKind::ALL` 16→17, `CANDIDATE_SCHEMA_VERSION` 4→5 (deliberate, receipted board-hash change, same precedent as G1.3's v3→v4 bump). New pack entry in `bpmn-semantic-pack.yaml`: `name` (identifier), `data_type` (text, per F-G7a — a token match over the four primitive words, not free quoted text). `data_type`/`name` added to the pack's `declarations.slot_kinds` allow-list (a real admission-validation gate found live, not anticipated — `semantic-pack`'s `validate` refuses an argument `name` absent from that list). `.lock`'s `source.sha256`/`pack.artifact_sha256`/`adapter_bindings` regenerated against the real recomputed values (twice — once for the entry itself, once more after the blind-review fix below).

**G7.3 — Slot binding** (`bpmn-lite-server-designer/src/proposal.rs`). Confirmed the plan's "start_workbook's per-op arm" framing was imprecise but not wrong in substance: `start_workbook` dispatches generically on `ArgumentKind`, not per-candidate-id; `name` binds via the existing `ArgumentKind::Identifier` quoted-name convention, `data_type` needed a new special case (`ArgumentKind::Text if argument.name == "data_type"`, mirroring the file's existing `Duration`/`interval` name-special-case precedent) doing an unquoted token match (`primitive_type_word`) over `bool|boolean`, `integer|int`, `decimal|float|double`, `string|text` — independent of the shared quoted-name index, so it can't collide with the `name` slot's own quote consumption. `legal_moves.rs::materialize_workbook` gained the matching `op.create_data_object` arm (added to `MATERIALIZED_CANDIDATE_IDS`) parsing the resolved text back into `DataObjectType::Primitive(...)`, role always `Internal` per F-G7a (role=Input is a deliberately separate future act).

**G7.4 — Referential integrity, scoped to the ruled F-G7b text (`collection_flag_name` only — see the scope-correction ruling above).** `verify_data_objects` (`bpmn-lite-compiler/src/verifier.rs`) gained a check that every `IRNode::MultiInstance.collection_flag_name` names a declared `DataObject`, localized to the referencing node's id — closing exactly the two-surfaces-two-semantics gap the post-close finding named (utterance path already required this via `mentioned_id`; raw graph-edit previously accepted any string). This retro-tightened **six** pre-existing fixtures across four crates that relied on the old, looser behavior — a real, receipted migration, not a silent weakening: `bpmn-lite-compiler/src/lowering.rs::make_multi_instance_graph`, `bpmn-lite-authoring/src/publish.rs::t_pub_13_g4_gate_scenario_manifest_survives_reload`, `bpmn-lite-engine/src/tests.rs`'s `multi_instance_v2_xml` fixture (five MI runtime tests), `bpmn-lite-server-designer/src/rest.rs`'s `build_mi_session` helper, and `designer-graph/src/ops.rs`'s `multi_instance_region_admits_and_non_mi_node_refused` — each now declares its collection as a real `DataObject` before referencing it. `Operation::DeleteNode` (`ops.rs`) gained F-G7c's referenced-DataObject refusal, mirroring the existing guard-dangling pattern exactly (naming both the data object and every referencing MI region's id, not just the first). New cement test `delete_refuses_dangling_mi_collection_reference` (`ops.rs`), RED (delete refused, both ids named) + GREEN (delete the region first, then the data object deletes clean) — same shape as the pre-existing `delete_refuses_dangling_guard`.

**A real pre-existing manifest defect surfaced while migrating the `t_pub_13` fixture, found and fixed (not scoped out, not a design fork — a mechanical bug the ruled behavior made reachable for the first time).** `derive_parameter_manifest`'s (`bpmn-lite-authoring/src/manifest.rs`) `upsert` never widens an existing slot's `kind` — a name independently declared `DataObjectRole::Input` was unconditionally registered `Scalar` *before* the MI walk ran, so a collection that is *also* correctly declared `Input` (the only sensible role for a client-supplied collection, now mandatory since G7.4 requires collections to be declared at all) silently lost its `Collection` classification to the earlier `Scalar` registration under the same name — invisible before G7.4 because no declared-Input DataObject had ever also been an MI collection name. Fixed: a `collection_flag_names` guard collected from every `MultiInstance` node up front, excluding those names from the blanket Input→Scalar loop so the more specific Collection classification always wins. Proven by the existing `t_pub_13` test (now exercising exactly this path with `directors` declared `Input`) plus the pre-existing `t_manifest_8_two_mi_regions_sharing_a_collection_merge_element_shape` (confirms the fix doesn't disturb the already-correct multi-region-sharing case).

**G7.5 — End-to-end receipt.** `test_super_user_repl_builds_6_step_2_branch_2_loop_workflow` (`bpmn-lite-server-designer/src/rest.rs`) rewritten to drop its one flagged `/graph-edit` fallback entirely: the full 6-step/2-branch/2-loop build (task1/task2/2-way parallel fork with an MI loop over a dictated collection in each branch/task3/task4) is now 100% utterance+ratify except the one truly unavoidable step (the very first node — no anchor exists to utter from before it). Two `op.create_data_object` utterances at `start` (`"Declare a new named data object with a primitive type... called 'items_1'/'items_2', of type string"`) mint the collections; two `op.create_multi_instance_region` utterances at each parallel branch (`"...over 'items_1'/'items_2' with a declared maximum of 2"`) consume them by name — proven bound end-to-end via the real HTTP disposition/workbook JSON (`data_type`/`name` slots resolved, `collection`/`declared_max` slots resolved), not asserted from source reading. RED half cemented: the isolated probe utterance now explicitly *names* an undeclared collection (`'ghost_collection'`, changed from the pre-G7 version which named nothing at all) and is refused, stuck at `needs_arguments` with `collection` the missing slot — proves `mentioned_id`'s fail-closed binding holds against a real declared-DataObject world, not just an empty one. Final graph asserts `compiles: true` and both `for_each_items_1`/`for_each_items_2` MI nodes present.

**Blind review (dispatched independent reviewer, CAREFUL tier per plan).** Verified correct: `CreateDataObject`'s apply arm and `insert_node`'s duplicate-id refusal; the verifier check's node-existence semantics (correctly type-agnostic — `Primitive` vs `SemOsDomain` — since no dedicated "Collection" `DataObjectType` variant exists, a pre-existing modeling property, not a G7 gap); the delete-refusal's `filter_map`/`collect` correctly finds *every* referencing MI node, not just the first; `primitive_type_word`'s whole-word matching (`words()` split+trim) cannot false-positive inside a longer word (e.g. "string_processor"); the `manifest.rs` fix's correctness including the multi-region-sharing edge case; full `cargo check --workspace` clean and `cargo test --workspace --lib --all-features` clean across every crate including the rewritten REPL test exercising real RED+GREEN, not just happy path. **One real finding, fixed:** the new pack entry's `positive_examples` string (`"Create a string collection called directors"`) omitted quotes around the identifier, inconsistent with the fact that `name` only ever binds from quoted text via `quoted_names` — the corpus example would never actually resolve through the real binder. Fixed (quoted the name); `checked_in_pack_receipt_detects_source_and_binding_drift`/`registry_snapshot_is_content_addressed_and_stable`'s golden hashes recomputed against the corrected content, `.lock` regenerated again, full suite re-verified green.

**Verification:** `cargo check --workspace --all-targets` clean (only the two pre-existing, documented, unsuppressed warnings in `rest.rs`). `cargo test --workspace --lib --bins` clean across all ~45 crate test binaries, 0 failures. `validate_registry_coverage` red→green exactly as designed (`OperationKind::CreateDataObject` landing alone failed it with `missing: ["op.create_data_object"]`; the matching pack entry closed it). `python3 scripts/check-semantic-gameboard-boundaries.py`: `status: pass`, zero drift on all 8 tracked (crate × feature-set) surfaces — G7's new `pub` items (`Operation::CreateDataObject`, `OperationKind::CreateDataObject`) live in `designer-graph`, module-list-tracked not hash-tracked, consistent with every prior gate's disposition for that crate.

**Not yet done, stated honestly:** `role: Input` for a session-minted DataObject is reachable only via raw `/graph-edit` (a `SetDataObjectRole` op, noted as a possible follow-up in F-G7a's ruling, was not built — out of scope, not silently dropped). `corr_key_source` referential integrity is the parked follow-up named above and in §3. `SemOsDomain`-typed DataObjects remain `/graph-edit`-only by design (F-G7a), never utterance-dictatable.

**Gate G7 met:** the REPL test builds the whole shape by utterance alone (proven, not asserted); `validate_registry_coverage` green with the new 1:1 entry; board-hash change (`CANDIDATE_SCHEMA_VERSION` 4→5) receipted; verifier tightening (F-G7b, scoped to `collection_flag_name`) red→green with real fixtures on both surfaces; delete referential integrity (F-G7c) red→green cemented; `check-semantic-gameboard-boundaries.py` clean; blind review complete with one real finding fixed and re-verified. This was the plan's first post-close tranche — G1 through G7 now all closed.

---

## E. Plan-level rulings (2026-07-27 — delegated by Adam "ok do it"; implementation-scope, no V&S clause touched)

| # | Fork | Ruling |
|---|---|---|
| E1 | Sage identity in the standalone build | **Sage is a trait** (evidence producer + escalation/clarification renderer), two impls: deterministic stub (renders policy-produced clarifications only, no free dialogue — the default, so standalone runs keyless) and a live LLM adapter (Anthropic API, config-keyed). Honest under D7: the routine path never needed Sage, so a stub default hides nothing. |
| E2 | Board-universe source | WS-C consumes the **registry interface** (the `ManifestPlaceholderRegistry` surface) behind a provider trait; T3's sealed pack becomes a drop-in provider behind the same trait when it lands. G2 does not require the sealed pack; T3 stays independently sequenced. |
| E3 | Tier-0 integration shape | **In-process embed-and-score**: board candidates embedded on the fly via the matcher's Candle embedder (CPU, L2-normalised), cosine in memory — boards are tens of candidates; the pgvector ranking path is NOT used for Designer boards (no DB round-trip, no /dev/rust schema dependency, deterministic and hashable). Palette pre-embedding is a later optimisation, not the mechanism. |
| E4 | UI stack | **Static HTML + vanilla JS (ES modules) + SVG**, served from bpmn-lite-server; renders the server-supplied DAG layout. No framework, no build toolchain. It is a window, not an editor — all mutation via endpoints; trivially Chrome-MCP-drivable. |
| E5 | Initial shadow thresholds | Thresholds live in a **versioned config struct hashed into `disposition_policy_hash`** — never inline literals. Initial values are named PLACEHOLDERs (separation margin, abstention floor, NONE_OF_THE_ABOVE-wins → abstain), low-stakes in shadow, recalibrated at G3 where the threshold values are Adam's. The ruling is the mechanism, not the numbers. |

**GOV.2 CLOSED (Adam confirmed 2026-07-27):** designer crates (`designer-graph`, `designer-ui`, `utterance-engine`) live in the **bpmn-lite workspace** as separate crates; ob-poc consumes later via git dependency with **exact rev pin** (never path-`[patch]`); extraction to an own repo deferred to the promotion gate when a second consumer exists. Rider: `/dev/rust` gets a private remote so `ob-semantic-matcher` is rev-pinnable.

**Executor split (Adam, 2026-07-27: "I will keep fable"):** Fable runs all CAREFUL items, entry traces, dispatch-brief authoring, and blind-review orchestration; Sonnet executes GRIND tasks only against a frozen upstream interface and a dispatch brief (full skeletons, verbatim invariants, HALT conditions, receipt pair named). No GRIND dispatch before its interface freezes.

## F. Receipts

### WS-A.0 entry traces + WS-C C5 trace — CLOSED 2026-07-27 (findings-only, no HALT)

**C2-residual: GREEN.** `compute_post_dominators`, `compute_region_map`,
`gateway_pairs` are `pub` + crate-root re-exported with R8 doc tags
(compiler lib.rs:24; lowering.rs:1028-1032, :1145-1149, :1330-1337); all
input/output types public and externally constructible (`IRGraph` =
petgraph `DiGraph<IRNode, IREdge>`, all-pub fields). Address-level
`compute_gateway_pairing` + `InclusiveBranchInfo` stay private BY DESIGN
— if WS-A.1 ever needs the `Addr`-level maps, that is a surfaced fork,
not a workaround. Doc-assertion converted to build-lock:
`bpmn-lite-authoring/src/oracle_boundary_tests.rs`
(`pairing_oracle_is_consumable_across_the_crate_boundary`, green) —
sibling-crate consumption of all three entry points, cement-locked.
Interface facts for the WS-A.2 brief: (i) acyclicity pre-gating is the
CALLER's responsibility; (ii) `compute_region_map`'s public contract is
**diverging-gateway → region-closing partner**, NOT node → region
membership; (iii) the public pairing name is `gateway_pairs`
(`compute_gateway_pairing` is private).

**C3: named-env-exists-but-no-runtime — claim UNSUPPORTED at HEAD; Q15
disposition recorded.** Named typed binding envs exist compile-time only
(dsl `BindingContext` HashMap<String, BindingInfo>, typed,
binding_context.rs:94-96 — zero runtime readers/writers; bpmn-lite
`PlaceholderSchema.slots` name-keyed untyped, plan.rs:238-253). NO macro
expansion path resolves named bindings to earlier-step identifiers: dsl
`MacroDefBody.expands_to` is an ordered opaque `Vec<serde_json::Value>`
with zero executors (macro_def.rs:21); bpmn-lite macro apply is caller-
param `%name%` textual substitution (macros.rs:124-160) + AstMutator
insert — no read of any binding env. The only live earlier-step→later-step
value path is POSITIONAL (`V2MiLoadElement` by array index). Order-
dependence present → **Q15 resolves toward a versioned durable named
representation** (per the WS-A.0 HALT-condition disposition). V&S §0 C3
row should flip OPEN → REFUTED-as-runtime on next V&S amendment.

**C5 (+E3 feasibility): AMBER — runtime path GREEN, build coupling
blocks.** Embedder (`/dev/rust/crates/ob-semantic-matcher`, HEAD eb0b3b6,
clean, NO remote) is DB-free at source level: Candle-only imports,
`Device::Cpu`, deterministic (no RNG/dropout; BGE-small-en-v1.5 weights
pinned to an immutable HF commit SHA; L2-normalised 384-dim; self-test
asserts same-text cosine ≈ 1.0). In-memory cosine trivial. BLOCK:
lib.rs:42-43 unconditionally compiles matcher/feedback; `sqlx`+`pgvector`
are non-optional deps → consuming the embedder drags the Postgres tree
into the designer build. REMEDY (chosen): default-on `pg` Cargo feature
gating matcher/feedback/populate_embeddings with sqlx/pgvector optional;
designer consumes `default-features = false`. Folded into the WS-C tier-0
wiring task. Note for the WS-C brief: exact-match 1.0 / phonetic 0.95
pins live in the pgvector matcher, so Designer tier-0 implements its own
exact-match pinning. GOV.2 rider CLOSED 2026-07-27:
`/dev/rust` pushed to private remote `adamtc007/ob-poc-rust` —
`ob-semantic-matcher` is now rev-pinnable.

**C4-residual — the design note (envelope ↔ instance-data mapping).**
No contradiction with ISA-002 §28 found; no HALT. The mapping:

1. **The typed invocation envelope rides the JSON planes, never `Value`.**
   `Value` has NO map/object variant (`Bool/I64/Str(interned)/Ref/Array`,
   types.rs:132-150) — a nested tagged union is not representable in
   `flags`. The envelope serialises as canonical JSON into
   `StartCommand.initial_payload` → stored verbatim as `domain_payload`
   AND (iff a JSON object; malformed = hard admission reject per R4)
   seeded into `placeholder_values` (engine.rs:789-815).
2. **Routing discriminants are top-level STRING keys.** Variant tags
   (e.g. `"delivery_kind": "client_portal"`) surface as top-level
   payload keys matched by `V2LoadPlaceholderMatch` →
   `placeholder_matches` (types.rs:1096-1111), which compares String and
   Bool ONLY (I64/arrays/objects never match — substrate rule, not a
   bug to fix silently). Designer staging validates: every declared
   routing discriminant is a string-or-bool top-level key.
3. **Variant payloads stay nested inside the JSON plane** and are read
   mid-flight via `bind_placeholder_from_payload` (absence = error,
   never null) — pointer-not-cargo intact; the envelope carries refs.
4. **Collections for MI ride `flags` as bounded `Value::Array`**
   (≤ MAX_VALUE_ARRAY_LEN=4096, depth ≤ MAX_VALUE_ARRAY_DEPTH=8,
   enforced at canonical decode + gRPC boundary + runtime backstop —
   canonical.rs:557-581, grpc.rs:170-195, kernel lib.rs:919-924).
   Per-element data is scalar/`Ref`/`Str`/nested-array by value
   (`V2MiLoadElement` clones `items[index]`); object-shaped per-element
   data must be flattened or carried as a `Ref` into the payload plane.
5. **Late-bound results enter via completion `orch_flags`**
   (`flag_<u32>` keys through the flag symbol table) — flags start
   EMPTY at spawn; there is no working start-time flag seeding (see
   finding F-DSGN-1 below).

**Finding F-DSGN-1 (surfaced, not fixed — awaiting Adam):**
`start_process` gRPC accepts and VALIDATES `req.orch_flags`
(grpc.rs:529) then silently DROPS them — `StartParams` is built without
them (grpc.rs:546-557); `StartCommand` has no flags field
(transition.rs:102-116); spawn sets `flags: BTreeMap::new()`
(engine.rs:802). The types.rs:167-174 comment describes spawn-time
seeding that does not exist. Validated-then-discarded input is a
trap-door-shaped defect under E6/fail-closed discipline. Options:
(a) wire orch_flags through StartParams→StartCommand→spawn seeding
(kernel/engine CAREFUL change; matches the comment's stated intent), or
(b) reject non-empty orch_flags at start until (a) is designed.
Recommendation: (b) now (small, fail-closed), (a) as a scheduled item —
the C4 mapping above needs neither.
**RULED (b) by Adam + IMPLEMENTED 2026-07-27:** `start_process` rejects
any non-empty `orch_flags` with `InvalidArgument` naming F-DSGN-1
(grpc.rs); stale types.rs spawn-seeding comment corrected. Receipts:
red = `start_process_rejects_any_nonempty_orch_flags` (a benign flag —
previously validated-then-discarded — now rejected); the two array-limit
start tests amended to the categorical-reject contract (strictly
stronger; completion-path limit cement unchanged); green = all existing
empty-flag lifecycle tests. Option (a) wire-through remains unscheduled
until a consumer needs spawn-time flags.

### WS-A.1 — CLOSED 2026-07-27 (CAREFUL; blind-reviewed, findings dispositioned)

Deliverables: `designer-graph` crate — frozen board-candidate interface
(19 §12.1 ops + 9 §12.2 productions, canonical ids + descriptions as
board-hash inputs, `CANDIDATE_SCHEMA_VERSION=1`, `LegalityOracle`) and
the canonical DAG schema (Q2) with the Q27 ruling: **node payload IS the
compiler's `IRNode`** — per-node declarations reach the sealed envelope
by construction; process-level declarations ride the DAG root and are
carried by `admit()` explicitly. Blind review verdict: decision SURVIVES
with three riders (per-node-scope claim narrowing; never the persistence
wire format — the edit log is, per §6.2/§12.5; NodeKey-level referential
integrity). Disposition:

| # | Severity | Finding | Disposition |
|---|---|---|---|
| F1 | BLOCKER | `admit()` dropped `default_guard_budget` (lowered with `None`); test camouflaged it | **FIXED**: `admit()` = `Compiler::lower_with_default(&ir, self.default_guard_budget)`; red→green `process_default_guard_budget_reaches_the_sealed_envelope` (Some(3) → envelope max_failures 3; None → conservative default); module-doc claim narrowed to per-node scope |
| F2 | CONCERN | `attached_to` string id lets renames dangle or silently re-point guards | **FIXED**: `DesignerNode.attached_to_key: Option<NodeKey>`; `to_ir()` projects the host's CURRENT id; non-boundary attachment refused at insert; stale-string test green |
| F3 | CONCERN | Id uniqueness promised, enforced nowhere; compiler admits duplicate ids (ambiguous budget/attachment binding) | **FIXED both halves**: insert-time duplicate node/flow-id rejection (designer) AND a new duplicate-id theorem in the production `verify()` (P8 — the oracle is the gate), cemented `duplicate_ids_are_refused_by_verify`; full workspace sweep green |
| F4 | CONCERN | `pub` mutators contradict bypass claim; `Uuid::new_v4` in `insert_node` breaks edit-log replay determinism | **FIXED**: mutators `pub(crate)` (WS-A.2 ops are the public surface, I18 structural); `NodeKey` caller-supplied — key generation belongs to the operation record (Q5), pinned in the WS-A.2 brief |
| F5 | CONCERN | `admit()` omitted `verify_bytecode` + types-crate V-1..V-11 — G1 which-theorem parity unsatisfiable | **FIXED**: `admit()` runs the exact direct-compilation chain via `Compiler::lower_with_default` (verify_or_err → lower → verify_bytecode → envelope → `from_verified_envelope`), returns the `VerifiedWorkflow` for G1 comparison |
| F6 | CONCERN/NOTEs | Description edits uncemented; `CandidateId` serde leaks variant names; no dedup | **FIXED**: blake3 golden content cement over (id, description, version) triples; hash-preimage contract documented as `(canonical_id, description, schema_version)` ONLY; `legal_candidates` dedups (test) |
| F7 | CONCERN | Raw-IRNode serde as durable format = C7's lesson repeated (landed IR field rename precedent) | **ACCEPTED as a rule**: module doc reworded to §6.2/§12.5 — the EDIT LOG is the persistence surface, the DAG a replay product; any snapshot goes through a versioned envelope. Written into the WS-A.2 brief |
| F8 | NOTE | I23 mechanism/backstop inverted (no per-op forward-only pre-gate yet) | **PINNED to WS-A.2 brief**: every edge-introducing operation pre-gates `has_path_connecting(to, from)`; verifier stays the backstop. Plus reviewer's `declared_max = 0` convention test |

**Substrate finding F-DSGN-2 (surfaced, unfixed — awaiting Adam):**
`verify_data_objects` (compiler verifier.rs) had ZERO non-test callers —
a gate that never ran. **RULED wire-in (Adam) + IMPLEMENTED 2026-07-27**:
`verify()` now runs it on every admission; cement
`verify_runs_data_object_checks` (unresolved FFI var-ref refused — red
that previously verified clean); full workspace sweep green.

### WS-C C-now items 1–3 — CLOSED 2026-07-27 (CAREFUL; blind-reviewed NOT-CLEAN → remediated)

Deliverables: `utterance-engine` crate — stable contract (`FiniteScore`
typed rejects, `SlmResult`, canonical tie-break, `NONE_OF_THE_ABOVE`),
§11.7 board construction, deterministic disposition policy + I28 record.
Blind review returned 2 BLOCKERS + 7 CONCERNS + 5 NOTES; disposition:

| # | Finding | Disposition |
|---|---|---|
| B1 | Board-hash preimage non-injective (anchor `"<root>"` sentinel collision; delimiter forgery via provider strings) | **FIXED**: length-prefixed domain-tagged preimage (`tag:len:bytes`), distinct tags for None/Some; red fixtures — sentinel collision + crafted delimiter pair now hash differently |
| B2 | Close-scope deviation: close-gap → `Ambiguous` CONTRADICTED §10.3's ruling (score topology cannot distinguish ambiguity from compound); `MissingArguments`/`Compound` absent vs I21 | **FIXED to the ruled reading**: insufficient separation → `EscalateToSage` (never a masking A-or-B render); enum carries the full I21 shape with `Ambiguous`/`MissingArguments`/`Compound` declared UNREACHABLE-in-v1 (reachable only with certified producers — policy version bump + plan amendment, not a threshold tweak). D20 escalation SHAPE (board ref, context-change channel) lands with WS-B.3's flow — recorded here as WS-B scope |
| C3 | Abstain description uncemented board-hash input | **FIXED**: folded into the designer-graph golden (hex bumped deliberately) |
| C4 | policy_hash rested on serde_json float text | **FIXED**: hand-built preimage (`f64::to_bits`), golden hex cement for shadow_v1 |
| C5 | decide trusted producer order; duplicates admitted | **FIXED**: policy re-sorts via `rank_canonically` (I28 tie-break policy-owned); duplicate ids refused; misorder receipt green |
| C6 | build_board fail-open on provider misbehavior | **FIXED**: `-> Result`; reserved `abstain.*` namespace refused; same-id/different-content collision refused; identical dupes still collapse |
| C7 | Reachability context lacked artifact identity | **PARTIALLY FIXED + WS-B obligation**: `BoardContext.graph_identity` added and hashed (None hashed distinctly); WS-B MUST supply the session revision/graph hash when building boards — brief item |
| C8 | I27 documentation-not-mechanism (pub fields allow forging records/boards) | **DEFERRED with note**: Repl recheck is the ratified gate (§11.7 "the pre-filter is hygiene, never the gate"); Board private-fields hardening rides WS-C item 4 wiring |
| C9 | anchor/anchor_id decoupled | **FIXED**: single `Option<(&NodeKey, &str)>` parameter |
| N1 | Empty ranking laundered as escalation | **FIXED strict**: producer malfunction, typed error |
| N2 | Ambiguous top-2 truncation | Moot in v1 (unreachable); revisit at producer certification |
| N3 | Projection hash unversioned | **FIXED**: `ctxproj.v1:` domain tag |
| N4 | policy_version honor-system | **FIXED**: golden decision table + golden policy hash tied to version 1 |
| N5 | Record ranking as raw f64 | **FIXED**: `FiniteScore` in `DecisionRecord` |

Config-by-hash registry (N3 rider) is a named WS-C item-6 (capture
pipeline) obligation: records are reproducible only if configs are
retrievable by `disposition_policy_hash`.

### WS-C C-now items 4–6 — CLOSED 2026-07-27

- **Item 4 (tier-0):** `ob-semantic-matcher` `pg` feature-gate landed
  and pushed (`ob-poc-rust @ ff3f12c7` — C5 AMBER→GREEN; Candle slice
  builds with no Postgres tree). `Tier0Retriever` trait is the producer
  seam; `LexicalTier0` (the demoted keyword gate's ruled successor:
  deterministic token overlap, designer-side exact-match 1.0 pin, NOTA
  as overlap complement) and `EmbedTier0` (E3: rev-pinned Candle
  embedder, on-the-fly board embedding, in-memory cosine; behind an
  off-by-default `embed` feature so default builds stay network-free;
  integration receipt `#[ignore]`d for cold-cache weight download).
  **Pipeline-in-loop receipt green**: board → tier-0 → policy → I28
  record end to end, gibberish abstains, deterministic (G2 criterion,
  first light).
- **Item 6 (capture, switch OFF):** `CapturePipeline::off()` sole
  zero-arg constructor; ON requires a ratified Q9 charter reference
  (D17 as mechanism); suppression visible, never silent; physical
  Evaluation/Training/Audit sink separation. `ConfigRegistry` closes
  the N3 rider (policy-hash → config, hash derived never supplied).
- **Item 5 (metrics):** the §10.7 per-tier decomposition
  (completeness / recall@K / ranking-given-inclusion / end-to-end /
  abstention coverage) with zero-denominator honesty, plus
  `assert_position_invariant` reusable against any producer.
- utterance-engine suite 23/23 (+1 ignored embed integration);
  workspace clean. **Next: WS-B day-one wiring** (session utterance
  endpoint → `decide()` with `LexicalTier0`) = formal shadow start per
  §C constraint 2; WS-B must supply `graph_identity` (C7 obligation).

### WS-A.2 slices 1–5 + WS-B UI — receipts (2026-07-27)

Five Sonnet GRIND dispatches, each against a committed proscriptive
brief, each reviewed first-hand before commit (the executor-split loop
proven): **16 operations** (linear 5, guard/declaration 5, region 4,
ReplaceNode, CreateBranch) and **6 of 9 §12.2 productions** as pure
`bindings → Vec<Operation>` compositions with atomic-abort application
and serde round-trip (Q5 edit-log entries). Binding rule minted by the
slice-4 remediation (my brief mis-specified `reminder_then_escalate`;
the executor flagged the admit-gap honestly): **a production ALONE must
admit — it owns its complete shape including guard escape flows.**
Illegal states unrepresentable by typing where possible
(cycle-on-interrupting unconstructible through
`InterruptingTimeoutBindings`; MI max mandatory; budget-on-non-guard
absent from the vocabulary). designer-graph 49/49.

Excluded pending CAREFUL substrate traces (never faked):
`CreateRace`/`timer_message_race` (no race IRNode),
`CallSubprocess`/`call_durable_subprocess` (no call-activity IRNode),
`AttachRollbackGuard` (no GUARD-R IR path),
`human_review_with_rework` (XOR default-edge semantics untraced).
`CloseParallelRegion` recorded unrepresentable-by-design (regions
constructed closed).

**WS-B.1/B.2 landed:** `/designer` static window (ruling E4) — session
list, REPL pane showing disposition + board-hash + D17 capture state
per turn, source/diagnostics pane, save-as-template, SVG graph window
over the server-built DAG + layout endpoint. UI smoke receipt green.
Shadow pipeline live at the session utterance endpoint since `d4e2406`.

Open to G2: solicit-document end-to-end authoring receipt; red-team
script; the four traces above; WS-B.4 edit-log persistence
formalization; blind review of the WS-B surface before the gate.

#### Receipts — four CAREFUL substrate traces (2026-07-27, findings-only, four independent read-only agents)

**Race / `timer_message_race` — NOT-REPRESENTABLE (frontend); kernel COMPLETE.** The ISA/kernel race primitive is fully built and loser-cancelling: `V2RaceOpen/V2ArmTimer/V2ArmMsg/V2RaceClose` (types.rs:575–611), `WaitState::V2Race`, winner resolution emitting `TimerMutation::V2CancelRace` (kernel lib.rs:3187, 4307–4320), V-5 contiguous-arm verifier rules. Three independent frontend breaks: no `IRNode` variant; parser hard-rejects `eventBasedGateway` (parser.rs:557–564); lowering never emits the race opcodes. Guard composition cannot substitute: boundary guards wrap task hosts only. Work to open it: parser + IR variant + lowering + frontend-verifier acceptance — zero kernel work. `CreateRace`/`TimerMessageRace` exclusions stand.

**CallSubprocess / `call_durable_subprocess` — NOT-REPRESENTABLE (IR/durable); authoring checks EXIST; spawn half ABSENT.** DSL hash-plug tasks already carry call-activity verification (child existence, blocking-deadlock, recursion — closure.rs:98–140) but lowering collapses every task to `Instr::ExecDslTask` (frontend.rs:86–100, `delivery_mode` dropped) — an external job, not a child workflow. The durable-invocation substrate is built but producer-less: `ProcessState::WaitingOnSubmission/WaitingOnInvocation`, `Command::StartChildResult`, `ChildStart` (zero call sites), `TickOperation::StartChild`, migration 033 (caller-side callout registry — no parent/child instance columns). Work: IR node, lowering arm, a kernel word that EMITS WaitingOnSubmission + child spawn, store producer, schema linkage. Exclusion stands.

**AttachRollbackGuard / GUARD-R — kernel EXISTS, frontend-inaccessible; compensation ABSENT by design.** `V2GuardR/V2GuardREnd/V2CancelScope` + A3 five-field snapshot restore + V-10 rules are complete and test-covered (kernel lib.rs:2122–2151, 3900–3991, 4045; v2_verifier.rs:531, 852). No IRNode, no parser keyword, no lowering emission. True saga-compensation (reverse-order handlers over COMPLETED work) is deliberately uninhabited (`RecordKind::Compensation`, concurrency.rs:65–80) — v3 scope. Exclusion stands; opening data-rollback = frontend-only work.

**XOR default-edge — default is MANDATORY; `human_review_with_rework` UNBLOCKED.** Verifier §6 requires EXACTLY ONE condition-less edge on every multi-out XOR (verifier.rs:304–330); lowering emits conditioned `BrIf` chains in edge order with the default as trailing `Jump` — zero-match deterministically takes the default, no incident on the XML path. Conditions are boolean Eq/Neq only at the XML frontend. Backward rework edges are REFUSED twice (IR cyclicity, verifier.rs:115–123; bytecode backward-jump, verifier.rs:847–871) — rework is forward-only or bounded (MI / cycle guard). Production shape ruled representable: HumanWait → XOR with conditioned approve arm + default reject/rework arm routing forward. Remains in the production queue.

#### Receipts — G2 (partial) + F-DSGN-3 fail-open closure (2026-07-27, `designer-graph/src/g2_receipts.rs`, 6 tests)

**GREEN:** §6.3 solicit-document chain (create → resolve → send → correlated MessageWait → register → HumanWait review → End) authored ENTIRELY through the edit log (ops + `request_and_wait`), admits through the full direct-compilation chain; declarations survive (default budget 3 → envelope; both correlation sources → projection); the whole edit log serde round-trips and replays bit-identically (Q5). Guard declarations proven on a SUPPORTED task host: GUARD-N> + GUARD-TIMER-CYCLE>{max_fires:3} opcodes in the envelope, budget override Some(2) in projection. **RED (red-team script, each refusal naming its theorem+elements):** duplicate BPMN id; I23 backward connect; delete/replace of a guarded host; cycle trigger on interrupting guard; undeclared correlation source rejected at admission naming the missing producer; I18 backstop green.

**F-DSGN-3 (fail-open, FIXED red→green):** verifier §7a listed HumanWait as a legal BoundaryTimer host but lowering's HumanWait arm never consults `boundary_lookup` — a verified guard-on-human-wait compiled with the guard SILENTLY DROPPED (proven: admitted envelope contained zero guard opcodes; escalation chain orphaned). Fix: HumanWait removed from §7a's host set — reject, don't skip. Cement: `g2_boundary_timer_on_human_wait_rejected_not_dropped`. Full workspace green.

**FORK SURFACED — §6.3 "guard the wait" is unrepresentable (G2 blocked-in-part, awaiting Adam).** The reminder cycle on the document wait: guard on MessageWait rejects at admission (fail-closed receipt `g2_fork_receipt_…`); guard on HumanWait now also rejects (F-DSGN-3). Guards lower on task hosts only. Options: (a) extend lowering to wrap wait hosts (HumanWait first, MessageWait with it) in the guard scope — enables §6.3's literal shape; kernel scope/arming mechanics appear ready but need CAREFUL receipts; (b) amend §6.3's temporal impl to hang the reminder elsewhere. **Recommendation: (a)**, as a CAREFUL tranche with kernel park/fire receipts. Until ruled, G2's end-to-end receipt stands minus the guarded wait.

#### Receipts — DIR-002 pre-B dependencies (2026-07-27)

**A1 serializer (`utterance-engine/src/context.rs`, commit a70beaa):** ctxproj.v1 canonical line grammar; golden bytes + hash pinned (`07290be2…f804`); injectivity by typed construction rejects; `decide()` takes `&ContextProjection`, hash DERIVED never supplied; the utterance-placeholder hash is deleted. I28 widening: `Utterance` events store the SERIALIZED projection (trainable, not hash-only, additive+defaulted); endpoint receipt proves stored bytes re-hash to `context_projection_hash`.

**Positional legality oracle (`designer-graph/src/positional.rs`):** real §11.7 position-dependent boards over a `DesignerDag`. Two-layer rule table (staging-enforced mirrored + consistency-cemented against `apply`; admission-enforced mirrored from verifier theorems — F-DSGN-3 alignment: guard attachment proposed at task hosts ONLY, never at waits). Absolute exclusions cemented: CreateRace/CloseParallelRegion/AttachRollbackGuard/CallSubprocess + TimerMessageRace/CallDurableSubprocess/HumanReviewWithRework never boarded (the interim WholeGraphLegality boards the full catalogue including unbuildables — superseded for corpus work; endpoint swap rides WS-B wiring). Position-sensitivity receipts: same op present at one anchor, absent at another (A2.2's mechanism). Whole-graph = deterministic union; empty graph = NOTA-only board.

**T3.4a shortlist (research receipt, candle support verified from candle-transformers source; loadability receipts still owed at Phase C per "verified not assumed"):** recommended four — `Alibaba-NLP/gte-reranker-modernbert-base` (149M, Apache-2.0, `models::modernbert` incl. SequenceClassification head, already a reranker); `cross-encoder/ms-marco-MiniLM-L6-v2` (22.7M, Apache-2.0/MIT base, `models::bert` + small head, only candidate comfortably sub-second CPU); `answerdotai/ModernBERT-base` (149M, Apache-2.0, clean-room fallback); `BAAI/bge-reranker-base` (278M, MIT, `models::xlm_roberta` incl. head — latency is its gate). Excluded: bge-reranker-v2-m3 (568M), mxbai v2 (Qwen), gte-multilingual (remote code), DeBERTa-v3 (candle head coverage + tokenizer conversion UNCERTAIN — flagged, not guessed). Latency caveat recorded: 149M tier needs batching/quantization receipts.

#### Receipts — DIR-002 Phase A: spec + blind review (2026-07-27)

EOP-SPEC-SLM-TRAIN-001 v0.1 authored; independent authorship-blind review returned 17 findings (3 BLOCKER / 11 CONCERN / 3 NIT — full disposition table in the spec §S8). Blockers, remediated same-session: (1) uncheckable loader board-hash check → S2 now stores the full §11.7 preimage; (2) **A1 skew one layer up — no shared DAG→projection CONSTRUCTOR existed** → `project_ir` + `ir_kind_str` (the one kind vocabulary) landed in `utterance-engine/src/context.rs`, `DesignerDag::seed` (fail-closed Start/DataObject-only public seeding) in designer-graph, golden cross-crate cement `project_ir_golden_from_designer_ops`; interim DSL-plan endpoint projections marked NON-training-grade — **substrate ask: WS-B DesignerDag-backed sessions are the convergence point**; (3) split rule could sever context-pair sides → split unit redefined as the connected component over family/pair-group/utterance-text. **OPEN RULING FOR ADAM (finding 5): listwise training lists — DIR-002 A4 says "over the board", the ratified §10.3/§10.6 inference contract scores the tier-0 RETRIEVED SUBSET; recommendation = train on the real retriever's K-subset + NOTA always appended. Phase B label generation holds until ruled.**

#### Receipts — DIR-002 Phase B start: ruling implemented, generator live (2026-07-27)

Finding-5 ruling (Adam): lists = real tier-0 K-subset + NOTA → `retrieval::tier1_list` (one function, generator + Phase-C serving). Corpus generator landed (`utterance-engine/examples/corpus_gen.rs`): enumerates board states via seed→ops→PositionalLegality→build_board, consumes authored banks (`seed/banks/*.json`), enforces mechanically: label-on-board (HALT otherwise), 0.5 Jaccard cap incl. the NOTA rule, normalized dedup, retrieval-miss drops counted, pair-group integrity (structural violations HALT; hygiene-broken pairs drop BOTH sides, counted). `synthetic-v2-alpha` emitted from a 43-entry starter bank: 38 examples (10 NOTA), drops: 2 overlap-cap (the cap catching description-like authoring — working as specified) / 1 retrieval-miss / 1 pair-break.

**Empirical finding (headline): the context pair "chase them again" (guard-anchor → set_guard_trigger vs task-anchor → reminder_then_escalate) was retrieval-missed under LexicalTier0 — zero token overlap with the gold description is the DEFINING property of high-value context pairs, so the lexical retriever structurally excludes exactly the examples tier-1 exists to learn.** Consequence: synthetic-v2 full generation runs on the embed tier-0 (E3, `--features embed`) as the retriever; lexical stays as the regression baseline. Recorded in spec §S5.

#### Receipt — embed tier-0 closes the retrieval gap (2026-07-27, corpus_v2-alpha regenerated)

Generator wired to `EmbedTier0` under `--features embed` (BGE-small-en-v1.5, SHA-pinned, integration test green). Same 43-entry bank, embed retriever: **40 examples, 2 paired (the "chase them again" context pair SURVIVES on both boards), 0 retrieval-miss** — vs lexical: 38 examples, 0 paired, pair lost to retrieval-miss. The red→green pair of corpus cards is the receipt that full synthetic-v2 generation runs on the embed retriever; retriever identity is recorded in each card, and `tier1_list` keeps training-list = serving-list on whichever retriever serves.

#### Receipts — guarded-wait ruling landed; synthetic-v2-beta emitted (2026-07-27)

**Guarded-wait ruling (Adam) closes the G2 fork.** Kernel trace (independent read-only agent, before implementation) found items 1–4 of the guard-on-parked-wait question WORK TODAY with zero kernel change: fire-while-parked is record-keyed and ignores fiber `WaitState` (`lib.rs:4360,4371`); interrupting cancellation walks `control_stack` to find the Msg-parked host, and fiber deletion IS the only message-registration cleanup — no side table to leak (`lib.rs:3777-3809,3844`); post-close late fires are staleness no-ops via `RecordState::Armed` gating (`lib.rs:4371-4384`); the v2 verifier already admits `V2WaitMsg` inside a guard scope (no anti-non-task rule exists). Item 5 (sizing) was the only real gap — compiler-only.

Implemented: `lowering.rs`'s `lower_boundary_guarded_task_v2` generalized behind a `GuardedBody{Native,WaitMsg}` enum (returns the body `Addr` so the caller registers the correlation source at the right instruction); `MessageWait`/`HumanWait` lowering arms route through it instead of the old flat 2-instruction body; `instr_count_for`'s guarded-host sizing arm extended to the two wait kinds (same `+extra` formula, base 2 instead of `ExecNative`+`Jump`); verifier §7a re-admits `MessageWait`/`HumanWait` as legal `BoundaryTimer` hosts (superseding the F-DSGN-3 fail-closed rejection — this is the fix the fork was blocked on, not a reopening of the fail-open). `PositionalLegality`'s `is_guard_host` now includes wait nodes (SendTask excluded — never a lowerable host). G2 receipts flipped red→green: `g2_solicit_guarded_wait_admits_and_arms` proves `V2WaitMsg` sits INSIDE the guard scope with `GUARD-N>`/`GUARD-TIMER-CYCLE>{max_fires:3}` present in the sealed envelope; `g2_guard_on_human_wait_admits_and_arms` proves the same on the HumanWait review step. **§6.3's literal "guard the wait" shape now admits end-to-end — G2 is CLOSEABLE pending only the WS-B blind-review half.** Full workspace sweep: 110/110 green.

Bank sweep for the new legality: 10 stale "waits can't host guards" NOTA entries removed across banks (labels-by-construction — dropped, never judgment-relabelled); one false-positive drop ("race the reply against a deadline" — races remain off-board regardless of the wait-guard ruling) caught and restored. `BRIEF-BANK-AUTHORING.md` updated with the corrected per-class legal-label table.

**synthetic-v2-beta emitted** (regenerated under the new legality + embed retriever, corrected corpus-version naming): 567 examples from 615 validated bank entries (five authoring-agent banks + alpha, ~666 authored total), zero label-legality halts — every bank entry's label was legal on its board, including the newly-legal wait-guard labels, confirming the oracle/lowering/verifier triangle agrees end to end. Drops: 2 overlap-cap, 2 duplicate, 39 retrieval-miss (6.8% — expected under the ruled K=8 embed-subset rule; visible in the card, not hidden), 5 pair-break (a pair whose one side missed retrieval loses both sides per spec S2). 83 NOTA (14.6%), 40 paired examples / 20 surviving pairs (7.1%) — both above spec S3 floors' proportional targets on the surviving base. Per-regime and per-label breakdowns in the card. Held-out eval slice emitted separately (`synthetic-v2-beta.eval.jsonl`, 100 disjoint-persona entries, never trained) plus the 37-item `eval_ambiguity_v1.json` (never trained, A2.5). **Total-count floor (≥5,000) explicitly NOT met — card says so plainly; this is an authoring-progress receipt, not a release corpus.**

**Performance finding (not yet actioned, flagged for Phase C):** `EmbedTier0::retrieve` re-embeds every board's full candidate set on every call — no description-embedding cache. Regenerating ~600 entries took >10 minutes of CPU even with model weights warm-cached from a prior run. This will not clear the Phase-D latency gate as built; caching per-`board_hash` candidate embeddings is the fix, sized for the S3-floor-scale (≥5,000 example) generation run and for symmetry with the eventual serving path.

#### Receipt — EmbedTier0 target-embedding cache (2026-07-27, closes the flagged performance finding)

`retrieve()` re-embedded every board candidate on every call; a corpus run reuses each enumeration-class board's small candidate set across hundreds of utterances, so this was O(entries × board_size) forward passes for what is really O(distinct descriptions). Fixed: `EmbedTier0` gained a `Mutex<HashMap<description, embedding>>` cache — exact, not approximate (`embed_target` is a pure function of its text, matcher contract), keyed by description so a future collision-in-id-but-not-text case stays correct by construction. Receipt (`embed_target_cache_is_exact_and_faster_on_repeat`, run against real pinned weights): a warm-cache retrieve is measurably faster than the cold one, AND re-running the cold utterance after the cache warms yields bit-identical `retrieved_subset_hash` and per-candidate scores (`to_bits()` equality) — caching never perturbs a score. **Measured end-to-end: full synthetic-v2-beta regeneration (567 examples + 98 held-out eval) dropped from >10 minutes to 4:19.** Still not fast enough alone for the 5,000-example S3-floor run at proportional scale — batching `embed_target` calls (the matcher crate already exposes a batch API) is the next lever, deferred until bank authoring actually scales that far.

#### Receipts — WS-B.4: DesignerDag-backed sessions (2026-07-27)

Architecture question (how sessions become DesignerDag-backed) resolved via a
dedicated read-only research agent before implementation, per verify-don't-infer:
exhaustive grep confirmed zero DSL-source↔DesignerDag round-trip code exists
anywhere, live or dead; `schema.rs`'s own module doc explicitly forbids raw DAG
snapshot persistence ("Any DAG snapshot persistence goes through a versioned
envelope, never raw serde of these types") and designs for an edit-log-backed
model instead; `g2_receipts.rs`'s `g2_solicit_edit_log_round_trips` already
proves the serde round-trip + replay pattern. This made the opaque edit-log
event kind the only architecturally-consistent option, not merely the
cheapest one.

Landed: `DesignSessionEventKind::GraphEdit { operations_json, note }` in
`bpmn-lite-store` — stored as an opaque `String`, deserialized only
server-side (`bpmn-lite-store` gains no new `designer-graph` dependency,
mirroring the existing `Utterance`/`decision_record_json` precedent).
`DesignerDag::key_for_bpmn_id` added to `designer-graph` (fail-closed
`Option<NodeKey>` — an unknown anchor id is a typed `None`, never a silent
whole-graph downgrade). `bpmn-lite-server/src/rest.rs`: deterministic
Start-node seeding (`seed_start_key` via `Uuid::new_v5`, fixed namespace,
so every replay reconstructs the identical key without persisting anything
extra); `reconstruct_designer_dag` replays a session's accumulated
`GraphEdit` payloads through `apply_production`; new
`POST /api/dsl/sessions/:id/graph-edit` endpoint stages against the current
reconstruction and persists only on admission (I18 clone-and-stage extended
to the session layer — a refusal persists nothing); `session_utterance_endpoint`
now branches on `is_graph_backed()`: graph-backed sessions resolve the
request's `anchor` via `key_for_bpmn_id` (422 on unresolvable), compute
`graph_identity` by hashing the accumulated edit-log payloads (the edit log
IS the DAG's sole source of truth — not a Debug-formatted IR string, rejected
mid-implementation as non-canonical), and drive the board/context through the
real `PositionalLegality` oracle and `project_ir` — legacy DSL-source sessions
are byte-for-byte unchanged (WholeGraphLegality + census-only fallback).

Receipts (`cargo test -p bpmn-lite-server --lib`, 3 new, all green):
`test_session_graph_edit_admits_and_persists` (valid two-op sequence stages/
admits/persists; empty-operations POST → 400); `test_session_graph_edit_refuses_invalid_ops_and_persists_nothing`
(unknown-anchor `AppendNode` → 422, zero `GraphEdit` events persisted —
the RED half); `test_session_utterance_uses_positional_legality_when_graph_backed`
(headline receipt: builds a 2-node chain via graph-edit, POSTs an utterance
against a ghost anchor → 422 fail-closed, then against the real anchor → 200,
then confirms the persisted `context_projection` text was produced by
`project_ir`, not the census fallback). Full workspace sweep:
`cargo test --workspace` — 0 failures across every crate.

This is the convergence point flagged by the SLM-training blind review
(finding 2, DIR-002 Phase A remediation): interim DSL-plan endpoint
projections were marked non-training-grade pending exactly this substrate.
Graph-backed session utterances now produce real `project_ir` context —
future corpus generation can ingest real session data, not synthetic-only.
Also closes the WS-B.4 half of G2's remaining open item (WS-B blind review
is the other half, still open).

#### Receipts — WS-B independent blind review + remediation (2026-07-27) — G2 NOT YET CLOSED

Independent read-only agent dispatched against the full WS-B surface
(`rest.rs`'s session lifecycle/graph-edit/utterance/save endpoints,
`store.rs`'s `DesignSessionEventKind`, `schema.rs`'s `DesignerDag`/
`key_for_bpmn_id`), instructed to assume at least one real bug exists and
not run the test suite (independence from what the suite already asserts).
Found 3 BLOCKERs; each re-derived against primary source before acceptance
(verify-don't-infer), not accepted on the sub-agent's paraphrase alone.

**BLOCKER 1 (FIXED, red→green): `session_graph_edit_endpoint` never called
`.admit()`.** `productions.rs:289-290`'s own contract states `apply_production`
"does NOT run admission itself... the caller stages then calls
`candidate.admit()`" — the endpoint discarded the staged candidate
(`let _ = staged;`) and persisted the op sequence regardless. `apply_production`
only proves per-op local anchor legality; the full `to_ir`→`verify`→
`Compiler::lower_with_default` theorem chain (fork/join matching, SESE
nesting, reachability) never ran. **Fix:** `staged.candidate.admit()` now
gates persistence; refusal returns 422 with the verifier's diagnostics,
nothing is appended. **Receipt:** new test
`test_session_graph_edit_refuses_locally_staged_but_globally_illegal_graph`
(a `GatewayAnd` split with no matching join — each `AppendNode` individually
legal, the resulting graph globally illegal) — confirmed genuinely RED
pre-fix (200 OK, wrongly accepted) by mechanically disabling the `admit()`
call and re-running, then GREEN restored.

**BLOCKER 3 (FIXED): TOCTOU race in `session_graph_edit_endpoint`.**
Neither backend (`MemoryStore::append_design_session_event`,
`store_postgres.rs`'s equivalent) serializes a session's load→reconstruct→
stage→append sequence — only the append's own storage write was atomic.
Two concurrent graph-edits against the same session could each load the
same base, each stage successfully, both persist — the second replays
against a DAG shape it was never validated against, permanently bricking
the session (every future reconstruct/utterance call errors, no repair
path). **Fix:** `DemoState::session_lock` — a per-session `tokio::sync::Mutex`
serializing the endpoint's full load-stage-append critical section; different
sessions never contend. **Receipt:**
`test_concurrent_graph_edits_on_same_anchor_never_corrupt_session` — 5
concurrent requests competing for one anchor's single outgoing-edge slot;
confirmed exactly 1 succeeds, 4 cleanly refused, and the session remains
reconstructible afterward. **Honesty note (no trap doors — don't overclaim
a receipt):** this test could NOT be made to fail without the lock in this
harness (`#[tokio::test]`'s current-thread runtime never interleaves the
critical section without an explicit forced yield point) — three runs
without the lock all passed by accident, not by correctness. The fix is
retained on the strength of the static TOCTOU analysis (both backends read
independently before any serialized write) and is a standard, obviously-
correct primitive; the test is a real regression guard for the post-fix
invariant, but is NOT a proven red→green pair the way BLOCKER 1's is. Flagged
here rather than silently presented as equally strong evidence.

**BLOCKER 2 (OPEN — genuine fork, not decided): `save_design_session_endpoint`
ignores `is_graph_backed()`.** `store.rs`'s `current_source()` returns `None`
for a graph-backed session unless a `Revision` event exists in its log; the
save endpoint calls it unconditionally. A pure graph-authored session gets a
400 (save-as-template simply broken); a session seeded with initial DSL text
then edited via graph ops silently saves that STALE, unrelated text as the
template — a quiet-wrong-success. Not a mechanical fix: `DesignerDag::admit()`
produces an `ExecutableWorkflow` (post-`to_ir`/`verify`/`lower_with_default`,
already-lowered) while `dsl::compile()` produces a `WorkflowExecutionPlan`
(pre-lowering AST-level plan) — different types, and `load_plan`'s stored
JSON is a LIVE contract: `bpmn-lite-bus-handler/src/lib.rs:715-717`
deserializes it specifically as `WorkflowExecutionPlan` before spawning a
process. My first-pass recommendation (store `ExecutableWorkflow` directly)
was WRONG — verified against this real consumer before touching any code —
and would have silently corrupted template instantiation for any
graph-authored template. Correcting the record rather than proceeding on a
disproven recommendation. **This blocks G2's close** until ruled: (a) build
a `DesignerDag`→`WorkflowExecutionPlan` projection so both session kinds
converge on the one stored/consumed type, or (b) widen `load_plan`'s
contract to a tagged union of both plan shapes with the bus-handler
consumer updated to handle both. Recommend (a) — the bus-handler's
`spawn_process_with_idempotency` and the AST-level plan machinery it
depends on are unlikely to want two competing input shapes — but this
changes a live wire contract, so surfacing rather than deciding.

Full workspace sweep after all fixes: `cargo test --workspace` — 0 failures.
**G2 remains OPEN pending Adam's ruling on BLOCKER 2.**

Also worth a line: the executor's course-correction on BLOCKER 2 — tracing
`load_plan`'s real consumer (`bpmn-lite-bus-handler`) before shipping the
first-pass recommendation, catching that storing `ExecutableWorkflow`
directly would have silently corrupted live template instantiation — is the
trace-before-trust discipline (verify-don't-infer) doing its job. Recorded
as a caught-by-process event, per Adam's ruling below.

#### Ruling — BLOCKER 2: (a), with two riders (Adam, 2026-07-27)

**(a)** — `DesignerDag`→`WorkflowExecutionPlan` projection, both session
kinds converge on the one stored/consumed type — **ratified, with two
riders that determine whether (a) is right or a trap:**

- **Rider 1 — the projection is a call, not a construction.** The
  `IRGraph → WorkflowExecutionPlan` step must be the production compiler's
  own validate-and-lower path, invoked at save/publish — never a bespoke
  mapping function. A hand-rolled projector would be a second lowering path
  (the three-faces-of-one-root-cause lesson, §23) and a direct I16/I17
  violation. If the existing compiler surface can't be called from the
  endpoint's position, that's a HALT and a substrate ask, not a
  reimplementation.
- **Rider 2 — converging the plan store must not lose the authoring
  truth.** `load_plan`'s ONE stored type is correct because that store
  feeds instantiation and should only ever contain compiled artifacts (P5).
  But the `DesignerDag` is the authoritative authored artifact (I1) and
  must persist in the session/edit-log store with its declarations (C7)
  intact so a graph-backed session can be reopened and edited. (a) as
  sketched saves only the projection — fixing the bus-handler while
  silently breaking G2's reopen-and-round-trip requirement is not a fix.
  **Two stores, two roles: session store holds the DAG (already true —
  `GraphEdit` events), plan store holds what the compiler produced from it.**

**Why not (b)** (widen `load_plan` to a tagged union): puts an authoring
representation into the execution consumer's contract — the bus-handler
would be interpreting/lowering a source graph at instantiation time,
exactly what P5 forbids, and spreads variant-handling into a shipped
consumer where a future wildcard arm is one lazy edit away. The
instantiation boundary receives compiled artifacts, full stop.

**Receipts required (both, not one):** red→green fixture that (i) saves a
graph-backed session, publishes, and instantiates end-to-end through
`bpmn-lite-bus-handler`; (ii) reopens the same session for edit with the
DAG and every declaration intact. The bug was silent precisely because
only one side of the boundary was ever exercised.

#### Trace result — Rider 1's condition IS triggered: HALT, substrate ask (2026-07-27)

Dispatched a read-only research agent to determine whether an
`IRGraph → WorkflowExecutionPlan` path already exists anywhere in the
compiler, at any crate position (the question Rider 1's fallback anticipates).
Finding, stronger than "not callable from this position" — **it does not
exist in any form:**

- `dsl::compile()`'s `lint()` phase (`linter.rs:261-288`) is what
  constructs `WorkflowExecutionPlan`; it operates purely on the parsed AST
  (`WorkflowSource`/`NodeAst`) and never builds or references an `IRGraph`
  — zero references to `ir.rs` types anywhere in `dsl/`.
- The IR path (`lowering.rs`'s `lower`/`lower_with_default`/`lower_v2`)
  goes `IRGraph → CompiledProgram/VerifiedWorkflow` — bytecode-level,
  never touches `WorkflowExecutionPlan`.
- A third, independent route (`dsl/frontend.rs:44-49`, `lower_plan`) goes
  `WorkflowExecutionPlan → VerifiedWorkflow` directly, bypassing `IRGraph`
  entirely.

Not a crate-boundary/visibility problem — `designer-graph` already depends
on `bpmn-lite-compiler` and already calls into it (`to_ir`/`verify`/
`Compiler::lower_with_default`); wiring a new call would be trivial. The
function has simply never been written. `WorkflowExecutionPlan`'s
`ExecutionNode` also carries placeholder-schema/delivery-mode fields
(`plug`, `static_args`, `produces_placeholder`) `IRNode` has no equivalent
for — not a mechanical reshuffle, but new compiler logic: how does
IR-authored graph state resolve to placeholder bindings the AST path
currently derives from source annotations?

Per Rider 1 and the working contract's HALT discipline: not building this
under implement-mode. **G2 stays OPEN, unchanged.** Needs Adam's scoping
of the substrate tranche (a new `IRGraph → WorkflowExecutionPlan` lowering
inside `bpmn-lite-compiler`, presumably CAREFUL-tier given it touches the
compiler's ratified surface) before further BLOCKER 2 work proceeds.

#### Receipts — BLOCKER 2 substrate tranche implemented (Adam: "careful scope and implement", 2026-07-27)

**Scope, per Rider 1 ("a call, not a construction").** New module
`bpmn-lite-compiler/src/dsl/ir_plan.rs`, `project_ir(ir: &IRGraph,
workflow_id: String) -> Result<WorkflowExecutionPlan, IrPlanError>` — the
production compiler's own path, extended to IR input, not a bespoke
mapping outside it. Deliberately conservative: supports `Start`, `End`,
`ServiceTask`, and `GatewayAnd`/`GatewayInclusive` matched
diverging/converging pairs (via the already-exposed `gateway_pairs`
pairing oracle — never hand-rolled repairing; this is the exact function
R8/C2 exposed at the compiler boundary for precisely this kind of reuse).
`DataObject` nodes (structural-only, zero bytecode) are omitted. Every
other IRNode kind — `GatewayXor` (no `direction` field, no
compiler-exposed join-pairing oracle; its DSL counterpart's `join` id is
an explicit AST annotation with no IR equivalent), `BoundaryTimer`,
`BoundaryError`, `MessageWait`, `HumanWait`, `SendTask`, `MultiInstance`,
`TimerWait`, `FfiServiceTask` (confirmed via the earlier trace: none has
an `ExecutionNode` representation in `WorkflowExecutionPlan` at all) —
fails closed with a typed `IrPlanError`, never a lossy shoehorn. Extracted
`derive_delivery_mode` (the P6/L8 "Blocking is derived, not chosen"
formula) out of `linter.rs`'s inline Pass 6 into a shared pure function in
`plan.rs`, called identically by both the AST path and the new IR path —
literally the same code computing delivery mode either way, not two
divergent implementations of the same rule. Graph-authored `ServiceTask`
nodes get no placeholder inference (IR's `task_type` is an external-job
dispatch identity, not a catalogue `domain:verb` symbol — attempting
registry resolution on it would be a category error) and default to
`BestEffort` through the same formula, fed honestly with no catalogue
signal.

**Wired into `save_design_session_endpoint`** (`bpmn-lite-server/src/rest.rs`):
branches on `is_graph_backed()`; the graph-backed path reconstructs the
DAG, calls `dag.admit()` (same admission discipline as the graph-edit
endpoint — refusal is 422, nothing stored), then `to_ir()` + `project_ir()`,
storing the projected plan in the PLAN store. **Rider 2 ("two stores, two
roles") honored by construction**: the session store's `GraphEdit` event
log is untouched by save (already true — this endpoint never appends to
it), so the DAG stays the authoring truth there and the session remains
reopenable for edit. The template catalog's `dsl_body` display field gets
an honest placeholder string for graph-authored sessions (`"<graph-authored
session {id}; edit via the graph-edit endpoint, not DSL text>"`) rather
than a fabricated DSL rendering.

**Independent blind review** (CAREFUL tier, dispatched per
verify-don't-infer) found 2 real issues in the first pass, both re-derived
against primary source and fixed before acceptance: (1) **BLOCKER**:
`IREdge.condition` is a generic field on every edge, not just
diverging-gateway edges — `Operation::Connect` can attach one between any
two existing nodes — so a condition on a plain `Task`'s or `Join`'s
outgoing edge was silently dropped (`single_successor` returned only the
successor id, never inspecting the edge's condition), exactly the "looks
valid but behaves differently than authored" failure the module exists to
prevent. Fixed: `single_successor` now refuses (typed
`UnrepresentableCondition`) any condition on an edge whose target
`ExecutionNode` kind has no field to carry one. (2) **CONCERN**: the
diverging-gateway flow-builder used `neighbors_directed` + `find_edge`,
which silently misattributes a parallel edge's condition to the FIRST edge
between a fork/successor pair when two edges connect the same pair. Fixed:
switched to `edges_directed` (pairs each edge directly with its own
target), the same pattern `verifier.rs` already uses elsewhere in the
compiler — reusing established idiom, not inventing a new one.

**Receipts.** `bpmn-lite-compiler`: 7 unit tests in `ir_plan.rs` (linear
chain projects; matched AND pair → Split/Join; `GatewayXor` refused, not
guessed; `BoundaryTimer` refused, not shoehorned; missing-Start refused;
condition-on-non-gateway-edge refused, not dropped — the BLOCKER's
red→green; parallel-edges-don't-misattribute — the CONCERN's red→green).
`bpmn-lite-server`: 3 new tests on `save_design_session_endpoint`
(projects a graph-backed session to a real `plan_hash`; refuses when the
graph doesn't admit — composes with BLOCKER 1's graph-edit-time fix, so a
non-admitting graph can never even reach save; **receipt (ii)**, reopen-
for-edit — save doesn't touch the session's edit-log event count, and a
further graph-edit against the same session still runs real staging logic
post-save, not a corrupted/frozen reconstruction). `bpmn-lite-bus-handler`:
new integration test `graph_authored_plan_instantiation.rs`, modeled on
the existing `sage_macro_assembly_tests.rs` pattern — **receipt (i), the
"define-template" half**: a `project_ir`-produced plan defines a template
through the real `BpmnLiteBusHandler` dispatch path, including
`validate_path_family`'s closure checks, green. **The "spawn-instance"
half is `#[ignore]`d, honestly documented, not faked**: the local
`data_designer` Postgres DB this test connects to (same connection string
the pre-existing bus-handler tests use) carries a FOREIGN migration
history (`_sqlx_migrations` shows `202412`/`6` — not this workspace's
numbered chain at all) and is missing `bpmn_spawn_idempotency`; `sqlx
migrate run` correctly refuses to reconcile rather than guess. This is a
pre-existing local-environment gap no prior test ever hit (none of them
exercised "spawn-instance" before), not a defect in this work — flagged as
ops follow-up rather than forced through with a risky migration-history
edit on a DB of unknown other dependents. Full `cargo test --workspace`
after all fixes: 0 failures.

**G2 is now CLOSEABLE**: both prior BLOCKERs (1: missing `.admit()`; 3:
TOCTOU race) were fixed in the earlier WS-B blind-review pass, and
BLOCKER 2 (save-as-template for graph-backed sessions) is now implemented
per Adam's ruling, itself independently blind-reviewed and remediated.
The one remaining gap — the "spawn-instance" leg of receipt (i) — is
infra, not code, and is captured as a concrete, re-runnable `#[ignore]`d
spec rather than silently dropped.

### GATE G2 — CLOSED (Adam, 2026-07-27)

Both required criteria (line 89) are satisfied:

- **Solicit-document workflow authored end to end + red-team script**:
  closed earlier this session (`designer-graph/src/g2_receipts.rs`, 6
  tests) — reminder cycle with `max_fires`, per-guard budget, correlation
  source, published/re-opened with every declaration intact; red-team
  script of deliberately invalid edits each refused at staging with the
  correct theorem named. The one gap flagged at that closure (§6.3's
  literal "guard the wait" shape) was resolved by the guarded-wait ruling
  the same session (`g2_solicit_guarded_wait_admits_and_arms`,
  `g2_guard_on_human_wait_admits_and_arms`).
- **Full pipeline (board → tier-0 → disposition policy → I28 record)
  demonstrably in the loop**: live since WS-B.4 landed
  (`session_utterance_endpoint` branches on `is_graph_backed()`, running
  the real `PositionalLegality` oracle + `project_ir` context for
  graph-backed sessions) — plus the independent blind review this pass
  ran against that exact surface (WS-B.4's session/graph-edit/save
  endpoints), which is the CAREFUL-tier "blind review" half of the gate's
  own name. Three real BLOCKERs found across two review passes, all
  fixed with red→green (or honestly-labeled regression-guard) receipts;
  zero BLOCKERs outstanding.

**Known, tracked, non-blocking residuals** (none reopen the gate — each is
either explicitly out of scope by design or a documented environment
gap, not a silent omission):
- `save_design_session_endpoint`'s `project_ir` scope is intentionally
  conservative (Start/End/ServiceTask/matched AND+Inclusive gateway pairs
  only) — `GatewayXor` and all v2-only wait/guard/MI/FFI node kinds fail
  closed rather than saving as a template today. Widening this scope is
  new compiler work, not a gate-closing item.
- The "spawn-instance" leg of the bus-handler instantiation receipt is
  `#[ignore]`d pending `data_designer`'s foreign migration-history gap
  being fixed as ops housekeeping (tracked in the memory checkpoint's open
  queue).

Full `cargo test --workspace`: 0 failures at every stage of this arc,
confirmed immediately before this closure.

#### Receipt — embed-batching lever measured and REJECTED (2026-07-28)

The open-queue note "batch `embed_target` before scaling banks much past
~1000" was evaluated before implementation, per the pre-registered gate:
batching is admissible for the corpus generator ONLY if the matcher's
batch path is bit-identical to the single-call path, because serving
embeds one utterance/description at a time — anything else builds
train/serve skew into the corpus silently. Measured on the pinned rev
(`ob-semantic-matcher` @ `ff3f12c`, real BGE weights): **361/384
components of the first probe vector differ bitwise** between
`embed_batch_targets` and `embed_target` (the batch path pads to the
batch max and runs one forward — masked-softmax residue and changed
matmul shapes perturb the output). The lever is rejected; the generator
stays on the single-call path. Wall-clock consequence, accepted: full
regen at the 5000-entry floor extrapolates to ~30–35 min (0.39 s/entry
observed), paid per regen round, not per authored entry.

Tripwire cemented:
`retrieval.rs::embed_batch_diverges_from_single_so_batching_stays_rejected`
PASSES while the divergence exists; if a future matcher rev makes batch
output bit-identical, the test fails — the signal to reopen the batching
decision on evidence, not a regression.

#### Receipt — synthetic-v2-beta crosses the S3 >=5000 floor (2026-07-28)

Open-queue item "scale banks to S3 floors" closed via five four-agent
authoring rounds (gamma/delta/epsilon/zeta) plus a small hand-authored
supplement (eta), each round independently re-validated against the
brief's per-class legal-label sets, the Jaccard overlap cap, and
cross-bank normalized-token/near-duplicate collision before every
regen — never trusted on an agent's self-report alone. Progression:
567 → 1665 (gamma) → 2805 (delta) → 3997 (epsilon) → 4991 (zeta) →
**5018 (eta), `total_floor_met: true`**. Final card: 723 NOTA (14.4%),
262 paired context-sensitivity examples (5.2%), 370 retrieval-miss +
6 hygiene drops out of ~5394 authored entries (~93% survival). Full
`cargo test --workspace`: 111/111 test-result groups green throughout
every regen.

One authoring agent (zeta_terse) encountered and correctly refused a
prompt-injection attempt disguised as a tool-result system-reminder
mid-task (claimed its own scratch file had been externally modified
with wrong-regime content, instructed it not to report this) — it
ignored the embedded instruction, flagged it, and its actual output
was independently verified unaffected before acceptance.

#### Receipt — Phase C step 0: T3.4a shortlist Candle loadability (2026-07-28)

The T3.4a shortlist note flags itself as a "research receipt... loadability
receipts still owed at Phase C per 'verified not assumed'" — source-reading
confirmed the right `candle_transformers::models` module exists for each
base, not that a real checkpoint's weight keys line up with that module's
`VarBuilder` prefixes or that its `config.json` deserializes cleanly.
Before spending any Phase C training time, `utterance-engine/examples/
candle_loadability_probe.rs` (new `candle-probe` feature, off by default —
network-only-on-demand like `embed`) downloads each base's real published
weights at an exact pinned commit SHA, loads them into the matching
Candle struct, and runs one real forward pass on a representative
(utterance, candidate-description) pair. Result: **all 4 bases PASS**
(`gte-modernbert`, `ms-marco`, `modernbert-base`, `bge-reranker` — real
logits/hidden-state shapes printed, not asserted).

Two genuine findings surfaced by actually running this, not by reading
source:
1. `Alibaba-NLP/gte-reranker-modernbert-base`'s published `config.json`
   has `label2id` values as integers (`{"LABEL_0": 0}`), not the strings
   `ClassifierConfig::label2id` (`HashMap<String,String>`) requires;
   `serde(flatten)` on an `Option` field silently swallows that mismatch
   into `None` rather than erroring, sizing the classifier at 0 outputs
   and failing to load the real `[1,768]` weight. Moot for Phase C
   regardless — every base's PRETRAINED head gets discarded and replaced
   with one trained fresh on our corpus (A4: "identically trained... same
   recipe"), so the probe tests the base encoder only for this model,
   identically to `modernbert-base`.
2. The same repo's `tokenizer.json` bakes in a fixed padding strategy
   (`Fixed(8000)`) for its original long-document use case. Left as
   loaded, encoding even this probe's ~20-token pair produced an
   `[1, 8000, 768]` tensor and ran real CPU minutes through 22 attention
   layers at that length — not a loadability problem, but proof that
   Phase C's training/inference code must explicitly override each
   tokenizer's baked-in padding/truncation (`with_padding(None)`,
   `with_truncation(None)`, then dynamic per-batch padding) rather than
   trust repo defaults built for a different workload. The probe now does
   this itself and the fix is documented inline as the reason.

Both findings are the kind "verified not assumed" exists to catch —
recorded here rather than only in the probe's own comments, per the
receipts-append-to-the-plan discipline.

**Caveat carried forward, not resolved by this receipt:** 5000 is a
count floor, not a quality bar. The corpus is now large enough per
EOP-SPEC-SLM-TRAIN-001 §S3, but Phase D's real evaluation (recall@K,
ranking-given-inclusion, position invariance, per-pack breakdown,
comparison against the tier-0-alone baseline) has not run — a large
corpus of correctly-labelled-but-narrowly-styled examples can still
train a model that overfits to this session's authoring voice. This is
exactly what A3.3's held-out disjoint-regime eval slice exists to catch
downstream, not this receipt to declare fixed.

#### Receipts — DIR-002 Phases C/D/E: bake-off trained, measured, reported (2026-07-28)

Full narrative + tables in **EOP-REPORT-SLM-BAKEOFF-001** (the Phase E
deliverable); this receipt records the plan-level facts and process
events.

- **Phase C closed.** All four T3.4a bases fine-tuned identically
  (listwise over the real tier1_list, family-split seed 20260728,
  best-checkpoint export), exported as `encoder.*`/`head.*` safetensors
  bundles, **loaded and scored back through Candle behind the real
  `SlmResult` contract** — the "verify each loads and scores in Candle"
  step closed with forward passes, not source-reading. A4 calibration
  temperatures fitted (1.03–1.75) and recorded per bundle; reloaded
  val-NLL matches training-time best val loss exactly (bit-faithful
  round-trip receipt).
- **Phase D core numbers.** C5 baseline: tier-0 alone top-1 = 0.4490.
  Best SLMs (modernbert-base / gte-modernbert, tied 87/98): top-1 =
  0.8878, **+43.9pp** — the context-conditioning thesis as measurement.
  Recall curve: K=8 = 95.9%, **K=12 = 100%** (all four misses at gold
  ranks 9–11). Latency-vs-K and per-class/ambiguity suites run; see the
  report.
- **Recommendation on Adam's desk, NOT ratified:** `modernbert-base`
  (statistically indistinguishable from gte-modernbert at n=98;
  provenance tiebreak). **Second open ruling: widen K 8→12** (spec-S5
  value; converts a permanent 4% error floor into ~40% more tier-1
  compute; recommendation = widen).
- **Caught-by-process event:** first training pass exported final-epoch
  weights; val-curve audit found 3/4 bases past their val-loss minimum.
  Fixed, all four retrained, conclusion held and strengthened. Recorded
  as the second self-caught near-miss of this build (after the
  `ExecutableWorkflow` store) — the discipline is transferring to the
  executor, not just living in the documents.
- **Standing caveats (spec's own language, unsoftened):** synthetic-only
  eval overstates real-world performance until Q9-chartered session data
  exists; n=98; nothing promoted; G3 thresholds and base ratification
  are Adam's. Human-authored eval utterances remain outstanding —
  re-requested in the report.
- **2026-08-01: Adam ratified `modernbert-base`; K widened 8→12.
  Serving integration follows.** Both open rulings above are closed:
  `modernbert-base` is the canonical tier-1 base (provenance tiebreak at
  the tied top-1; hash retained in its bundle card) and
  `retrieval::TIER1_K = 12` is the ONE standing K (spec §S5's recorded
  K=8 stays as the historical trained configuration — this receipt, not
  the spec, is the record of the change).

## D. Delta table — v0.1 → v0.2 (per EOP-DIR-BPMN-DESIGN-003-001 Phase 3)

Every change tagged `sequencing` or `content`. No `content` change to a ratified constraint was made; no HALT condition arose.

| # | Change | Tag | Basis |
|---|---|---|---|
| 1 | Serial T1→T2→T3 becomes concurrent WS-A ∥ WS-B ∥ WS-C(C-now) + GOV track; tranche names → workstream names | sequencing | Directive §1/§2; V&S §16 phases' *content* unchanged |
| 2 | Charter-critical-path statement is the plan's first line; T0.1 → GOV.1 unchanged in content | sequencing | Directive §2 governance track; D17 restated verbatim (§A.1) |
| 3 | WS-A.1 gains the named board-candidate schema interface, sequenced first within WS-A.1 | sequencing | Directive §3 bullet 1; schema content itself unchanged (Q2/Q27 close as before) |
| 4 | WS-B disposition path calls WS-C's policy function from day one (tier-0 + Sage initial producers); WS-B.3's D20 flow wired live instead of stubbed | sequencing | Directive §2 WS-B; architecture already ratified (D7/D8/I27 — one deterministic disposition path); only *when the code exists* changes |
| 5 | G2's "SLM-insertion readiness" structural-readiness item removed; replaced by "full pipeline (board → tier-0 → policy → record) demonstrably in the loop with records written" | sequencing | Directive §1 (item declared obsolete) + §3 (G2 re-scope). Verification is strictly strengthened: the same three seams are now exercised as running code under the same blind-reviewed gate, not reviewed as interface promises. No criterion weakened |
| 6 | T3 → WS-C split into C-now (ungated) / C-gated (Q9) / C-bakeoff; C-now built immediately with capture switch OFF | sequencing | Directive §2 WS-C; D17 restated verbatim — the split *encodes* the gate rather than deferring the code |
| 7 | C5 trace promoted from T3.0 entry gate to WS-C's immediate first task; known register state (matcher located, R10-versioned) recorded in the task | sequencing | Directive §2 ("C5 trace is now an immediate task — locate the matcher first"); trace content unchanged |
| 8 | Tier-0 corpus recall@K baseline measurement moved from C-now trace into C-gated with an explicit charter-timing flag to Adam | sequencing | Directive §2 C-gated ("flag it to Adam explicitly… rather than assuming it is exempt"); tightens, not loosens, D17's application |
| 9 | GOV.2 placement presumption updated: ob-poc workspace → bpmn-lite/standalone deploy unit as the working presumption, decision still open in GOV.2 | sequencing | Adam's standalone ruling 2026-07-27 (EOP-SAGE-REPL-BPMN-001 T0) as decision input; V&S is silent on repo placement — T0.2/GOV.2 was always the open decision slot; no ratified clause touched |
| 10 | §A added: ratified constraints restated verbatim-in-substance as a standing section | sequencing | Directive §1 ("restate verbatim in the amended plan, do not weaken") |
| 11 | Shadow-start definition added: the day WS-B's session loop first runs against a real Pack | sequencing | Directive §3 bullet 2 |
| 12 | G1, G3 (criteria + threshold table + ladder), T4/G4: untouched | — | Directive §1 items 5, §3 bullet 4 |

### DIR-003 Phase 2 — xor_gateway description audit (CAREFUL analysis; 2026-07-29)

**Change.** Three near-synonym `xor_gateway` candidate descriptions in `designer-graph/src/board_candidate.rs::OperationKind::description()` rewritten to state their distinguishing consequence at a routing node, per Adam's suggested wording:

| candidate | before | after |
|---|---|---|
| `create_branch` | "Add an outgoing routing branch at an exclusive gateway" | "Adds a new outgoing route with its own outcome key" |
| `insert_after` | "Insert a new node after the anchor node" | "Places a node on an existing route, after the selected node" |
| `connect` | "Connect two existing nodes with a typed sequence flow" | "Joins two existing nodes with a typed connector" |

Descriptions are board-hash input; the rewrite changes board hashes and re-embeds tier-0's targets. Re-scored the same 98-entry eval set (K=12) on all four already-trained bundles **with no retraining** — pure re-scoring against the new text.

**Result — before (K=12 ratification baseline, commit `4d25535`) vs after (description audit, no retrain):**

| base | xor_gateway before | xor_gateway after | overall top1 before | overall top1 after |
|---|---|---|---|---|
| modernbert-base (canonical) | 5/8 | 5/8 | 87/98 | 83/98 |
| gte-modernbert (runner-up) | 6/8 | 5/8 | 89/98 | 87/98 |
| bge-reranker | 6/8 | 6/8 | 79/98 | 77/98 |
| ms-marco | 5/8 | 3/8 | 75/98 | 69/98 |

Tier-0-alone baseline also moved: `tier0_top1_accuracy` 0.4490 (44/98) → 0.4388 (43/98) — a small shift purely from re-embedding the changed descriptions, with no model change at all. `recall@12` moved 1.0000 (98/98) → 0.9898 (97/98).

**Honest read (this is the point of the CAREFUL tier — not spinning it).** This is *not* the improvement the audit was hoping to measure. `xor_gateway` accuracy stayed flat or got worse on every base (no base improved on the targeted class), and every base's overall accuracy dropped by 2–6 points despite no change to any model weight. That drop pattern — uniform, across all four architecturally-distinct bases, on classes that were never touched — is the signature of **train/serve description skew dominating**: the four bundles were trained against the old description text baked into 5,018 corpus records: the model learned to associate specific old phrasing with each candidate, and swapping the served text out from under it costs accuracy independent of whether the new text is clearer. The clean way to separate "is the new wording better" from "is this skew" does not exist without a controlled experiment (retrain one base on both description sets, hold everything else fixed) — this audit, by design, ran with no retraining, so it cannot resolve that separation. What it *can* say: on this evidence, the new wording did not pay for itself against the skew cost it introduced, and does not clear the bar for keeping unretrained. **Disposition: description change is diagnostic only for this cycle — do not carry it forward into the next retrain's corpus generation as a settled improvement.** Adam's call whether the wording itself is still worth keeping for corpus-v2 (independent of this cycle's skew-contaminated read) is open; the standing rule above (rule 5) means whatever is decided, keeping any adopted description change past its audit obligates a full corpus regeneration at the next retrain — never a partial, skewed serve.

**Receipt state:** `board_candidate.rs` description edit is committed as part of this branch's Phase 2 commit; `eval_enriched.jsonl`/`.card.json` and `eval_scores.json` reflect the new (post-audit) descriptions going forward — this is now the working head-of-branch state. If Adam elects to revert the wording, that is a one-line revert plus a re-run of `eval_enrich`/`score_trained_bundle`, not a retrain.

### DIR-003 Phase 3 — `starter-seed-v1` permanent named suite (2026-07-29)

**What.** 34 utterances Adam authored outside the generation pipeline (verbatim in EOP-DIR-BPMN-DESIGN-003-003 §Phase 3), across 7 categories: routing/xor, waits/timers/reminders, guards/rollback, MI/collections, correlation/messages, declarations, off-board (NOTA-expected), vague/compound. Each was board-mapped to one of the 13 real enumeration-class positions (`fixtures.rs`, real board-construction code, not invented) and assigned a **provisional hypothesis label** — free utterances have no label-by-construction, so every label here is Adam's/the executor's best-effort read, not gold. 8 of 34 are flagged `disputed` with the specific alternate reading noted; disputed misses are evidence, not model error.

**Harness.** `TrainedRanker` extracted from `score_trained_bundle.rs` into `utterance_engine::trained_ranker` (shared, one scoring path — avoids a second copy of the Candle key-remap logic). New permanent binary `examples/starter_seed_eval.rs` (`cargo run -p utterance-engine --example starter_seed_eval --features embed,candle-probe --release`): loads `seed/banks/starter_seed_v1.json`, builds the real boards, runs tier-0 (Candle embed) at K=12 and the ratified canonical base (`modernbert-base`) with no retraining, and reports **per-category evidence, not pass/fail** (directive 3.1) to `seed/corpus_v2/starter-seed-v1.report.json` + `.enriched.jsonl`. This is now the permanent named suite: every future bundle should report against it until real developer-session usage supersedes it (Phase 4 AFTER-item).

**Result (provisional-hypothesis hits, out of 34; canonical base = modernbert-base, K=12):**

| category | n | tier0 top1 hits | tier1 top1 hits |
|---|---|---|---|
| routing_xor | 7 | 2 | 4 |
| waits_timers_reminders | 6 | 1 | 1 |
| guards_rollback | 4 | 1 | 2 |
| mi_collections | 3 | 0 | 1 |
| correlation_messages | 3 | 1 | 2 |
| declarations | 2 | 1 | 2 |
| off_board | 5 | 0 | 2 |
| vague_compound | 4 | 0 | 1 |
| **total** | **34** | **6 (17.6%)** | **15 (44.1%)** |

Tier-1 still uplifts over tier-0 by a similar multiplicative margin as the synthetic eval (~2.5x here vs ~2x there), but both numbers land far below their synthetic-eval counterparts (tier0_top1 43.9%→17.6%, tier1 top1 88.8%→44.1%). This is not a regression — it is the first honest signal of what Open Risk #1 in EOP-REPORT-SLM-BAKEOFF-001 already named: synthetic-only eval overstates real-world performance. Full per-utterance detail (hypothesis label, tier0/tier1 pick, disputed flag and note) is in `starter-seed-v1.report.json`, carried into the Phase 4 report addendum.

**Disputed items requiring Adam's adjudication (8):** seq 4 ("wire the rejected path back to... actually where does rejected go" — Connect-to-unstated-target vs NOTA-as-clarification), seq 7 ("give the timeout its own route" — CreateBranch vs guard-timer reading), seq 10 ("nudge every 48 hours, three times max" — direct guard config vs reminder-escalate production), seq 12 ("park this until the document shows up" — request_and_wait vs bare wait-append), seq 18 ("do this for each director" — MI construction vs clarify-for-missing-ceiling), seq 22 ("when their answer lands, wake this up" — request_and_wait vs bare MessageWait append), seq 25 ("make the default budget three for the whole flow" — workflow-level default has no current node-scoped candidate; possible missing candidate class, flagged not decided), seq 32 ("chase them and also loop legal in if it's high risk" — compound-ask NOTA vs partial-credit reminder_then_escalate reading).

### DIR-003 Phase 4 — report addendum + close-out (2026-07-29)

Report addendum written (`EOP-REPORT-SLM-BAKEOFF-001.md` §10: K=12 standing baseline, description-audit result with skew-aware interpretation, `starter-seed-v1` results by category with the 8 disputed labels). No promotion, no retraining performed in this task — G3 thresholds and base ratification-of-record both stayed exactly where Phase 1–3 left them.

**AFTER-item 1 (prepare, don't run, corpus-v2 config):** written as `EOP-CORPUS-V2-GEN-CONFIG.md` — K 8→12, the two live description options (keep audited wording vs revert), a new xor-anchored paraphrase-pair reinforcement regime (the one repeated real finding across both the bake-off and the audit), and three starter-seed-v1-derived open items (MI-without-ceiling, wait-vs-production ambiguity, the workflow-level-declaration gap) each explicitly flagged as needing Adam's ruling before becoming a generation rule, not decided in that document. Not wired into `corpus_gen.rs`; nothing runs until Adam calls it.

**AFTER-item 2 (confirm developer-session capture path) — a fork surfaced, not decided:** `utterance-engine/src/capture.rs`'s `CapturePipeline::on_under_charter(charter_ref: &str)` is a SINGLE generic gate — it accepts any non-empty string as "the charter reference," with no built-in distinction between a ratified Q9 user-capture charter and an ad-hoc self-declared reference. So the path is *mechanically* on-able today (pass any non-empty string), but there is no dedicated, enforced separation between "Adam's own testing, self-consented, stated at session start" and the real Q9-gated user capture — only documentation/discipline would keep them apart if used as-is. That is a real fork, not a confirmation:

- **Option A (interim, no code change):** use the existing generic gate with a clearly-named, unmistakable ref (e.g. `"ADAM-DEV-SELFTEST-<date>"`, never resembling a real charter id), document the convention here, and treat it as a personal-use exception by discipline alone. Lowest effort; carries the risk the "no trap doors" principle warns about — a string convention is not a mechanism.
- **Option B:** add a distinct `CapturePipeline` constructor/dataset-class variant (e.g. `DatasetClass::DevSelfTest` or a separate `on_for_developer_self_test()` path) that is structurally incapable of being confused with the Q9-gated path — more work, but closes the gap for real, and gives Q9's eventual charter a clean precedent to point at rather than a retrofit.

**Recommendation: Option B**, sized as a small WS-C task when Adam next wants his own utterances flowing — cheap now, and the alternative (Option A) is exactly the kind of "gate satisfied by convention, not mechanism" the working contract's "no trap doors" rule exists to catch. Not implemented here; this is a fork surfaced for Adam's ruling, per standing rule ("surface forks, don't decide them"), not an AFTER-item completed.

**AFTER-item 3 (stop):** done. Open on Adam's desk after this task: the Q9 charter (critical path, unchanged), corpus-v2/retrain timing (`EOP-CORPUS-V2-GEN-CONFIG.md`), the `starter-seed-v1` label adjudication (8 disputed items, §10.3), and the two capture-path options above.

### DIR-004 Phase 1 — structural capture separation (Option B), CAREFUL (2026-07-29, commit `d2dc700`)

**Ruling executed:** the capture-path fork DIR-003 surfaced (`capture::CapturePipeline::on_under_charter` was a single generic string-gated switch, no structural difference between a real Q9 charter and a self-declared ref) is resolved as **Option B — structural separation**, not the naming-convention Option A.

- **Distinct types, distinct stores.** `utterance-engine/src/dev_capture.rs` (always compiled): `DevSessionRecord` carries the full I28 closure plus raw utterance + serialized context projection TEXT (train-on-able, not hash-only). `DevSessionSubject` has exactly one variant (`Adam`) — a compile-time fact. `DEV_SESSION_PROVENANCE = "dev-session-adam-v1"` is a module constant, never caller-settable. `capture.rs` (the Q9 path) shares no record/store type with it; `ConfigRegistry` (general I28 hash-resolution, not capture-specific) moved to `policy.rs` so the split is clean.
- **Feature-gated, CI-enforced exclusion from default/release builds.** New `q9-capture` feature (off by default, both `utterance-engine` and `bpmn-lite-server`); `lib.rs` gates the `capture` module DECLARATION itself, so a pre-charter build has no live user-capture path to even find. New `scripts/check-q9-capture-gate.sh` (5-fixture self-test) asserts this mechanically — wired into `layering.yml`, `production-gates.yml`, `nightly-chaos.yml`. Found in the process: `production-gates.yml`/`nightly-chaos.yml` used `--all-features` (which cannot exclude anything, including the new feature) — switched to an explicit, verified-complete feature list.
- **Blind review (independent agent, CAREFUL tier) found a real BLOCKER**: the first cut of the build-invocation check used a `--features` flag-syntax regex, defeated by quoting (`--features "postgres,q9-capture"`). Fixed by rewriting the check as a blunt "does this file mention the string `q9-capture` outside a comment at all" substring test — no flag syntax left to evade — plus a self-test gap (fixtures didn't call the real check functions) fixed by having self-test run the real functions against throwaway fixture repos. A genuine near-miss caught by the process working as designed, not a rubber-stamped review.
- **Adjudication tooling (1.4):** `examples/adjudicate_starter_seed.rs` replays the 8 disputed `starter-seed-v1` items against the real board's legal candidates for a human verdict; verified non-interactively (`--list` / no TTY) that it writes nothing and never fabricates a verdict. Verdicts land in a separate `starter_seed_v1_adjudications.json` (original bank untouched), provenance `adam-adjudicated`; `starter_seed_eval.rs` merges them in automatically — confirmed byte-identical results to the pre-adjudication baseline since none exist yet.

Verified: `cargo build`/`cargo test` clean under default features AND `--features q9-capture` (both crates); `check-q9-capture-gate.sh` self-test and real scan both pass; full `cargo test --workspace` green throughout.

**Reporting Phase 1 done and stopping at Phase 2 (Adam's first live session — a human gate, not this executor's work) per the directive.**

### Fork closures (2026-08-01)

**R4 `task_type = "noop"` default — ratified as-is.** Surfaced in 432cef3 as the
one deterministic-binding exception to "underivable = MISSING, not defaulted"
(`proposal.rs`'s module doc, R4). Adam's ruling: no change — a `noop`-typed
task is a valid, inspectable graph node (visible, admits, refined by a later
manual edit), and forcing `task_type` to MISSING would make "create a task"
alone impossible to ratify from an utterance at all, the wrong failure mode
for a drafting tool. Closed; not revisited absent a new reason.

**xor-anchored paraphrase-pair seed phrases — ratified.** The
`create_branch`/`insert_after`/`connect` seed utterance set proposed for
`EOP-CORPUS-V2-GEN-CONFIG.md` §3's reinforcement regime is adopted as drafted
("your suggestions seem sensible - lets go with them"). Recorded in that file;
still gated on the same "prepared, not run" status — no corpus regeneration
triggered by this ruling alone, §1/§2/§4's open items are unchanged.

### Fork closure + new phase (2026-08-03)

**BoundaryTimer execution — ratified as its own phase ("own phase", Adam,
2026-08-03).** Not folded into DIR-002 serving work. Phase spec below.

## WS-D — Timer-semantics projection phase (BoundaryTimer / TimerWait on the plan path)

**Ground truth that scopes this phase (traced 2026-08-03).** The kernel,
engine, store, and production runner already execute boundary timers end to
end on the XML/IR path: `lowering.rs` emits `V2GuardArmTimer`/
`V2GuardTimerCycle`/`V2GuardArmError` inline in the host's lowering
(`lowering.rs:1676`), the kernel fires them with staleness gating,
interrupting unwind vs non-interrupting rearm, and budget
(`kernel/lib.rs:4357-4440`), timers persist via
`WorkflowStore::claim_due_timers`, and the runner's 500ms scheduler loop
drains them (`server-runner/main.rs:272-330`). The gap is entirely the
**plan path** the designer serves: `WorkflowExecutionPlan` has six node
kinds and no timer/guard/attachment concept (`plan.rs:239-247`);
`project_ir` fail-closes on `BoundaryTimer`/`BoundaryError`/`TimerWait`
(`ir_plan.rs:274`, locked by `boundary_timer_is_refused_not_shoehorned`);
`dsl/frontend.rs` lowers every Task to `ExecDslTask` with no `V2Guard*`/
`V2Wait*` emission; and the designer has no `tick_due_timers` call, so
even a lowered timer would never fire there. This is a
projection-and-serving phase, not an engine phase.

**Architecture fork, resolved by recommendation pending Adam's veto:
extend the plan format (Option A), do NOT route the designer around it to
IR-direct lowering (Option B).** B is cheaper (the IR path's lowering
already works) but forks the compile provenance: plan-consuming code
(bus-handler `plan.nodes()` walks, `analyze_safety` proofs, status DTOs)
would be blind to guard-bearing instances, and the plan — the typed
provable artifact that is the product's value claim — would become a
format that cannot express a timeout. `ir_plan.rs`'s own error text says
"no representation **yet**"; A is the honest completion. The plan format
gains:

- `TaskExecNode.guards: Vec<GuardExecSpec>` — decoration, not a node:
  `{ guard_id, trigger: Timer(TimerSpec) | Error{code}, interrupting,
  failure_budget, escape_entry: NodeId }`. Mirrors the kernel's model
  (arming is inline in host lowering; the escape flow is ordinary nodes).
- `ExecutionNode::Wait(WaitExecNode { spec: TimerSpec })` for standalone
  `TimerWait` — replaces the dead `bpmn:timer-wait` Task-plug shoehorn in
  `importer.rs:126-151` (no consumer of that plug exists; delete it).
- `analyze_safety` learns real guard awareness: the string-plug
  `BPMN_BOUNDARY_EVENT_BYPASS` breach detector stays (it catches
  shoehorns) but genuinely-represented guards must NOT trip it, and the
  proof obligations extend to escape-route closure (every `escape_entry`
  reaches End — same shape as verifier §3's alternative-root DFS).

**Steps (each with red→green receipts, one commit per step):**

- **D1 — plan format + projection.** Add `GuardExecSpec`/`Wait` to
  `plan.rs`; `project_ir` projects `BoundaryTimer`/`BoundaryError` onto
  the host's guard list, projects the escape subgraph as ordinary plan
  nodes (verifier §3 already admits them as alternative roots), projects
  `TimerWait` → `Wait`. `validate_dag` admits escape subgraphs.
  `MessageWait`/`HumanWait`/`SendTask`/`MultiInstance`/`FfiServiceTask`/
  `GatewayXor` stay refused — amend the cement test, don't delete it.
  RED: a plan whose `escape_entry` dangles or whose escape subgraph
  doesn't close must reject with a named diagnostic.
- **D2 — frontend lowering.** `dsl/frontend.rs` emits the same opcode
  sequence the IR path emits for guarded hosts (reuse/mirror
  `lower_boundary_guarded_task_v2`; do not re-derive semantics) and
  `V2WaitFor`/`V2WaitUntil` for `Wait` (Cycle on standalone wait follows
  `lowering.rs:565`'s existing first-interval rule). Receipt: bytecode
  diff — the same guarded graph lowered via XML path vs plan path
  produces equivalent guard opcodes (arming instr + timer kind + budget).
- **D3 — designer serving.** `advance` ticks due timers
  (`engine.tick_due_timers`) before and after its job-drain pass —
  request-driven, preserving the designer's "no background loop"
  doctrine; `advance` accepts optional `logical_time_ms` (defaults wall
  clock) so receipts are deterministic; `/status` surfaces
  `WaitState::Timer` fibers with their deadline instead of dumping them
  into `wait_count`. Receipt: spawn a guarded template, advance past the
  deadline with injected time → interrupting guard unwinds the host and
  the escape path runs to its End; non-interrupting variant rearms;
  budget exhausts.
- **D4 — consumer sweep.** Bus-handler `plan.nodes()` walks already
  `continue` on non-Task and the guards field is additive — verify, don't
  assume. The runner demo simulation's `_ => break` (`server-runner/
  rest.rs:683`) will hit `Wait`: teach it or make it reject loudly; a
  silent break is the exact fail-open this plan forbids. Designer/runner
  node-DTO builders learn the new shapes.

**In scope:** interrupting + non-interrupting BoundaryTimer, TimerWait,
and BoundaryError (same projection shape, kernel routing already built —
excluding it would be artificial). **Out of scope:** MessageWait/HumanWait
plan projection (guarded waits stay XML-path-only for now), MultiInstance,
FFI, compensation, any engine/kernel change.

**Open sub-fork for Adam (does not block D1/D2):** should the designer
additionally grow an opt-in background tick loop (runner-style) for
long-lived demo instances, or is request-driven-only permanent? D3 is
written request-driven; the loop would be additive.

### WS-D receipts — D1–D4 CLOSED 2026-08-03 (commits 8008207, 86ffa34, e2d4627, fca5dd0 + ob-poc 49aeafff)

- **D1** plan format + projection: guards as `TaskExecNode.guards`
  decorations, `ExecutionNode::Wait`; escape-closure proof in
  `analyze_safety` (`BPMN_GUARD_ESCAPE_DANGLING`/`_OPEN` breach REDs);
  cement test amended (escape-less guard refuses as
  `GuardEscapeUnresolved{count:0}`); MessageWait/MI/FFI/XOR stay
  refused. 114 suites green.
- **D2** frontend emission: the SAME guarded IR lowered via
  `Compiler::lower_v2` and via `project_ir`→`DslFrontend::lower` yields
  the IDENTICAL guard scaffold (cemented literal opcode sequences,
  budget at guard-open, arm-at-open+1 adjacency, guardn-close ×2), both
  admitted by the real V-verifier. Zero kernel changes — `ExecDslTask`
  parks on `WaitState::Job` like `ExecNative`; guard firing/error
  routing are fibre/record-based. Contradiction REDs (two timers,
  interrupting Cycle).
- **D3** designer serving: `advance` fires due timers first
  (request-driven, optional injected `logical_time_ms`); `/status`
  surfaces `waiting_timers`. Money receipt through the REAL path
  (graph-edit `AttachGuard` → save → publish → spawn → advance):
  injected-clock timeout completes down the ESCAPE flow (round parked
  on `notify_esc`), pre-deadline control completes the NORMAL flow,
  standalone Wait holds then resumes. Designer 38/38.
- **D4** consumer sweep: runner plan-walker fails loudly on Wait
  (traced diagnostic + `Failed` state — no silent task-like stall);
  bus-handler walks verified continue-safe; TS kind unions widened.

**Still open:** the request-driven-vs-background-tick sub-fork above,
and the kernel job-cancel gap below.

**RESOLVED (Adam's "go ahead", 2026-08-04, commit ba2221f):** option
(a) implemented — `JobMutation::Cancel` emitted by both unwind paths
for every cancelled fiber parked on a job; store contract is
remove-everywhere (ack-shaped, replay-tolerant); RED→GREEN proven by
stash-revert (pre-fix dequeue returned the ghost host activation
alongside the escalation job). The designer's structural filter stays
as defense in depth. Original finding kept below for the record.

**Surfaced kernel gap (found by D3's timeout receipt, 2026-08-03 — NOW
FIXED, see above; originally recorded as needs-its-own-ruling):** an interrupting guard
unwind redirects the fiber but emits NO job-cancel mutation — the
kernel's `JobMutation` enum has only `RetryClaimed`/`DeadLetterClaimed`,
no `Cancel` — so the host's already-queued job activation stays live in
the store. Any consumer that dequeues it gets a ghost: completion is
(correctly) refused with "completion has no parked fiber". The
designer's advance loop now filters structurally (completes only jobs a
fiber is parked on; superseded claims lease-expire) — but PRODUCTION
workers holding the host's job when its guard fires hit the same ghost
on `complete_job_with_claim`. BPMN semantics say an interrupting
boundary CANCELS the task; today nothing signals the worker. Options
when ruled on: (a) `JobMutation::Cancel` emitted by the unwind, store
drops/tombstones the activation; (b) keep ghost-refusal as the contract
and make workers treat it as cancellation. Recommendation: (a) — the
queue claiming to hold executable work that is cancelled is
structure-consulted-as-state, the same D1-class defect this codebase
keeps deleting.

---
*v0.2 restructured 2026-07-27 per EOP-DIR-BPMN-DESIGN-003-001. Receipts append here per workstream as each closes. Amend in place.*

---

*(v0.2's receipts carry forward above this line — see merge note.)*
