# Phase 0 — execution lease and dispatch: behavioural map

**Companion docs:** `wasm_bpmn_execution_lease_forensic_review.md`, `zed_agent_execution_lease_remediation_plan.md`
**Baseline:** commit `6e4de6d`, branch `feat/lease-remediation-phase0`
**Scope:** trace every producer of a process transition command, every claim/lease table, every claim/renew/release/consume/reclaim path, its transaction boundary, and whether it has a durable idempotency identity. No production behaviour changed in this phase.

## Producers of a process transition command

| Producer | Entry point | Command | Transaction boundary |
|---|---|---|---|
| Scheduler tick | `BpmnLiteEngine::tick_claimed_batch` → `tick_instance` (`engine.rs:858,999`) | `Command::Tick` | claim (own tx) → decode/execute (in memory) → `commit_transition` (own tx) → `release_instance_transition` (own tx). **Three separate transactions**, not one. |
| Timer firing | `tick_due_timers` → `apply_timer_command` (`engine.rs:872,906`) | `Command::Timer` | Timer claimed via `claim_due_timers` (own tx); commit via `commit_transition`; timer released via `release_timer_claim`. Three transactions. |
| Buffered/direct message | `run_instance` message correlation (`engine.rs:1138`) | `Command::Message` | Message claimed via `claim_buffered_message`; commit separate; no unified transaction. |
| External job completion | `complete_job` / `complete_job_with_claim` (`engine.rs:1173,1248`) | `Command::JobResult` | **F-02**: `validate_job_claim` (own tx, read-only) then `complete_job` → `apply_and_commit_command` → `commit_transition` (own tx, unconditional `DELETE ... WHERE job_key=$2`). Two transactions with an unbounded gap; the second has no ownership predicate. |
| External job failure | `fail_job` / `fail_job_with_claim` (`engine.rs:1291,1314`) | `Command::JobFailure` | Same two-transaction shape as completion; not audited in this pass — recommend Phase 1 apply the same claim-token fix here, not just to the success path. |
| Effect dispatch/response | `dispatch_pending_effects` (`engine.rs:348`) | `Command::EffectResult` | Effect claimed via `claim_pending_effects`; released via `release_effect_claim`. Already token-identified (`ClaimedEffect`) — the pattern F-04/F-08 want generalised to transition leases. |
| Signal | `signal` / `signal_with_value` (`engine.rs:1365,1389`) | `Command::Signal` | Goes through the same `claim_instance_for_transition` → commit → release triple as tick. |
| Cancellation | `cancel` (`engine.rs:1416`) | `Command::Cancel` | Same triple. Job-side cancellation (`JobMutation::Cancel`, `store_postgres.rs:1743-1749`) already has a documented pending-or-claimed-or-gone absence-is-legal rule — Phase 1's claim-bound ack should reuse this reasoning, not re-derive it. |
| Startup recovery | `recover_all_tenants` (`engine.rs:1625`) | `Command::Tick` (administrative quarantine on failure) | Per-instance `claim_work_for_transition`; **F-03**: a busy claim returns `None`, `.ok_or_else` turns that into a hard `Err` that aborts the entire tenant loop, not just that instance. |

**Observation not called out explicitly in the review:** every one of these paths is claim → (execute) → commit → release as **independent transactions**, not one enclosing transaction. The fenced-commit CAS is what keeps this safe against stale overwrites (I-1/I-2 hold), but it means the claim and the release are two separate opportunities for the ABA/TOCTOU windows in F-02/F-04 to open. Any Phase 2/3 redesign should keep asking, per path, "what closes between claim and release, and can this table's owner-string change underneath it."

## Lease/claim tables and their clock/deadline fields

| Table | Owner/token fields | Deadline field | Fence/generation | Notes |
|---|---|---|---|---|
| `workflow_instances` | `lease_owner` (text, reused per-engine-instance — `engine.rs:207`) | `lease_until` | `fence` (bigint, monotonic) | **No token column.** `Claim` (`bpmn-lite-types/src/transition.rs:55-60`) carries `tenant_id, instance_id, expected_revision, fence` — no owner, no token, no deadline. This is the root cause of F-04/F-08. |
| `job_queue` | `worker_id`, `claim_token` (added migration `019_worker_claim_ownership.sql`) | `claim_expires_at` | `attempt_count`, `retries_remaining` (no fence) | Has the token shape F-02 needs; the gap is that `commit_transition`'s `jobs_ack` delete (`store_postgres.rs:1735-1741`) doesn't use it. |
| timers | `ClaimedTimer`/`ClaimedTimerIdentity` (`bpmn-lite-types/src/transition.rs:796,808`) | persisted deadline | per-claim identity | Correct pattern — cited by the review as the template. |
| effects | `ClaimedEffect` (`bpmn-lite-types/src/transition.rs:493`) | persisted deadline | per-claim identity | Correct pattern, same as timers. |
| buffered messages | claim fields on `message_buffer` | `expires_at` | none observed | Not deep-audited this pass; same family as jobs — worth the same Phase-1-style check before declaring Phase 2 done. |

## Idempotency identity by command type

- **Tick / Signal / Cancel:** none beyond the fence — correct, since these aren't externally retried; the fence alone provides I-1.
- **Job completion/failure:** `job_key` is the only identity threaded into the commit; no worker/token/claim-generation reaches the SQL delete. **No durable completion receipt exists** — I-9 (ambiguous success is recoverable) is unimplemented for jobs today. Phase 1 must add this, not just the ownership check.
- **Timers/effects:** claim-token identity exists; response idempotency wasn't traced in this pass (out of Phase 0 budget — flag for Phase 1 review before Phase 2 sign-off).
- **Recovery:** administrative quarantine transitions use `EffectId::for_transition(instance_id, revision+1, u32::MAX)` — a derived, not caller-supplied, ID. Fine for its purpose (system-initiated, not a retried client command).

## Phase 0 regression tests added

All four land in `bpmn-lite-store-postgres/src/store_postgres.rs`'s existing `mod tests` (the crate's established convention — every postgres-backed test already lives in this one file). Three run unmarked against the live `bpmn_lite_test` database and are **expected to fail today** (red), which is the point: they pin the defect so Phase 1/2 fixes have a receipt. One (idle population / F-01) is a direct, deterministic reproduction and also runs unmarked.

1. `test_phase0_f04_same_owner_aba_release_clears_new_claim` — same-owner ABA release (F-04). **Expected: FAILS today.** Fixed by Phase 2 tokenised release.
2. `test_phase0_f02_unconditional_job_ack_deletes_reassigned_claim` — job completion ownership race (F-02), exercised against the exact `DELETE FROM job_queue WHERE tenant_id = $1 AND job_key = $2` primitive `commit_transition` uses. **Expected: FAILS today** (i.e. asserts the row survives; it doesn't). See scoping note below. Fixed by Phase 1.
3. `test_phase0_f03_active_active_recovery_aborts_on_busy_lease` — active-active recovery (F-03), via `recover_all_tenants` against an instance already leased by a live peer. **Expected: FAILS today** (recovery returns `Err` instead of skipping the busy instance). Fixed by Phase 4.
4. `test_phase0_f01_idle_population_claims_regardless_of_readiness` — idle population write amplification (F-01), via `claim_running_instances` directly. **Expected: PASSES today**, asserting the current (undesirable) behaviour — that idle `Running` rows are claimed regardless of fibre readiness — since there is no readiness predicate to fail against yet. This one flips from "asserts the bug" to "asserts the fix" once Phase 3's durable activation queue lands; a comment marks it for that swap.

### Scoping note on test 2 (F-02)

`BpmnLiteEngine::complete_job_with_claim` requires a real compiled artifact behind `bytecode_version` to reach `commit_transition` through the full command-apply path (`apply_and_commit_command` decodes and executes against the loaded artifact). Building that fixture is out of Phase 0's budget. The test instead exercises the vulnerable SQL primitive directly — the same `DELETE ... WHERE tenant_id = $1 AND job_key = $2` statement `commit_transition` runs unconditionally on `jobs_ack` — seeded through the real `dequeue_jobs`/`reclaim_stale_jobs` store calls so the claim-token reassignment itself is genuine, not simulated. This proves the primitive has no ownership predicate, which is the actual defect; it does not additionally prove `complete_job_with_claim`'s full call path reaches that primitive unchanged (that much is a straight code read, already confirmed against `engine.rs:1248-1285` and `store_postgres.rs:1735-1741` in the review). Recommend Phase 1's own test suite (already specified in the plan) add the full-artifact round trip once the claim-bound ack API exists to test.

## Gate 0

- Existing focused lease tests (`test_risk_009_lease_fence_rejection`, `test_regression_healed_by_claim`, `test_park_releases_lease`, `test_concurrent_claim_and_recovery`) still pass unmodified.
- Four new tests added and run against `bpmn_lite_test`; three are expected-red defect captures, one is an expected-green baseline capture that will flip meaning at Phase 3.
- No production code changed in this phase.
