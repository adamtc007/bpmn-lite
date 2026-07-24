-- V6.4 destructive cutover wipe.
--
-- Clears all persisted v2 runtime/execution state and the artifact corpus
-- (to be recompiled), mirroring the store test harness's own proven-clean
-- reset set (bpmn-lite-store-postgres store_postgres.rs `setup()`).
--
-- `TRUNCATE ... CASCADE` follows FK references, so dependent runtime tables
-- are cleared transitively rather than each being named here — including the
-- V8 `guard_failure_budget` table (FK instance_id -> workflow_instances
-- ON DELETE CASCADE), plus fibers, join_barriers, workflow_effects,
-- workflow_timers, workflow_journal and the rest of the per-instance runtime
-- state hung off `workflow_instances`. TRUNCATE ignores row-level security,
-- so a FORCE-RLS table like `guard_failure_budget` is still cleared.
--
-- Provisioning/config (tenants, tenant_pools) is intentionally NOT wiped —
-- this is an execution-state cutover, not a re-provisioning.
--
-- Order matters: instances are truncated (CASCADE) before compiled_programs
-- so the instance -> artifact FK does not block the corpus truncation.

TRUNCATE dsl_bus.outbox CASCADE;
TRUNCATE workflow_instances CASCADE;
TRUNCATE compiled_programs;
TRUNCATE job_queue;
TRUNCATE dedupe_cache;
TRUNCATE message_dedupe;
TRUNCATE dead_letter_queue;
TRUNCATE event_sequences;
TRUNCATE event_log;
TRUNCATE payload_history;
TRUNCATE incidents;
TRUNCATE artifact_lineage;
