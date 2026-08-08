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
   ratification flow. `POST /api/dsl/sessions/:id/palette/select` now accepts a legal
   move ID, creates an explicit palette-selection receipt and enters that same workbook
   answer/preview/ratification path.
2. `POST /api/dsl/sessions/:id/graph-edit` resolves a single direct deletion to its
   current, fully bound semantic move ID when the legal board proves exact equivalence;
   all other tapes remain attributed `lower_level_direct_edit`. Broader operation-to-move
   equivalence remains to be qualified before this route can claim full convergence.
3. Sage now has a read-only alias for the same policy-filtered board and move IDs as
   the palette, the bounded canonical attempt history projection, and a
   position-bound candidate-guidance endpoint. Guidance returns typed applicability,
   a pack-derived rule explanation and bounded recovery options; it cannot select,
   preview, ratify or mutate a move. The response carries its reconstructed
   `DesignPosition`. Sage can also retrieve a retained attempt receipt by its
   canonical attempt identity, including its position, rule-explanation,
   feedback-option and correction identities; attempt receipts remain owned by the
   interaction path that actually attempts a move.
4. General session/event read-back remains broader than the dedicated Sage view; the
   remaining audit/history compatibility boundary has not been receipted.
5. Legacy text-backed sessions retain only their explicit compatibility utterance
   boundary: gameboard, Sage-board and Sage-guidance requests fail closed rather than
   manufacturing a graph-authoritative position. The Phase 7 removal-call-site audit
   is still outstanding while that rollback window remains open.
6. A duplicate ratification after a lost response now resolves through the durable
   terminal proposal audit keyed by the canonical workbook/proposal identity; it
   returns the retained terminal receipt without appending another graph revision.
   The broader restart/lost-response/duplicate-request and concurrent-revision tape
   replay through the in-process API adapter is not yet a Phase 7-qualified suite.

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
