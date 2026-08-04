# Phase 2 (scoped) — tokenised transition release (F-04)

**Companion docs:** `PHASE0-execution-lease-mapping.md`, `PHASE1-claim-bound-job-completion.md`, `wasm_bpmn_execution_lease_forensic_review.md`, `zed_agent_execution_lease_remediation_plan.md`
**Baseline:** commit `6e4de6d`, branch `feat/lease-remediation-phase0`
**Scope:** close F-04's same-owner ABA release hole precisely. Deliberately narrower than the plan's full Phase 2 spec — see "What's deferred" below, with the reason it's a genuine fork rather than an oversight.

## The discovery that reshaped scope

The plan's Phase 2 description ("An active same-owner acquisition returns typed `Busy`... if local code needs to continue holding a claim, it uses renewal, not reacquisition") reads as if same-owner-while-live reacquisition should simply be removed. Tracing every caller first showed why that would break production: the scheduler's dispatch path is a **two-phase claim** — `claim_running_instances` bulk-reserves a batch of instance IDs under one owner (`lease_owner`/`lease_until`/`fence` on `workflow_instances`), then `tick_instance_inner` immediately calls `claim_work_for_transition` **again, as the same owner**, to get the real per-instance `Claim` it commits against. Removing same-owner-while-live reacquisition outright would make step 2 see step 1's own lease as "busy" and return `None` for every instance the scheduler had just reserved — breaking `tick_claimed_batch` entirely.

Building the alternative (thread a token from `claim_running_instances` through to `tick_instance_inner`, or implement full token/fence-checked renewal as its own primitive) is real, additional-capability work with **zero existing callers and zero failing tests** driving it — nothing today calls a renewal API, because nothing needs to outlive its lease yet (that's Phase 5's fault-injection concern, not a current defect). Building it now would be scope creep against "don't design for hypothetical future requirements."

**Decision:** keep the same-owner-while-live claim path exactly as it is (needed, safe on its own terms — it's a same-generation continuation, not a takeover). Fix only what F-04's red test actually proves is broken: **release must be keyed on the acquisition's identity, not the owner string**, so a stale release can't clear a live lease it doesn't actually hold.

## What changed

- **Migration `061_transition_lease_token.sql`** — adds `lease_token TEXT` to `workflow_instances`.
- **`bpmn_lite_types::Claim`** gained a `lease_token: String` field/accessor. `Claim::new` takes it as a 5th argument — every construction site updated (8 total; 6 are synthetic genesis claims for one-shot commits that are never released, given `""`).
- **`claim_work_for_transition`'s SQL** (`store_postgres.rs`): the token is now part of the same `CASE` shape already governing the fence — reused unchanged on a same-owner-still-live reacquisition (same generation), minted fresh (`md5(random()::text || clock_timestamp()::text)`, matching the job-queue token convention) whenever the fence actually advances (a genuinely new acquisition, whether first-ever or post-expiry takeover). Returned via the same `RETURNING` clause.
- **`release_instance_transition`** — trait signature, postgres impl, `RuntimeStore` wrapper, memory-store impl, and the fuzz crate's fault-injection wrapper all changed from `owner: &str` to `lease_token: &str`. The SQL match moved from `lease_owner = $3` to `lease_token = $3`, and release now also clears `lease_token`.
- **`bpmn-lite-store/src/store_memory.rs`** — the in-memory store's `transition_leases` map gained the same token dimension (`(owner, expires_at, fence, lease_token)`), with matching same-generation-reuse / new-generation-fresh-token logic, for dev/test parity with postgres (Phase 1's established convention).
- **Every production call site** that claims then releases (`engine.rs` ×6: `apply_and_commit_command`, `apply_timer_command`, `tick_instance_inner`, `complete_job_inner`, `recover_all_tenants` ×2; `bus_runtime.rs` ×4; `rest.rs` ×1 in both `bpmn-lite-server-runner` and via the `Claim` it already held) now captures `claim.lease_token()` — either before `work.into_parts()` consumes the claim, or directly from an already-in-scope `Claim` — and passes it to release instead of the owner string.

## Why this needed careful auditing, not just a compiler pass

`release_instance_transition`'s parameter type didn't change (`&str` either way), so **the compiler cannot catch a call site that still passes an owner string where a token is now expected** — that mismatch is silent and semantically wrong, not a build error. Every one of the ~20 call sites (production and test) had to be found by grep and read individually, not discovered by `cargo build`. One test (`test_durable_timer_survives_transition_cut_points`) genuinely broke this way — it released with the literal string `"timer-b"` instead of the real claim's token, which is now correctly a no-op, leaving the lease held and the next claim in the test blocked. Fixed by using the held `Claim`'s token, as the test always should have.

Three more pre-existing tests were passing "by accident" after the token switch — release with a hardcoded owner string is now unconditionally a no-op, so cleanup silently stopped happening even though the assertions downstream didn't yet notice. All three fixed to capture and pass the real token instead of leaving a latent bug for later.

## Test receipts

`cargo test -p bpmn-lite-store-postgres -- --test-threads=1`: **85 passed, 1 failed** (was 84/0 at the Phase 1 baseline).

- `test_phase0_f04_same_owner_aba_release_clears_new_claim` — **now passes.** Rewritten to hold both generations' `Claim`s explicitly, assert the fresh token differs from the stale one on a genuine takeover, and release with the *stale* token — proving the exact ABA sequence the forensic review described no longer clears the live lease.
- The four pre-existing tests above (`test_durable_timer_survives_transition_cut_points` and three release-by-owner-string sites) — fixed to use real tokens; all pass.
- `test_phase0_f03_active_active_recovery_aborts_on_busy_lease` — **still fails**, unchanged and correctly untouched (Phase 4 territory).
- Everything else — unchanged, still passing.

Full-workspace receipts, no regressions:
- `cargo build --workspace --tests` — clean.
- `cargo check --lib` in `bpmn-lite-engine/fuzz` — clean.
- `cargo test -p bpmn-lite-engine -p bpmn-lite-store -p bpmn-lite-server-runner -p bpmn-lite-server-designer` — every suite green (76+3+1+1+1 / 38 / 2+7+4 / 29 passed, 0 failed).
- `cargo fmt --check` / `cargo clippy --tests --no-deps` — no new diagnostics in any line I touched; one formatting slip in my own edit (`store_memory.rs`) was caught by the check and fixed in place.

## What's deferred, and why it's a fork rather than a gap

The plan's full Phase 2 asks for: removing same-owner-while-live reacquisition entirely, a typed `Busy` result (vs `None`), token/fence-checked **renewal**, and in-process execution-permit reservation before claiming. None of these are implemented here. Reasons, per item:

- **Removing same-owner-while-live reacquisition** would break the scheduler's two-phase claim as traced above. Fixing that properly requires either threading a token through `claim_running_instances` or building renewal as a first-class primitive with a real caller — a design decision (which shape) that affects the scheduler's dispatch path, not a mechanical follow-on to this change. **Recommend:** decide this before Phase 3 (durable activation queue) is designed, since Phase 3 replaces `claim_running_instances` outright and may make the question moot rather than something to solve twice.
- **Typed `Busy`** — `claim_work_for_transition` still returns `None`/`ClaimError` where the plan wants a distinguishable `Busy { lease_until, retry_after }`. No caller currently branches on "busy vs missing" differently, so this is pure API surface with no behavior fix behind it yet (F-10's territory, rated Low in the review).
- **Renewal** — zero current callers need to outlive a lease; building it now is speculative. Should land when Phase 5's fault-injection work actually needs a bounded-execution-longer-than-lease scenario, or when Phase 3's activation queue design calls for it.
- **In-process execution-permit reservation** (F-05) — untouched; the scheduler still claims up to `batch_size` before checking `MAX_SCHEDULER_IN_FLIGHT` permits. Independent of F-04, no interaction with this change.

This scoping decision itself is the fork to review: I chose the minimal slice that closes the one concretely red, high-severity test (F-04) over building the fuller token-lifecycle machinery the plan describes, because the fuller version has a real design question (how the scheduler's two-phase claim should work post-fix) that deserves your call rather than mine.

## Addendum — F-08 fixed via a per-instance single-flight guard, not token-threading

Asked to close the fork rather than defer it. Re-derived first: the plan's literal ask (thread a token through `claim_running_instances` into `tick_instance_inner`, replacing implicit same-owner reuse with explicit renewal) turned out to be solving a problem this codebase mostly doesn't have — `for_tenant()` mints a **fresh `transition_owner` on every call** (`format!("engine-{}", runtime_context.new_id())`), and `bpmn-lite-server-runner/src/grpc.rs` calls `self.engine.for_tenant(tenant_id)` fresh on every single RPC. Two concurrent gRPC requests never share an owner, so they never hit the same-owner-still-live reacquisition branch at all — they hit the ordinary different-owner exclusion instead, which already works.

The real residual exposure is narrower and different: **two operations issued concurrently on the SAME `BpmnLiteEngine` *value*** (same `transition_owner`, e.g. a caller holding one engine handle and racing two of its own async calls against the same instance) can both pass `claim_work_for_transition`'s same-owner-still-live check and both compute against the same revision — wasted work, not corruption (the revision CAS still lets only one commit), but exactly what the plan's own suggested remedy names: *"Add a per-instance single-flight guard if multiple local API/scheduler paths can target the same instance concurrently. The database remains final authority."*

Implemented that instead of the heavier redesign:

- `BpmnLiteEngine` gained `in_flight: Arc<Mutex<HashSet<Uuid>>>`, shared (Arc-cloned, not reconstructed) across every `for_tenant(...)` view so it protects at the "one process, one root engine" granularity the concern actually operates at.
- `with_instance_guard(instance_id, body)` inserts before running `body`, removes via an RAII `Drop` guard afterward — not a manual post-`.await` removal, which would leak the entry forever if `body` is ever dropped mid-flight by caller-side cancellation (a `select!`, a gRPC client disconnect, an outer timeout) rather than running to completion.
- Wired at the true independent entry points: `tick_instance_as_owner` (covers `tick_instance`/`tick_claimed_batch`/`run_instance`), `apply_timer_command`, `complete_job_inner` (covers both `complete_job` and `complete_job_with_claim`), `signal_with_value` (covers `signal`), `cancel`, `fail_job`, `fail_job_with_claim`, `emit_job_claimed_events`, and `recover_all_tenants`'s per-instance loop body.
- **Deliberately NOT wired on `apply_and_commit_command`**, the shared internal primitive several of those call through — because it is *also* invoked nested, from within an already-guarded caller's own call chain: `tick_instance_inner`'s loop calls `dispatch_pending_effects` → `apply_pending_effect_responses` → `apply_and_commit_command` for the same instance it is still ticking, entirely within the outer guard's scope. Guarding the shared primitive itself rejected that legitimate same-chain re-entry as if it were a genuine race — caught by 3 real test failures (`a11_ffi_end_to_end.rs`) on the first attempt, not by inspection.

### A `std::sync::Mutex` async-Send gotcha, for the record

The first cut used `std::sync::Mutex` end-to-end with an explicit `drop(guard)` before the `.await` inside `with_instance_guard`. That failed to compile two crates downstream (`bpmn-lite-bus-handler`) with a `MutexGuard` `!Send` error, even though the guard was provably dropped before the only `.await` in the function. The generator-state analysis for `async fn` can still capture a `!Send` local that is lexically alive at any point reachable from a yield, regardless of an explicit early `drop()`. Fix: move the lock acquisition into a plain, non-async `fn` (`try_acquire_instance_guard`/`release_instance_guard`) called *before* entering the async body — the guard's entire lifetime is then invisible to the enclosing async state machine, sidestepping the issue entirely rather than switching to `tokio::sync::Mutex` (which would need its guard held across a real `.await` to matter, and it never is here).

### Test receipts

- `instance_guard_rejects_second_acquisition_until_released` (new, `bpmn-lite-engine/src/tests.rs`) — tests the guard primitive directly and deterministically (`try_acquire_instance_guard`/`release_instance_guard` made `pub(crate)` for this). A first cut tried to prove this by racing two real `tokio::spawn`ed `engine.cancel()` calls with a `tokio::sync::Barrier`; it was flaky by construction — the guarded critical section against an in-memory store has no real `.await` suspension inside it, so one task reliably completes (including release) before the other is ever polled, even under a multi-thread runtime. Testing the mechanism directly avoids that entirely.
- The 3 `a11_ffi_end_to_end.rs` tests that broke on the first (wrong-boundary) attempt now pass, confirming the nested-reentry case is handled correctly.
- Full workspace + fuzz crate + every previously-green suite still green, run 3× to check for new flakiness — stable every time (`bpmn-lite-engine`: 77/0/3/1/1/1/0 passed across all binaries, identical on each run).
- `fmt`/`clippy` clean on every line touched.

What's still deferred, unchanged from the original Phase 2 write-up: typed `Busy` (vs `None`/`ClaimError`), an explicit renewal primitive, and F-05's execution-permit reservation. None of them are needed to close F-08 with this approach.
