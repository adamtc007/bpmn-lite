# Zed agent remediation plan: BPMN execution leases and durable dispatch

**Target repository:** `/Users/adamtc007/dev/bpmn-lite`  
**Baseline reviewed:** commit `6e4de6d`  
**Companion review:** `wasm_bpmn_execution_lease_forensic_review.md`  
**Objective:** make process execution safe and operationally scalable under multiple native or Wasm executors, process crashes, lease expiry, retries and ambiguous responses.

## Instructions to the Zed agent

Execute this plan in phases. Do not combine all phases into one large rewrite. At the start of every phase:

1. Read this plan and the forensic review completely.
2. Inspect the current repository state and determine whether the baseline has moved.
3. Preserve all pre-existing user changes. The reviewed worktree was already dirty in designer and utterance-related files; do not modify, format, stage or revert unrelated files.
4. Locate repository instructions such as `AGENTS.md` and follow them.
5. State the phase being executed, the files expected to change and its acceptance tests.
6. Add a regression test that demonstrates the defect before or alongside its fix.
7. Keep schema changes forward-only. Do not edit an already-applied migration; create the next numbered migration.
8. Run focused tests first, then workspace-level checks appropriate to the change.
9. Stop at the phase gate and report results before beginning the next phase.

Do not weaken revision/fence checks to make a test pass. Do not introduce an in-memory queue as the source of truth. PostgreSQL must remain able to reconstruct all ready and in-flight work after every executor is killed.

## Non-negotiable invariants

The implementation must preserve these properties:

**I-1 — One durable process history.** A transition commits only against its expected instance revision and fence. Obsolete executors cannot overwrite a newer transition.

**I-2 — Expiry is eligibility, not revocation.** Merely passing a deadline does not invalidate an executor. A successful newer acquisition increments the fence and performs revocation.

**I-3 — Acquisition identity is unique.** Every lease acquisition has a unique token. Owner/pod/worker names are diagnostic identity, never sufficient authority.

**I-4 — Release and renewal are conditional.** They match tenant, instance or work item, acquisition token and fence/generation. A stale release is a harmless no-op.

**I-5 — External-work ownership is atomic with transition commit.** A job completion, timer delivery, message delivery or effect response cannot change process state unless the same database transaction proves that the command is still valid and owned.

**I-6 — Ready work is durable.** If all compute processes disappear, PostgreSQL still contains every activation that must eventually execute.

**I-7 — Idle workflows are idle.** A process with no ready activation causes no periodic process-row update, snapshot decode or Wasm invocation.

**I-8 — Per-instance serialisation.** At most one activation for an instance can hold transition authority at a time. Other commands may remain durably queued.

**I-9 — Ambiguous success is recoverable.** If commit succeeds but the response is lost, retry or receipt lookup returns the already-committed result without applying the business command twice.

**I-10 — Graceful shutdown stops acquisition first.** A server stops claiming new work, drains bounded in-flight transitions, then exits. Forced exit remains recoverable by expiry.

## Phase 0 — Baseline, behavioural map and failing regression tests

### Work

Create a short implementation note under the repository's normal design-document location. Record:

- every producer of a process transition command;
- every lease/claim table and its clock/deadline fields;
- every path that claims, renews, releases, consumes or reclaims it;
- the transaction boundary for each path;
- whether the command has a durable idempotency identity.

At minimum trace:

- scheduler tick and runnable fibre continuation;
- timer firing;
- buffered message consumption and direct message correlation;
- external job completion and failure;
- effect dispatch and effect response application;
- cancellation and administrative transitions;
- startup recovery and graceful shutdown.

Add regression tests for the four known failures. Tests may initially be marked ignored only if they require a later schema/API phase, but the same-owner stale-release test should be made runnable immediately.

### Required tests

1. **Same-owner ABA release:** acquire as owner X; expire; reacquire as X with a newer fence/token; release using the old claim; assert the new claim remains authoritative.
2. **Job completion ownership race:** worker A validates; its claim expires and is assigned to B; A attempts completion; assert no process transition and no deletion of B's job.
3. **Active-active recovery:** replica A holds a legitimate transition lease while replica B runs startup recovery; B must not fail startup or quarantine the instance.
4. **Idle population:** create many `ProcessState::Running` instances whose fibres all wait; one scheduler cycle must claim/update zero process instances.

### Gate 0

- Current focused lease tests still pass.
- New tests accurately capture current defects; expected failing tests are documented.
- No production behaviour is changed in this phase.

## Phase 1 — Make successful job completion atomically claim-bound

This closes the highest-risk correctness hole before the wider dispatcher redesign.

### API and model changes

Replace a bare successful `jobs_ack(job_key)` mutation with a claim-bound mutation containing:

- tenant ID from the enclosing claim;
- job key;
- worker ID;
- unique job claim token;
- optionally a claim generation if one is introduced;
- a stable completion command/request ID for receipt lookup.

The public completion path must pass these values into the kernel/transition model. Do not perform a separate `validate_job_claim` as the authority decision. A preliminary read may improve error messages, but it must not authorize commit.

### PostgreSQL commit rule

Within the same transaction that updates the process revision:

1. conditionally consume the job row where `status = 'claimed'`, worker and claim token match, and `claim_expires_at > now()`;
2. require exactly one row;
3. if zero rows match, roll back the entire process transition and return a typed `LostClaim`/`StaleWork` result;
4. write a durable completion receipt keyed by tenant plus completion command ID or another existing dedupe key;
5. on an exact retry, return the receipt as idempotent success without reapplying the transition.

Do not delete or acknowledge a job by job key alone. Review cancellation separately: cancellation may intentionally target pending or claimed work, but it needs its own documented authority rule and must not masquerade as worker acknowledgement.

### Retry-budget correction

Remove `retries_remaining > 1` from successful completion validation. A currently claimed attempt may succeed regardless of how many future retries remain. Confirm and document whether the counter means total attempts remaining or future retries; make dequeue, failure and stale-reclaim transitions consistent with that definition.

### Reclaim correction

Change stale-job selection to use the persisted `claim_expires_at <= now()`, not `claimed_at` plus a separately supplied timeout. Validate lease duration when the claim is issued. Remove or deprecate the reclaim timeout argument if it no longer represents authority.

### Tests

- stale A cannot complete after reassignment to B;
- stale A cannot delete B's job row;
- the process revision is unchanged when atomic acknowledgement fails;
- current owner can succeed on the final permitted attempt;
- exact completion retry returns idempotent success;
- different payload under the same completion ID is rejected as an idempotency conflict;
- reclaim occurs at the persisted deadline for short and long leases;
- failure/retry and dead-letter paths still require exact claim ownership.

### Gate 1

- No successful job path uses a check-then-act ownership decision.
- Process transition and job acknowledgement succeed or roll back together.
- Focused engine, store and PostgreSQL tests pass.

## Phase 2 — Tokenise transition leases

### Schema

Add transition acquisition identity to the mutable instance coordination fields. Use names consistent with the repository, conceptually:

- `transition_lease_token UUID NULL`;
- existing owner and expiry fields;
- existing monotonic `fence`.

Use PostgreSQL or the runtime's cryptographically strong UUID generator. UUIDv7 may be used if it is already the repository standard, but ordering is not required for the token. Token uniqueness—not timestamp order—is the authority property.

### Claim type

Extend `Claim` to contain:

- owner;
- lease token;
- lease deadline as returned by PostgreSQL;
- expected revision;
- fence;
- existing tenant and instance identity.

Do not trust a worker-calculated deadline. Return the database deadline.

### Acquisition semantics

- A new claim is permitted only when no lease exists or the recorded deadline has passed.
- A new acquisition always writes a fresh token and increments the fence.
- The fact that the requested owner string equals the existing owner does not make it the same acquisition.
- An active acquisition is returned as typed `Busy`, not `None` or “leased or missing.” Do not expose another worker's secret token.
- If local code needs to continue holding a claim, it uses renewal, not reacquisition.

### Release

Change release to accept `&Claim` or an explicit claim identity. SQL must match tenant, instance, lease token and fence. Owner may also be checked but is insufficient alone. Return whether a row was affected so stale release can be observed without being treated as an infrastructure failure.

Remove all owner-only release call sites, including error cleanup paths.

### Renewal

Add token/fence-checked renewal:

- renewal matches the current acquisition and returns the database deadline;
- renewal never increments the fence;
- renewal after takeover affects zero rows and returns `LostClaim`;
- enforce a maximum permitted continuous execution/renewal policy rather than allowing unbounded heartbeat.

The normal deterministic kernel step should remain short and bounded. Renew only around unavoidable bounded delays such as cold artifact loading or explicitly permitted host work. Prefer loading artifacts before claiming when safe.

### In-process scheduling

- Reserve an execution permit before acquiring database work.
- Do not claim a batch larger than immediately available permits.
- Add a per-instance single-flight guard if multiple local API/scheduler paths can target the same instance concurrently. The database remains final authority.

### Tests

- the Phase 0 same-owner ABA test passes;
- wrong token cannot release or renew;
- old token cannot release after same-owner reacquisition;
- new owner takeover increments fence;
- expiry without takeover does not by itself make an otherwise valid commit stale;
- takeover causes old commit to fail `StaleFence`;
- active same-owner claim returns `Busy` rather than creating duplicate holders;
- queued batch items do not acquire leases until an execution permit exists;
- renewal/takeover race has exactly one authoritative outcome.

### Gate 2

- There are no transition release/renew SQL statements matching owner alone.
- Every acquisition is individually identifiable in logs and metrics by a safe shortened token/fence representation.
- Existing timer/effect token semantics remain intact.

## Phase 3 — Introduce the durable activation queue

This is the main scale remediation. Implement it in compatibility stages rather than deleting the old scheduler immediately.

### 3A — Schema and store primitives

Create a durable activation table. Final names may follow repository conventions, but it needs:

- activation ID and stable command ID;
- tenant and process instance ID;
- command kind and versioned serialized command/envelope;
- target/base revision where applicable;
- status: ready, claimed, completed, cancelled or dead-lettered;
- `available_at`, priority and deterministic ordering fields;
- claim owner, unique claim token, claim deadline and attempt count;
- created/completed timestamps and last failure classification;
- payload/canonical hash sufficient to detect conflicting reuse of an ID.

Indexes must support ready selection by tenant/partition, status and `available_at`. Add uniqueness for command/dedupe identity. Multiple commands may wait for one instance, but only one may hold instance transition authority.

Store operations should include:

- enqueue idempotently;
- claim a bounded ready set using `FOR UPDATE SKIP LOCKED`;
- atomically acquire the associated instance transition token/fence;
- renew exact claim;
- release exact claim to ready with retry scheduling;
- atomically consume activation during transition commit;
- dead-letter poison work with durable diagnostics;
- recover expired claims using their persisted deadlines.

The activation claim and instance semaphore must not allow two claimed activations for one instance. Achieve this with a transactionally locked/conditionally updated instance coordination row, not an in-memory assumption.

### 3B — Dual-write/shadow observability

Inventory every condition that makes work ready. In the same transaction that creates the condition, enqueue the activation:

- new instance/start continuation;
- a transition leaving a fibre runnable;
- due timer delivery;
- correlated message delivery;
- successful external job result/failure command;
- persisted effect response;
- administrative cancellation/resume/retry.

Initially keep old dispatch available behind a feature flag while recording activation production and consumption metrics. Add a reconciliation query that detects:

- runnable state with no ready/claimed activation;
- ready activation whose instance is terminal/quarantined;
- multiple authoritative claims for an instance;
- activation base revision inconsistent with the current head;
- expired claimed activations not reclaimable.

Do not let the old scheduler and new dispatcher both execute the same semantic command without shared command-level idempotency.

### 3C — Switch execution

Change the scheduler to claim activations, not `ProcessState::Running` rows. Execution input should be a claimed activation plus the atomically loaded snapshot and transition claim.

On commit:

1. prove activation token and process fence/revision;
2. apply process/current-head mutation;
3. append journal, snapshot/receipt and outbox records;
4. consume the activation;
5. enqueue any immediate continuation activation;
6. commit all steps together.

If the kernel determines the activation is semantically stale, record an idempotent/stale receipt and consume it without changing business state where appropriate. Do not endlessly retry stale commands.

### 3D — Remove population polling

Remove the periodic call to `claim_running_instances` from the server scheduler. Deprecate and then remove the method after all producers have moved. A diagnostic reconciliation sweep may read process heads slowly, but it must not be the normal dispatch mechanism and must not update idle rows.

### Tests and scale gate

- immediate continuation creates exactly one next activation;
- waiting timer/message/job/effect fibre creates no runnable tick activation;
- external event atomically creates its activation;
- two queued commands for one instance execute serially;
- duplicate enqueue by command ID is idempotent; conflicting content is rejected;
- executor death after activation claim leads to reclaim and one committed transition;
- commit plus activation consumption is atomic under injected failure;
- stale activation is consumed/recorded according to policy, not retried forever;
- tenant/priority fairness under a hot process;
- at least one million waiting process heads can remain idle through repeated scheduler intervals with zero process-head writes and zero snapshot decodes caused by generic ticking;
- dispatch throughput scales with ready activation count and executor permits.

### Gate 3

- Normal scheduler queries only durable ready/reclaimable work.
- Killing all executors and starting new ones resumes every activation from PostgreSQL.
- Idle population size does not materially affect scheduler write rate.

## Phase 4 — Active-active recovery, shutdown and operations

### Startup recovery

Split fail-closed validation from opportunistic reconciliation:

- startup may fail for schema incompatibility, unavailable durable storage or global integrity prerequisites;
- a legitimately busy process is not startup failure;
- joining replicas become ready without claiming every running process;
- broad reconciliation runs incrementally in the background, optionally under a separately elected leader lease;
- corrupt individual instances are quarantined individually without preventing unrelated tenant execution.

Recovery should primarily enumerate ready/expired activations, timers, effects and jobs, not all process heads.

### Graceful shutdown

Put scheduler/reclaimer/pruner tasks under structured cancellation:

1. readiness becomes false;
2. cancel acquisition loops;
3. stop accepting new external mutation requests or return retryable draining status;
4. drain in-flight commits for a configured bounded period;
5. release exact tokenised claims when possible;
6. exit, allowing deadlines to recover anything forcibly interrupted.

### Metrics and diagnostics

Separate counters and latency histograms for:

- activations enqueued, claimed, committed, stale, retried and dead-lettered;
- claim busy, renewal success/lost claim and stale release;
- queue age and claim-to-start delay;
- kernel duration, artifact-load duration and transaction duration;
- fence conflicts versus revision conflicts;
- claimed work that produced no business transition;
- expired claims by work type;
- recovery lag;
- idle process population versus ready activation count.

Do not label number claimed as number successfully ticked.

### Gate 4

- Two or more replicas can start, run and restart while work is active.
- Graceful shutdown produces no new claims after drain begins.
- Forced termination recovers within the documented lease/reclaim bound.
- Dashboards distinguish contention, stale work, business no-op and infrastructure failure.

## Phase 5 — Fault-injection and release qualification

Build a repeatable qualification suite. Inject failure at least at:

- immediately before and after activation claim;
- after snapshot load;
- during artifact load;
- before SQL transaction;
- after conditional process update but before journal/activation consumption;
- immediately before SQL commit;
- immediately after SQL commit but before response delivery;
- during renewal;
- during graceful shutdown;
- after job validation data is received but before atomic job completion;
- under PostgreSQL connection loss and transaction retry.

Run each scenario with one executor, multiple native executors and the intended Wasmtime pool. Wasm instance destruction must be indistinguishable from native worker death to the durable authority model.

### Release criteria

The remediation is complete only when:

- all invariants I-1 through I-10 have named automated tests;
- no stale acquisition can release, renew, acknowledge or consume newer work;
- no business transition can commit from a lost external-work claim;
- active-active startup and rolling restart pass repeatedly under load;
- an idle large process population generates no generic tick write amplification;
- ambiguous commit responses are idempotently recoverable;
- all PostgreSQL integration tests run against an actual test database in CI;
- `cargo fmt`, focused tests, relevant workspace tests, `cargo clippy` and repository CI checks pass;
- migration upgrade is tested from the last production schema and rollback strategy is documented at the deployment level.

## Recommended commit sequence

Keep commits reviewable and independently testable. A suitable sequence is:

1. `test: capture lease and job ownership races`
2. `fix: bind job completion to claim token atomically`
3. `fix: reclaim jobs by persisted claim expiry`
4. `feat: add tokenised transition lease identity`
5. `fix: make transition release and renewal claim-specific`
6. `fix: reserve executor capacity before claiming work`
7. `feat: add durable workflow activation storage`
8. `feat: enqueue activations at durable readiness boundaries`
9. `feat: execute claimed activations atomically`
10. `refactor: remove running-instance scheduler polling`
11. `fix: make recovery active-active tolerant`
12. `feat: structured scheduler shutdown and lease metrics`
13. `test: add crash, failover and idle-population qualification`

Do not commit unrelated existing worktree changes. If a phase uncovers a new authority or data-loss issue, stop, document a minimal reproducer and amend the plan before expanding implementation scope.

## First-session prompt for Zed

Use the following as the initial execution instruction:

> Work in `/Users/adamtc007/dev/bpmn-lite`. Read `/Users/adamtc007/dev/bpmn-lite/docs/todo/wasm_bpmn_execution_lease_forensic_review.md` and `/Users/adamtc007/dev/bpmn-lite/docs/todo/zed_agent_execution_lease_remediation_plan.md` completely. Execute **Phase 0 only**. Preserve all pre-existing working-tree changes and do not modify unrelated designer or utterance files. Map every durable command/claim transaction boundary, add the four specified regression tests, run focused checks, and stop at Gate 0 with a report of changed files, test results, unresolved questions and any necessary amendments to later phases. Do not implement production fixes until Gate 0 is reviewed.
