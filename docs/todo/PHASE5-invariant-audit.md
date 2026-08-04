# Phase 5 — I-1 through I-10 invariant audit

Status: **done**. Closes the first Phase 5 release criterion — *"all
invariants I-1 through I-10 have named automated tests"* — from
`docs/todo/zed_agent_execution_lease_remediation_plan.md`'s "Non-
negotiable invariants" section (lines 28-46). Every invariant maps to
at least one specific, verified-to-exist test; one partial gap is
called out explicitly rather than papered over.

Every test name/file/line below was independently confirmed to exist
via `grep` against the actual source, not taken on faith from the
research pass that first proposed the mapping.

| Inv. | Name | Test(s) | Why it proves the invariant |
|---|---|---|---|
| **I-1** | One durable process history | `test_fenced_transition_rejects_expired_owner` — `bpmn-lite-store-postgres/src/store_postgres.rs:8799` | A superseded owner's `commit_transition` at the old fence returns `CommitError::StaleFence`; the row is untouched. A new owner's commit at the correct fence advances the revision. Obsolete executors cannot overwrite a newer transition. |
| **I-2** | Expiry is eligibility, not revocation | `test_fenced_transition_rejects_expired_owner` (same test, line 8799) + `test_risk_009_lease_fence_rejection` — `store_postgres.rs:7891` | Passing a deadline alone does not revoke the current owner; only a *successful new acquisition* by a different owner bumps the fence and makes the old owner's subsequent write fail. |
| **I-3** | Acquisition identity is unique | `test_phase0_f04_same_owner_aba_release_clears_new_claim` — `store_postgres.rs:11822` | Same owner **string** reacquires after expiry; the fence advances and a **fresh token** is minted (`assert_ne!` on `lease_token()`) even though the owner string is identical to the expired acquisition — proves the owner name alone is never sufficient authority, only the token is. |
| **I-4** | Release and renewal are conditional | `test_transition_lease_excludes_other_owner_until_release` — `bpmn-lite-store/src/store_memory.rs:3381` + `test_phase1_f02_complete_job_with_claim_rejects_stale_worker` — `store_postgres.rs` (Phase 1) | Memory-store test: releasing with a token that doesn't match the live acquisition is a harmless no-op — the lease stays held. Postgres test: a stale worker's claim-checked completion (mismatched `claim_token`) is rejected without touching the live claimant's row. |
| **I-5** | External-work ownership atomic with transition commit | `test_risk_003_emit_atomicity` — `store_postgres.rs:8229` + `test_transient_effect_failure_retains_effect_without_advancing_instance` — `store_postgres.rs:10797` | First: outbox/pending-invocation inserts and the instance save roll back together atomically if any one operation in the same transaction fails. Second: a transient effect retry leaves the instance revision and the effect-response table untouched — no partial state change escapes the transaction boundary. |
| **I-6** | Ready work is durable | `restart_mid_run_resumes_and_conserves_via_activation_queue` — `bpmn-lite-engine/fuzz/src/fault.rs` (Phase 5) | Engine A enqueues via the durable activation queue, then is dropped entirely — only the store survives. A fresh Engine B resumes and drives the instance to completion via `claim_ready_activations`, proving ready work survives total compute-process loss, not just an in-memory engine restart. |
| **I-7** | Idle workflows are idle | `test_phase3c_f01_idle_population_produces_zero_activation_claims` — `store_postgres.rs` (Phase 4), contrasted directly against the pre-fix baseline `test_phase0_f01_idle_population_claims_regardless_of_readiness` in the same file | Ten `Running` instances with no runnable fiber (no Phase 3B dual-write) produce **zero** claims from `claim_ready_activations` — the baseline test right above it in the same file shows the *old* scheduler claiming and lease-writing every one of those same ten idle rows, making the contrast load-bearing, not just a standalone assertion. |
| **I-8** | Per-instance serialisation | `test_phase3a_memory_claim_enforces_one_claimed_activation_per_instance` — `bpmn-lite-store/src/store_memory.rs` (assertion text literally cites "I-8") + `instance_guard_rejects_second_acquisition_until_released` — `bpmn-lite-engine/src/tests.rs:6030` | Store-level: three activations queued for one instance yield exactly one claimed row (the partial unique index / in-memory equivalent). Engine-level: the in-process single-flight guard (F-08) rejects a second concurrent acquisition for the same instance while allowing an unrelated instance through — together cover both the durable and in-process halves of "at most one activation holds transition authority at a time." |
| **I-9** | Ambiguous success is recoverable | `test_pg_atomic_complete_idempotency` — `store_postgres.rs:8453` | Re-delivering the same completion command after a successful first commit is a no-op: state stays at its first-commit value (not overwritten by the redelivered command's payload) and the event count doesn't grow — proves a retry after a lost response doesn't reapply the business command a second time. |
| **I-10** | Graceful shutdown stops acquisition first | `sleep_or_shutdown_returns_true_when_the_sleep_elapses_first`, `sleep_or_shutdown_returns_false_the_instant_shutdown_is_signalled`, `sleep_or_shutdown_returns_false_if_the_sender_is_dropped` — `bpmn-lite-server-runner/src/main.rs` `tests_owner` mod (Phase 4) | Covers the primitive every background/acquisition loop polls instead of a blind sleep: a shutdown signal (or an unexpectedly dropped sender) stops the wait immediately, proving new-work acquisition halts on signal rather than completing its current sleep first. |

## Gap, stated plainly

**I-10 is only partially covered.** The three `sleep_or_shutdown` tests
prove the *first* half of the invariant — acquisition loops stop taking
new work the instant shutdown is signalled. Nothing in the test suite
asserts the *second* half — that bounded in-flight transitions actually
drain before process exit (`main()`'s `tokio::time::timeout(drain_
bound, join_all([...]))` sequence, Phase 4's graceful-shutdown work).
That code path exists and is reasoned about in
`docs/todo/PHASE4-graceful-shutdown.md`, but it runs inside `main()`
itself — there is no test harness in this codebase that starts a real
server process, puts work in flight, sends it a shutdown signal, and
asserts what happened to that work before exit. Closing this fully
would need either extracting the drain sequence into an independently
testable function (the way `sleep_or_shutdown` already was), or a
process-level integration test — neither is a quick addition, and
that's the honest state, not a passing grade rounded up.

## Verification of the mapping itself

Every test name and file location in the table above was independently
confirmed via `grep -n "fn <test_name>"` against the actual current
source before being written into this document — not taken on faith
from whatever research pass first proposed the mapping. All ten rows
resolved to real, currently-existing tests on the first check.
