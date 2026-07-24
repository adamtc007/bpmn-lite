-- V6.4 destructive cutover wipe.
--
-- Clears all persisted v2 runtime/execution state and the artifact corpus
-- (to be recompiled). This is an execution-state cutover, not a
-- re-provisioning: config (tenants, tenant_pools) is intentionally preserved.
--
-- TRUNCATE ... CASCADE follows FK references, so per-instance runtime tables
-- are cleared transitively rather than each being named here -- including the
-- V8 guard_failure_budget table (FK instance_id -> workflow_instances
-- ON DELETE CASCADE), plus fibers, join_barriers, workflow_effects,
-- workflow_timers, workflow_inbox, workflow_journal and the rest of the state
-- hung off workflow_instances. TRUNCATE ignores row-level security, so a
-- FORCE-RLS table (guard_failure_budget, dsl_bus.inbox) is still cleared.
--
-- FK-ORPHANED live state MUST be named explicitly -- these tables carry live
-- runtime state but have NO FK path to workflow_instances, so CASCADE does not
-- reach them:
--   * message_buffer     -- inter-instance buffered messages (uncons. delivery)
--   * dsl_bus.inbox      -- receiver idempotency/dedup keys (outbox's counterpart)
--   * workflow_payloads  -- tenant-scoped payload blobs, content-addressed so
--                           wiping is safe, and its sibling payload_history is
--                           wiped too -- kept symmetric for a clean cold start.
--                           Truncated CASCADE: job_queue FK-references it.
--
-- There is no workflow_instances -> compiled_programs FK, so corpus truncation
-- order relative to instances is not FK-constrained.

TRUNCATE dsl_bus.outbox CASCADE;
TRUNCATE dsl_bus.inbox CASCADE;
TRUNCATE workflow_instances CASCADE;
TRUNCATE compiled_programs;
TRUNCATE job_queue;
TRUNCATE dedupe_cache;
TRUNCATE message_dedupe;
TRUNCATE message_buffer;
TRUNCATE dead_letter_queue;
TRUNCATE event_sequences;
TRUNCATE event_log;
TRUNCATE payload_history;
TRUNCATE workflow_payloads CASCADE;
TRUNCATE incidents;
TRUNCATE artifact_lineage;
