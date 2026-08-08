# Semantic Gameboard Phase 7 red receipt

Date: 2026-08-07

Phase: 7 — converge APIs and user surfaces

Entry authority: Phase 6 structural-green receipt
`docs/receipts/semantic-gameboard-phase6-structural-green-2026-08-07.md`.

Baseline: `4d1236e7c2f21a4b6c58bc00180b58c03244e07c`

Status: RED — implementation has not yet converged all input surfaces.

## Observed production drift

1. `GET /api/dsl/sessions/:id/gameboard` returns a graph-backed
   `DesignPosition` and concrete legal moves, while
   `POST /api/dsl/sessions/:id/utterance` can create a workbook, preview and
   ratification flow. There is no palette-selection endpoint that accepts a legal move
   ID and enters that same workbook path.
2. `POST /api/dsl/sessions/:id/graph-edit` now labels every raw operation tape as an
   attributed `lower_level_direct_edit`; it still does not resolve an equivalent
   semantic move where one exists.
3. Sage now has a read-only alias for the same policy-filtered board and move IDs as
   the palette, plus the bounded canonical attempt history projection. It still lacks
   typed rule and feedback retrieval with the required snapshot and receipt identities.
4. General session/event read-back remains broader than the dedicated Sage view; the
   remaining audit/history compatibility boundary has not been receipted.
5. Legacy text-backed sessions remain isolated in code, but the Phase 7 compatibility
   boundary and removal-call-site audit have not been receipted.
6. Restart/lost-response/duplicate-request and concurrent-revision tape replay through
   the in-process API adapter is not yet a Phase 7-qualified suite.

## Required red assertions

- A palette-selected move cannot be observed to bypass workbook, preview, explicit
  ratification or compiler admission.
- A direct manipulation is either the same typed move as palette/language or is an
  explicitly attributed lower-level edit.
- A Sage response cannot be derived from an internal Rust error string.
- A refused attempt advances only session history and preserves graph revision.
- Every new API/fuzz/tooling consumer stays on a reviewed facade.

## Scope retained

Phase 6 promotion evidence remains pending. This Phase 7 work must not treat synthetic
fixtures as real-session performance evidence, authorize learned-policy promotion or
introduce automatic application.
