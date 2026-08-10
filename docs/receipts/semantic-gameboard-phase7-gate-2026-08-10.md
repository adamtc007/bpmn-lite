# Semantic Gameboard Phase 7 gate

Date: 2026-08-10

Phase: 7 — converge APIs and user surfaces

Entry authority: Phase 7 red receipt
`docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md`.

Status: **GREEN.**

## Disposition of every red-receipt item

1. **Palette/workbook/preview/ratification path** — was already implemented and tested
   as of the red receipt; unchanged and still passing (76/76 suite).
2. **Direct-edit equivalence** — closed in two steps this session:
   `docs/receipts/semantic-gameboard-phase2-direct-edit-equivalence-generalization-2026-08-10.md`
   (single-`Operation` candidates, v0.8) and
   `docs/receipts/semantic-gameboard-phase2-multi-op-tranche-2026-08-10.md` (the 6
   multi-op candidates, v0.9). All 19 candidates reachable via `materialize_workbook`
   now resolve through the general recover-synthesize-materialize-compare mechanism —
   18 by proof, 1 pair (`prod.reminder_then_escalate` /
   `prod.non_interrupting_notification`, structurally indistinguishable by operation
   content) by ruled fail-closed refusal.
3. **Sage read-only surfaces** — was already implemented and tested; unchanged.
4. **Sage audit/history compatibility boundary** — closed:
   `docs/receipts/semantic-gameboard-phase7-sage-audit-history-boundary-2026-08-10.md`.
   Canary-based test proves Sage's four dedicated views structurally exclude everything
   that makes the general session/event read-back broader; verified as a real gate by
   temporarily reintroducing the exact leak class and confirming red before reverting.
   Flags an open, separately-tracked fork: this closes the *content* boundary, not a
   *request-time authorization* boundary (no per-caller identity exists anywhere in the
   server today).
5. **Legacy rollback / removal-call-site audit** — closed, no code changes required:
   `docs/receipts/semantic-gameboard-phase7-legacy-rollback-audit-2026-08-10.md`. The
   fail-closed graph-authoritative boundary is implemented and tested at five call
   sites; the `rank`/`score_serving` call-site audit is complete (structurally
   unreachable from a legacy board); thin-board removal is correctly out of Phase 7
   scope by design (Phase 9 rollout decision, not an engineering gap).
6. **Restart/lost-response/duplicate-request/concurrent-revision suite** — closed:
   `docs/receipts/semantic-gameboard-phase7-concurrent-revision-drift-2026-08-10.md`
   (concurrent-revision, the harder two-different-proposals case) and
   `docs/receipts/semantic-gameboard-phase7-fault-tape-suite-2026-08-10.md` (coverage
   matrix for all four categories, plus one new test closing the one real gap found:
   duplicate-request against a non-drift, workbook-completing `/answers` submission).
7. **Public-surface/dependency-direction gate** — re-verified fresh, not assumed:
   `python3 scripts/check-semantic-gameboard-boundaries.py` → `{"status": "pass", ...}`,
   exit 0, after every change landed this session. No public API widening.
8. **libFuzzer smoke** — closed for real, correcting an error made earlier in this
   session: `docs/receipts/semantic-gameboard-phase7-libfuzzer-smoke-2026-08-10.md`.
   The "host-blocked, no nightly sanitizer toolchain" framing (inherited from the
   2026-08-07 red receipt without re-verification) was stale — this environment has
   `nightly` and `cargo-fuzz` installed. Actually built and ran the designer fuzz
   target: 37,690 executions in 30s, 0 crashes/hangs/OOMs. This is an unconditional
   green, not the documented exception the user ruled for when the (incorrect)
   host-blocked framing was presented.

## Required red assertions — re-confirmed

- A palette-selected move cannot be observed to bypass workbook, preview, explicit
  ratification or compiler admission. — unchanged, passing.
- A direct manipulation is either the same typed move as palette/language or is an
  explicitly attributed lower-level edit. — items 2 above; `"ambiguous_candidate_shape"`
  added as a third, explicitly-named outcome (never a silent guess).
- A Sage response cannot be derived from an internal Rust error string. — item 4 above.
- A refused attempt advances only session history and preserves graph revision. —
  `rest::tests::test_session_utterance_uses_positional_legality_when_graph_backed`,
  asserted directly ("wrong attempts never append an authoritative graph edit").
- Every new API/fuzz/tooling consumer stays on a reviewed facade. — item 7 above.

## Open items carried forward (not blocking this gate)

- The request-time authorization gap named in item 4's receipt (no per-caller identity
  distinguishing a Sage request from any other router caller) — a fork for a ruling
  whenever wanted, not a Phase 7 requirement.
- Phase 6 promotion evidence remains pending, as scoped by the red receipt's own "Scope
  retained" section — this gate does not touch or claim it.

## Results

- `cargo test -p bpmn-lite-server-designer --all-features`: 76/76, 0 regressions across
  the full session (started at 63/63 on the red receipt's baseline).
- `cargo check --workspace --all-targets --all-features`: clean.
- `python3 scripts/check-semantic-gameboard-boundaries.py`: pass, exit 0.
- `cargo +nightly fuzz run bpmn_binding_extract -- -max_total_time=30 -runs=200000`:
  37,690 runs, 0 crashes.

Phase 7 is GREEN. Phase 6 promotion evidence remains the standing prerequisite for any
subsequent phase claiming production rollout authority — unchanged by this gate.
