-- Migration 036: allow the non-superuser runtime role to run bus storage
-- migrations in its own database.
--
-- Main bpmn-lite schema migrations run through DATABASE_ADMIN_URL, but the
-- federated bus storage layer runs its small outbox/inbox migrations from the
-- runtime pool. That pool uses bpmn_lite_app so RLS remains active, which means
-- it needs CREATE on public to create the bus tables and migration metadata.

GRANT CREATE ON SCHEMA public TO bpmn_lite_app;
