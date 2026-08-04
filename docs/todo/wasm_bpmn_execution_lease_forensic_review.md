# BPMN execution lease and dispatch: forensic review

**Repository reviewed:** `/Users/adamtc007/dev/bpmn-lite`  
**Revision:** `6e4de6d`  
**Review date:** 4 August 2026  
**Scope:** transition leases, scheduler dispatch, recovery, commit fencing, job ownership, expiry and crash behaviour. This was a read-only source review; unrelated working-tree changes were left untouched.

## Executive verdict

The durable transition core is substantially better than a conventional in-memory worker design. A transition claim is obtained with the snapshot in one database transaction; PostgreSQL supplies the lease clock; commits compare both revision and a monotonic fence; and the state change, journal, snapshot envelope, timers, jobs, effects and outbox work are committed atomically. A dead or obsolete executor therefore cannot silently overwrite a newer revision.

That is a strong safety foundation. It is not yet an enterprise-safe scale-out dispatch system.

Three issues are release blockers for active-active, high-volume operation:

1. The scheduler polls and writes **every process whose overall state is `Running`**, including processes whose fibres are all waiting. Its cost therefore grows with total live process population rather than ready work.
2. Successful job completion validates the worker claim in one transaction and later deletes/commits in another. A stale worker can cross that gap and complete a job after ownership has moved.
3. Startup recovery insists on leasing every running instance. A new replica can fail its recovery gate simply because a healthy replica currently owns one of those leases.

There is also a concrete stale-release race because transition leases are released by owner name alone rather than by a unique acquisition token and fence. This generally causes duplicate work and fence churn rather than corrupt state, but under load it can become a liveness failure.

**Bottom line:** the snapshot/fibre execution model is compatible with disposable Wasm compute. Wasm is not the risky part. The missing enterprise layer is a durable ready-work/authority state machine with tokenised claims. Fixing that layer does not require abandoning the stack machine or immutable journal.

## What is already sound

### Fenced commit protects process state

`Claim` carries the expected revision and fence (`bpmn-lite-types/src/transition.rs:55`). The PostgreSQL commit updates the current aggregate only where both still match (`bpmn-lite-store-postgres/src/store_postgres.rs:1488-1535`). A later claimant increments the fence, so an old executor receives `StaleFence`; concurrent work on the same revision receives `Conflict`.

Notably, commit does not require the lease time still to be in the future. This is correct. Expiry makes the work eligible for takeover; expiry alone should not invalidate a result. Actual takeover increments the fence and performs the revocation.

### Claim and snapshot read are atomic in PostgreSQL

The production `claim_work_for_transition` updates the lease/fence and returns the persisted snapshot within one database transaction (`store_postgres.rs:1217 onward`). It also validates the snapshot frame, canonical representation, artifact binding and journal relationship before returning work. The executor is not stitching an aggregate together from independently timed reads.

### Transition persistence is atomic

The current-head update and the transition's journal/snapshot/effect/job/timer/outbox mutations share a SQL transaction. A crash before commit publishes none of them; a crash after commit leaves a complete durable transition. A crash after commit but before lease release merely delays another claim until lease expiry.

### Database time is the lease clock

Lease eligibility and expiry use PostgreSQL `now()`. Worker clock skew therefore does not split lease authority.

### Timers and effects demonstrate the right token pattern

`ClaimedTimer` and `ClaimedEffect` contain unique claim identities, and their release/consume/response operations match those identities. Transition leases should adopt this same pattern.

## Findings

### F-01 — Critical: dispatch scans the live population, not ready work

`claim_running_instances` selects rows whose process state JSON is `"Running"`, orders them by `updated_at`, and updates the lease, tick time and sometimes fence (`store_postgres.rs:1145-1197`). It does not test whether a fibre is runnable.

Only after claiming and decoding the complete snapshot does `tick_instance_inner` inspect fibres and return if none has `WaitState::Running` (`bpmn-lite-engine/src/engine.rs:1009-1052`). A normal long-lived BPMN process can remain in process state `Running` while all fibres wait for a timer, message, job or effect.

Consequences:

- Scheduler work is O(all live instances), not O(ready activations).
- Each pointless pass writes the instance to acquire the lease and normally writes it again to release it.
- PostgreSQL MVCC creates dead tuples; the `updated_at` trigger also changes ordering on each update. At large scale this produces WAL, index churn and vacuum pressure even when business work is idle.
- A pool of Wasm instances cannot cure this: the bottleneck and amplification occur before Wasm execution.

At one million waiting processes and a 500 ms scheduler interval, the desired steady-state activity is approximately zero process ticks. The current model continually attempts to rotate through that million-row population.

**Required change:** introduce a durable ready-activation table/queue. Scheduler claims must select only durable activation records whose `available_at <= now()` and status is ready/reclaimable. Timers, messages, job completions and effect responses should create an activation. A transition that leaves another fibre runnable should create the next activation in the same commit.

### F-02 — Critical: successful job completion has a claim-validation TOCTOU race

`complete_job_with_claim` first calls `validate_job_claim` and then calls `complete_job` (`engine.rs:1248-1282`). These are separate transactions with arbitrary time between them.

The completion transition records only `jobs_ack: job_key`. Commit then performs an unconditional delete by tenant and job key (`store_postgres.rs:1735-1741`). Worker ID, claim token and expiry are absent from the atomic acknowledgement.

Failure sequence:

1. Worker A's claim is valid when checked.
2. A pauses or loses the network.
3. The claim expires; the job is reclaimed and assigned to B.
4. A resumes, acquires the process transition lease, commits its payload, and deletes B's job row.

The process fence does not prevent this because it protects instance transitions, not ownership of the job activation. This violates the intended rule that only the current job claimant may complete it.

**Required change:** carry `job_key`, `worker_id`, `claim_token` and claim generation into the transition mutation. In the same commit transaction as the process transition, condition the acknowledgement on the job still being claimed by that exact token and require exactly one affected row. If zero rows are affected, roll back the process transition as `LostClaim`. Preserve an explicit durable completion receipt for idempotent response recovery.

### F-03 — High: startup recovery is incompatible with active-active replicas

The server runs `recover_all_tenants` before declaring the startup recovery gate passed (`bpmn-lite-server-runner/src/main.rs:242`). Recovery enumerates every running instance and treats a failed/busy claim as a fatal error (`engine.rs:1625-1670`).

A healthy existing replica may legitimately hold such a lease. Therefore a joining or restarting replica can fail startup because the cluster is doing useful work. As the active instance count and replica count rise, the probability approaches certainty.

**Required change:** startup should validate global schema/artifact prerequisites, then become available. Busy instances must be skipped, not treated as corruption. Full reconciliation should be an incremental, retryable background sweep, preferably leader-owned; normal executors should not require exclusive access to the whole running population to become ready.

### F-04 — High: owner-only release permits stale same-owner lease deletion

Transition release executes:

```sql
UPDATE workflow_instances
SET lease_owner = NULL, lease_until = NULL
WHERE tenant_id = ? AND instance_id = ? AND lease_owner = ?
```

(`store_postgres.rs:4482-4501`). `Claim` contains no owner, lease deadline or acquisition token. An engine also reuses one owner string across concurrent transitions (`engine.rs:207-212`).

Concrete ABA sequence:

1. Task A, owner X, obtains fence 4 and stalls beyond expiry.
2. Task B, also owner X, reacquires after expiry and obtains fence 5.
3. A's commit correctly fails `StaleFence`.
4. A releases by owner X and clears B's current fence-5 lease.
5. Another executor can now claim and fence B while B computes.

The fence continues to protect durable state, but useful work can be repeatedly invalidated. This is a liveness/throughput defect and can become starvation.

**Required change:** every acquisition gets a unique `lease_token`. Return `{owner, lease_token, fence, lease_until}` in `Claim`. Release and renew must match tenant, instance, token and fence. Avoid treating a shared process/pod owner as the identity of a particular acquisition.

### F-05 — High: five-second leases have no renewal path and batches exceed execution capacity

The default transition lease is 5 seconds (`engine.rs:24`; server default in `main.rs:231`). No transition lease renewal/heartbeat API was found. The scheduler can claim 128 instances but executes only 32 concurrently (`engine.rs:30, 858-991`). Thus many claimed leases can spend much of their lifetime merely waiting for a local permit.

The fence makes an overrun safe, but not productive: takeover rejects the old result. Repeated overruns can cause retry churn or livelock. Wasm module loading, cache misses, database contention and host calls enlarge the tail latency even if the kernel itself is fast.

**Required change:** claim near the moment a worker permit is available, size claims to available capacity, and support token-checked renewal for transitions that are explicitly allowed to outlive the normal budget. Keep deterministic kernel steps bounded; do not use heartbeat to hide unbounded execution.

### F-06 — High: stale job recovery ignores the recorded claim expiry

Job activation records `claim_expires_at`, and validation checks it. Stale reclaim instead compares `claimed_at` with a separately supplied timeout (`store_postgres.rs:4402`). The server supplies a hard-coded five minutes every 60 seconds, while dequeue accepts a caller-selected lease.

If a worker requested a shorter lease, recovery is late. If it requested a longer lease, recovery can take a still-valid job. The latter combines dangerously with F-02.

**Required change:** reclaim on `claim_expires_at <= now()`. Configuration may cap allowed lease duration at dequeue time, but recovery authority must use the deadline actually persisted with that acquisition.

### F-07 — High/medium: the last job attempt cannot report success

`validate_job_claim` requires `retries_remaining > 1` (`store_postgres.rs:762-789`). The queue can hold a claimed job with one attempt remaining, but that worker's successful completion is rejected. Stale recovery then dead-letters rows with `retries_remaining <= 1`.

Unless `retries_remaining` is deliberately defined as “future retries excluding the current attempt” throughout the system—and dequeue prevents a claim at zero—the success validator should not test retry budget at all. Ownership, status, token and expiry determine whether a claimed attempt may complete.

### F-08 — Medium: same-owner reacquisition allows duplicate local computation

Claims explicitly permit `lease_owner = requested owner` even while the lease is live, without increasing the fence (`store_postgres.rs:1160-1183, 1230-1240`). Concurrent operations sharing the engine owner may therefore both calculate against the same revision. Revision CAS ensures at most one commits, but the loser wastes work and can trigger F-04 during release.

Unique acquisition tokens plus a per-instance in-process single-flight guard would remove this ambiguity. A semantic command should normally join/retry a busy instance rather than silently becoming a second holder.

### F-09 — Medium: scheduler shutdown is not explicitly drained

Scheduler, reclaim and pruning loops are detached tasks. Graceful server shutdown does not visibly cancel and join these loops. A hard process exit remains state-safe because leases expire, but it creates avoidable duplicate work and recovery latency. Track the tasks under a cancellation token and stop claiming before draining in-flight commits.

### F-10 — Low: useful-work metrics are inaccurate and busy is untyped

`tick_instance_ids_as_owner` returns the number of IDs claimed, even when ticks fail or discover no runnable fibre (`engine.rs:979-994`). Operationally, “claimed”, “executed”, “no-op waiting”, “stale fence”, “busy” and “failed” need separate counters.

A busy transition is currently represented as `Option::None` and often converted into a generic error such as “leased or missing.” A typed `Busy { lease_until/retry_after }` result will make retry policy and alerting materially safer.

## Failure-mode matrix

| Failure point | Current result | Assessment |
|---|---|---|
| Executor dies after claim, before compute | Lease eventually expires; next claim increments fence | State-safe; latency bounded only by lease/reclaimer |
| Executor dies during compute | Same as above | State-safe if all host effects remain outside the kernel transaction |
| Executor dies before SQL commit | SQL transaction rolls back | Safe |
| Executor dies after SQL commit, before release | Complete revision is durable; lease expires later | Safe, with temporary delay |
| Old executor commits after takeover | Fence mismatch rejects commit | Safe |
| Commit succeeds but caller sees transport failure | Durable transition exists, but caller needs an authoritative receipt/re-read policy | Must be tested for every command type |
| Old task releases after same-owner reacquisition | It can clear the new lease | Liveness defect (F-04) |
| Stale job worker resumes after reassignment | It can pass earlier validation and delete/complete the reassigned job | Correctness defect (F-02) |
| New replica starts while another holds a lease | Startup recovery can fail | Availability defect (F-03) |
| Millions of processes wait with no ready fibre | Scheduler continuously claims/decodes/releases them | Scale blocker (F-01) |

## Recommended authority model

Keep two deliberately different persistence categories:

1. **Immutable domain evidence:** transition journal, snapshots/checkpoints, command receipts, business events and outbox records. Inserts are ideal here.
2. **Mutable coordination state:** ready/in-flight activation, claim token, deadline, attempt count and current aggregate head. Updates are appropriate here because a semaphore is current authority, not historical evidence.

A suitable `workflow_activation` record would contain at least:

- tenant ID, activation ID and instance ID;
- durable command/reason and target base revision;
- state: ready, claimed, completed or dead-lettered;
- `available_at`, claim owner, unique claim token, claim deadline and attempt;
- optional priority/partition and creation sequence.

Maintain a uniqueness rule that permits only the intended active head for an instance. Claim ready rows with `FOR UPDATE SKIP LOCKED`, but only after local execution capacity is reserved. A successful transition transaction should:

1. verify activation token, process fence and expected revision;
2. update the mutable process head;
3. insert immutable transition/snapshot/receipt/outbox evidence;
4. mark or delete the consumed activation with the same token;
5. insert any next ready activation(s).

Notifications can reduce wake-up latency, but the activation row—not a notification—is the durable source of truth. This yields disposable compute: killing every Wasm worker loses no authority and the database can enumerate exactly the work that is ready or reclaimable.

## Required verification gates

Before calling the execution layer enterprise-ready, add deterministic tests for:

1. expiry → same-owner reacquisition → old release must not clear the new claim;
2. expiry → new-owner takeover → old commit and old release;
3. renewal with wrong token/fence and renewal racing takeover;
4. execution longer than lease, including a queued batch larger than local permits;
5. two replicas starting while one holds active transition leases;
6. stale job completion racing reclaim and reassignment;
7. successful completion on the final allowed job attempt;
8. reclaim respecting each persisted `claim_expires_at`;
9. crash injection before/after claim, kernel, SQL commit and response delivery;
10. commit success followed by lost response, retried for every external command type;
11. graceful shutdown while claims and commits are in flight;
12. a large population of waiting instances producing no process-row writes or decode work;
13. poison activation/dead-letter behaviour without blocking other instances;
14. fairness and starvation under a hot instance and multiple tenants.

The existing focused in-memory lease test passed, and the lease-related PostgreSQL crate compiled successfully. Existing tests cover different-owner exclusion, wrong-owner release, concurrent claim selection and stale-fence rejection. They do not cover the same-owner ABA release, active-active startup recovery, waiting-population write amplification or the successful job-completion race. PostgreSQL integration tests were not run because no test database URL was available in the review environment.

## Suggested delivery order

1. Atomically bind successful job acknowledgement to its claim token (F-02) and reclaim by recorded expiry (F-06).
2. Add token/fence-aware transition release and renewal; eliminate same-owner reacquisition ambiguity (F-04, F-05, F-08).
3. Replace running-instance polling with durable ready activations (F-01).
4. Make recovery active-active tolerant and drain scheduler tasks on shutdown (F-03, F-09).
5. Add the failure-injection and scale gates above, then tune lease durations from measured tail latency.

This order first closes the correctness hole, then makes authority unambiguous, and finally removes the architecture's dominant scale cost.
