# EOP-PLAN-CA-REUSE-001 — bpmn-lite tranche ledger

- **BL-T1** (2026-09-09) — write-once guard on pending-invocation ack; `StoreError::Conflict` mirrors sibling; postgres + memory parity; red→green ×2. Receipt: `ca-reuse-001-BL-T1-receipt.md`.
- **BL-T2** (2026-09-09) — fork ruled **O3 (cut-over)**; executed: opaque `WorkspaceKind` (tag `0x0B`), retired band `0x00–0x0A` ⇒ typed `UnsupportedWireVersion`, dev DBs wiped via `cutover-wipe.sql` (env enumeration first — all dev), noun gate in CI with no allowlist, public-api diff recorded. Receipt: `ca-reuse-001-BL-T2-receipt.md`. (Fork history: `ca-reuse-001-BL-T2-fork.md`.)
- **Follow-ups** logged in `ca-reuse-001-followups.md` (demo bindings on prod path; embedded ob-poc manifests; decimal→f64 aliasing; `bind_placeholder_from_payload`; inbox re-stamp).
