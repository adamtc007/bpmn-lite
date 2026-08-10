# Semantic gameboard Phase 7 — legacy rollback / removal-call-site audit

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Closes red-receipt item 5 (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
"Legacy text-backed sessions retain only their explicit compatibility utterance
boundary... The Phase 7 removal-call-site audit is still outstanding while that
rollback window remains open."

## What this item actually is

Not a code gap — an audit-and-scoping item. The red receipt names two distinct legacy
surfaces; each has a different disposition.

## 1. Fail-closed graph-authoritative boundary — implemented and tested

`DesignSessionRecord::is_graph_backed()` (`bpmn-lite-store/src/store.rs:161`) is a pure
function of accumulated event history: `true` iff the session has ever appended a
`GraphEdit` event. No feature flag or config switch exists — the split is structural and
permanent per session, "purely additive, no existing session's behavior changes
underneath it" (`store.rs` doc comment).

It gates every graph-authoritative surface, fail-closed (`409 CONFLICT`), confirmed at
five call sites in `bpmn-lite-server-designer/src/rest.rs` (broader than the red
receipt's own text, which named only two):

| Surface | Line |
|---|---|
| Semantic palette (`gameboard` shared handler, palette + Sage alias) | `5838` |
| Sage guidance (`sage_move_guidance_endpoint`) | `6106` |
| Palette move selection | `5420` |
| Save-as-template | `5608` (explicit `legacy_authoring_path` error, not a bare conflict) |
| Compiled graph endpoint (routes to the DAG-authoritative path only when graph-backed) | `6266` |

Test proof: `rest::tests::test_session_utterance_runs_shadow_pipeline`, `rest.rs:7314-7321` —
loops a freshly-created text-only session through all three of `/gameboard`,
`/sage/.../gameboard`, `/sage/.../guidance/op.insert_after` and asserts `409 CONFLICT` on
each. Real assertions in a green test, not a stub; part of the 75/75 passing suite.

## 2. Removal-call-site audit — two named legacy surfaces, two different outcomes

**`Tier1Ranker::rank` / `score_serving` (K-subset v3 helpers,
`utterance-engine/src/trained_ranker.rs`): audit complete, no live call sites.**
Confirmed: reachable only from an `#[ignore]`d test
(`tier1_v3_bundle_refuses_legacy_k12_route`, `trained_ranker.rs:1035`, requires a
manually-set `SLM_BUNDLE_DIR`) and one non-serving example
(`utterance-engine/examples/score_trained_bundle.rs`). Production serving
(`rest.rs:369-382`) carries an explicit comment explaining why it deliberately never
calls `t1.rank(...)`: every loadable tier-1 bundle's card requires
`pair_serializer_id`/`pair_serializer_hash` matching `pair::serialize_candidate_pair`,
which a legacy/thin board — having no `CandidateSemanticSlice`s — cannot produce. There
is no correct way to route a legacy board through tier-1 at all; the code degrades to
tier-0 with an honest producer identity instead. Structurally unreachable, not merely
avoided by convention.

**Legacy thin-board serving** (`build_board` + `WholeGraphLegality` oracle, one call
site at `rest.rs:4523`+ in `session_utterance_endpoint`'s non-graph-backed branch):
**cannot be audited for removal yet — correctly out of Phase 7 scope.** This *is* the
legacy-session compatibility path item 5 itself describes; removing it would remove the
"rollback window" it names. The plan doc is explicit that this is later work:
`EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 7 Work item 7 ("Keep legacy sessions isolated and
clearly identified until their rollback window closes") and Phase 9 Cleanup ("After the
rollback window: remove thin-board production construction; remove exclusive
lane-priority serving; remove legacy v2 textualisation APIs...").

**Duplicate candidate/disposition DTOs** (Phase 7 Work item 6, "Remove duplicate
candidate/disposition DTOs after compatibility tests pass"): none found in the current
tree (`grep` for `struct.*Candidate.*Disposition` across the workspace returns nothing).
This sub-item is moot — either never introduced as duplicated code, or already resolved
before this audit.

## Disposition

No code changes required. The rollback window itself — closing the legacy thin-board
serving path — is a product/rollout decision (Phase 9), not an engineering gap Phase 7
can close unilaterally; attempting to "audit" its removal now would be auditing a path
that is still intentionally live by design.

## Results

No new tests required (existing coverage already proves the fail-closed boundary);
75/75 `bpmn-lite-server-designer --all-features` suite (unchanged by this item, verified
current via the same run as the multi-op tranche receipt).
