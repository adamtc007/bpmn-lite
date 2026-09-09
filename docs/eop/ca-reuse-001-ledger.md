# EOP-PLAN-CA-REUSE-001 — bpmn-lite tranche ledger

- **BL-T1** (2026-09-09) — write-once guard on pending-invocation ack; `StoreError::Conflict` mirrors sibling; postgres + memory parity; red→green ×2. Receipt: `ca-reuse-001-BL-T1-receipt.md`.
- **BL-T2** (2026-09-09) — **STOPPED at design fork**: hash stability (Ring 2 recompute-and-compare at claim) and zero-domain-noun grep are mutually exclusive as specified; three options + recommendation (O2, tag-preserving `Legacy(u8)`) in `ca-reuse-001-BL-T2-fork.md`. Awaiting ruling. BL-T3 blocked behind it.
- **Follow-ups** logged in `ca-reuse-001-followups.md` (demo bindings on prod path; embedded ob-poc manifests; decimal→f64 aliasing; `bind_placeholder_from_payload`; inbox re-stamp).
