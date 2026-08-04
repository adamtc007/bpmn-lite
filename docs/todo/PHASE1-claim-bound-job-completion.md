# Phase 1 — claim-bound job completion (F-02, F-06, F-07)

**Companion docs:** `PHASE0-execution-lease-mapping.md`, `wasm_bpmn_execution_lease_forensic_review.md`, `zed_agent_execution_lease_remediation_plan.md`
**Baseline:** commit `6e4de6d`, branch `feat/lease-remediation-phase0`
**Scope:** make successful job completion atomically claim-bound (F-02); reclaim by persisted `claim_expires_at` (F-06); stop rejecting a job's last permitted attempt (F-07). No design changes beyond what Phase 1 of the remediation plan specifies.

## What changed, and why it's the minimal-blast-radius shape

The plan's Phase 1 spec assumed `complete_job_with_claim` was the only completion path. Investigation showed a second, wider one: bare `complete_job` (no worker identity at all) is called by ~60 test/fuzz sites *and* one live production entry point — `bpmn-lite-server-designer/src/rest.rs`'s demo-advance loop, which self-dequeues and self-completes jobs. That loop already holds a real `worker_id`/`claim_token` on the `JobActivation` it dequeued (via `run_instance`); it just wasn't threading them through.

Given that, changing `JobCompletion`'s required shape would have broken dozens of unrelated call sites for no safety gain — those callers never held a job_queue claim to check in the first place. Instead:

- `JobCompletion` gained two **optional** fields, `worker_id: Option<String>` / `claim_token: Option<String>` (`bpmn-lite-types/src/types.rs`). `None` for internal/trusted callers (bare `complete_job`); `Some` only when a real external claim exists.
- `JobMutation` gained a new variant, `AckClaimed { job_key, worker_id, claim_token }` (`bpmn-lite-types/src/transition.rs`), sitting alongside the pre-existing `RetryClaimed`/`DeadLetterClaimed` — the codebase already had this exact "claimed-only, exact-one-row, Conflict-on-mismatch" pattern established for retry/dead-letter; completion now uses the same shape instead of inventing a new one.
- The kernel (`bpmn-lite-kernel/src/lib.rs::apply_job_completion`, both the normal-completion and terminal-state-ignored branches) branches on whether `completion.worker_id`/`claim_token` are present: `Some` → push `JobMutation::AckClaimed`; `None` → push the legacy unconditional `jobs_ack` (unchanged).
- `bpmn-lite-store-postgres/src/store_postgres.rs`'s commit loop gained an `AckClaimed` arm: `DELETE FROM job_queue WHERE tenant_id = $1 AND job_key = $2 AND status = 'claimed' AND worker_id = $3 AND claim_token = $4 AND claim_expires_at > now()`, requiring exactly one affected row or returning `CommitError::Conflict` — which, since this runs inside the same transaction as the process-revision update and nothing commits until every mutation in the transition has been applied, rolls back the whole transition (I-5 held automatically, no new plumbing needed). `bpmn-lite-store/src/store_memory.rs` got the matching in-memory arm for parity.
- `bpmn-lite-engine/src/engine.rs`: `complete_job` and `complete_job_with_claim` now both delegate to a private `complete_job_inner(..., claim_identity: Option<(&str, &str)>)`. **Neither public signature changed** — zero of the ~60 existing call sites needed touching.
- **F-07:** `validate_job_claim`'s SQL dropped `AND retries_remaining > 1`. It is now correctly *advisory only* (the doc comment says so explicitly) — the real authority is the atomic `AckClaimed` commit, so a bad pre-check can no longer gate correctness, only produce a worse error message in the rare case it's wrong.
- **F-06:** `reclaim_stale_jobs_inner`'s predicate changed from `claimed_at < now() - <caller-supplied timeout>` to `claim_expires_at <= now()`. The now-authorityless `timeout_ms` parameter was removed from the trait (`bpmn-lite-store/src/store.rs`) and every call site (`bpmn-lite-engine/src/engine.rs` recovery, `bpmn-lite-server-runner/src/main.rs`'s background reclaim loop, the postgres `RuntimeStore` wrapper, the fuzz crate's fault-injection wrapper) rather than left as an unused/ignored argument — matching the plan's explicit sanction ("remove or deprecate... if it no longer represents authority") and the project's "no trap doors" rule against silently-ignored parameters. `bpmn-lite-store/src/store_memory.rs`'s in-memory implementation was *already* using `claim_expires_at` correctly and already ignored the timeout — confirming F-06 was postgres-only, as the forensic review said.

I-9 (ambiguous success is recoverable) was **not built new** — `complete_job_inner` already checks `dedupe_get(tenant, job_key)` before doing anything else and returns early `Ok(())` on a hit, backed by the existing `dedupe_cache` table and `DedupeWrite` mechanism. This check runs before the claim-bound ack, so an exact retry after a lost response returns idempotent success regardless of whether the job_queue row still exists. Confirmed by reading, not newly tested this phase — flagging as a real gap if it needs its own explicit regression test before Phase 1 is considered fully closed.

## What did *not* change (explicitly out of scope)

- `fail_job` / `fail_job_with_claim` were not touched. `RetryClaimed`/`DeadLetterClaimed` (the mutations they drive) were already claim-checked before this phase — the forensic review's F-02 was specifically about the *success* path, and the plan's Phase 1 title matches that scope. Worth a one-line follow-up check in a later phase to confirm nothing in the failure path shares F-02's shape.
- `complete_job`'s unconditional `jobs_ack` primitive is unchanged and remains reachable by ~60 test sites and the designer REST demo loop. It is not a live production vulnerability *for the external worker protocol* (which now goes through gRPC's `complete_job_with_claim` exclusively), but the designer demo loop could trivially be upgraded to pass its already-available `worker_id`/`claim_token` through — a small, safe follow-up, not done here to keep this phase's diff minimal and reviewable.
- F-01, F-03, F-04, F-05, F-08 through F-10 are untouched — Phases 2–5 territory per the plan's own sequencing.

## Test receipts

`cargo test -p bpmn-lite-store-postgres -- --test-threads=1` against live `bpmn_lite_test`: **84 passed, 0 failed** (was 81 passed / 3 failed at the Phase 0 baseline).

- `test_phase0_f02_unconditional_job_ack_deletes_reassigned_claim` — **now passes**, re-scoped: it documents the legacy unconditional primitive's intentional contract (internal/trusted callers with no claim to check) rather than a live defect. Doc comment updated in place to explain the re-scoping rather than silently flipping its meaning.
- `test_phase1_f02_complete_job_with_claim_rejects_stale_worker` (new) — end-to-end through the real API: compiles `SMOKE_BPMN`, starts an instance, dequeues as worker A, forces reassignment to worker B via the real `reclaim_stale_jobs`/`dequeue_jobs` paths, asserts A's stale `complete_job_with_claim` is rejected and B's row survives, then asserts B's legitimate completion succeeds and the instance reaches `Completed`. **Passes.**
- `test_phase1_f07_validate_job_claim_allows_final_attempt` (new) — seeds a job with `retries_remaining = 1`, dequeues it, asserts `validate_job_claim` now returns `true` on the final permitted attempt. **Passes.**
- `test_phase0_f03_...` and `test_phase0_f04_...` — **still fail**, unchanged, correctly untouched by this phase (Phase 4 and Phase 2 territory respectively).
- `test_phase0_f01_...` — still passes (baseline capture, unaffected).

Full-workspace receipts, no regressions:
- `cargo build --workspace` (excluding the fuzz crate, which is not a cargo workspace member) — clean.
- `cargo build --workspace --tests` — clean.
- `cargo check --lib` in `bpmn-lite-engine/fuzz` — clean (the fuzz crate's `FaultInjectingStore::reclaim_stale_jobs` wrapper needed its signature updated to match F-06's fix).
- `cargo test -p bpmn-lite-engine` — 76 + 3 + 1 + 1 + 1 passed, 0 failed (covers the ~60 bare-`complete_job`/`fail_job` call sites in `tests.rs` and the integration test files — all still pass unmodified, confirming the optional-field approach preserved every existing call site).
- `cargo test -p bpmn-lite-server-runner -p bpmn-lite-server-designer -p bpmn-lite-store` — 38 + 2 + 4 + 7 + 4 + 29 passed, 0 failed (covers the designer REST demo loop and the gRPC `complete_job_with_claim` server wiring).
- `cargo fmt --check` / `cargo clippy --tests --no-deps` on every touched crate — no new diagnostics in any code I added or changed; all flagged items are pre-existing drift elsewhere in the same files (spot-checked by line number against my edit locations).

## Gate 1

- No successful job path uses a check-then-act ownership decision for the external worker protocol: `complete_job_with_claim` → `complete_job_inner` → `AckClaimed`'s atomic, exact-one-row, same-transaction-as-the-process-revision commit is now the sole authority; `validate_job_claim` is advisory only (doc-commented as such).
- Process transition and job acknowledgement succeed or roll back together (proven by the new end-to-end test, not just by reading the SQL).
- Focused engine, store, and PostgreSQL tests pass — plus the wider workspace suite, which Phase 1's own plan didn't explicitly require but which was the only way to verify the "zero blast radius on existing callers" design choice actually held.

Stopping here per the plan's phase-gate discipline. Phase 2 (tokenised transition leases, F-04/F-05/F-08) is the natural next step but is a larger, more invasive change (new `Claim` fields, new release/renewal semantics) and should get its own review before implementation starts.
