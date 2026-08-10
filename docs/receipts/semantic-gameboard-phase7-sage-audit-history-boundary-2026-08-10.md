# Semantic gameboard Phase 7 — Sage audit/history compatibility boundary

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Closes red-receipt item 4 (`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`):
"General session/event read-back remains broader than the dedicated Sage
view; the remaining audit/history compatibility boundary has not been
receipted."

## What was mapped

`GET /api/dsl/sessions/:id` (`get_design_session_endpoint`, `rest.rs`)
serializes the entire, unbounded, unredacted `DesignSessionRecord` — every
event ever appended, verbatim. Sage's four dedicated read views
(`/api/dsl/sage/sessions/:id/{gameboard,history,audit,attempts/:id}` plus
`/guidance/:candidate_id`) are each windowed (`MAX_WINDOW = 64`) and typed
through `semantic-decision-contracts` structs (`DesignPosition`,
`MoveAttemptReceipt`, `ProposalWorkbook`) that structurally cannot carry:

| Field (general surface only) | Where it lives | Sage exposure |
|---|---|---|
| `Revision.dsl_source` | full DSL text, every revision | never |
| `Utterance.text` / `.response` | raw dialogue | never |
| `Utterance.context_projection` | full training-grade context | never |
| `Utterance.decision_record_json` | full I28 decision record | never |
| `Utterance.gameboard_disposition_json` | — | never (Sage sees only `belief`/`attempts`) |
| `GraphEdit.operations_json` | raw `Operation` tape incl. `NodeKey(Uuid)` | never (Sage uses `GraphElementRef`, a validated BPMN-id string) |
| `ProposalAudit.bound_plan_json` | compiled `Operation` tape incl. `NodeKey` | explicitly dropped by a `..` match arm |
| `ProposalAudit.dry_run_diagnostics` | can carry raw Rust `Display` error text (`rest.rs` refusal paths) | explicitly dropped by the same `..` match arm |
| Event count | unbounded | capped at 64 |

## Red → green proof

Added `rest::tests::test_sage_endpoints_exclude_general_session_surface_fields`:

1. Builds a graph-backed session, captures the raw internal `NodeKey` used
   for a graph edit, submits an utterance with a distinctive canary string
   as its raw text, and drives a real palette selection through to a
   `ProposalAudit` event (so the audit endpoint has real content to redact
   — an empty `entries` array would make that half of the check vacuous).
2. Confirms both canaries, and the `context_projection` / `operations_json`
   keys, are genuinely present on `GET /api/dsl/sessions/:id` — the
   superset relationship is real, not assumed.
3. Confirms neither canary, nor any of the seven general-surface-only field
   names, appears anywhere in any of Sage's five endpoint responses
   (`gameboard`, `history`, `audit`, `attempts/:id`, `guidance`).

**Verified this is a real gate, not a tautology**: temporarily reintroduced
the exact defect class item 4 warns about — patched
`sage_session_audit_endpoint` to also capture and emit
`ProposalAudit.dry_run_diagnostics` (mirroring the general endpoint's
verbatim exposure) — reran the test, confirmed it failed red with the
leaked field quoted in the assertion message, then reverted the patch and
confirmed 71/71 green again. Diff-and-revert only; no defect shipped.

## Scope and an open fork

This receipts the **content boundary**: Sage's response types are
structurally incapable of carrying the fields the general surface exposes,
and a canary-based regression test now proves and holds that line.

It does **not** establish a **request-time authorization boundary**. There
is no per-caller identity anywhere in `bpmn-lite-server-designer` — every
handler uses the same hardcoded `TenantId::new("demo")`
(`DesignerState.tenant_id`), and nothing distinguishes "a request coming
from Sage" from "a request coming from any other caller of the router." The
boundary Sage observes today exists only because Sage's own client
integration chooses to call `/api/dsl/sage/...` routes instead of
`/api/dsl/sessions/:id` — a code-level contract on the caller side, not an
enforced gate on the server side. Any caller that reaches the router and
knows (or enumerates) a session id gets the full unredacted record via the
general endpoint regardless of whether it identifies as Sage.

Whether that residual gap needs closing now (e.g. a distinct caller-identity
check gating `/api/dsl/sessions/:id` to non-Sage/internal callers) or is
accepted scope for a single-tenant demo system with no auth layer elsewhere
either, is a fork for a ruling — not decided here.

## Results

- `cargo test -p bpmn-lite-server-designer --all-features`: 71/71 (was
  70/70; net +1, 0 regressions).
- `cargo check --workspace --all-targets --all-features`: clean.

Phase 7 remains RED pending the multi-operation direct-edit tranche
(deferred from the Phase 2 item 9 generalization), the removal-call-site
audit for the legacy text-backed rollback window (item 5), the broader
restart/lost-response/duplicate-request/concurrent-revision Phase
7-qualified suite (item 6 — concurrent-revision drift itself is now
covered, see `semantic-gameboard-phase7-concurrent-revision-drift-2026-08-10.md`),
and the host-blocked libFuzzer smoke (item 8).
