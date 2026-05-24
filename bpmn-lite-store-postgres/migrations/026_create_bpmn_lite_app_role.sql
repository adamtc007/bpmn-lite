-- A18-Session-1 — Create the runtime role used by the bpmn-lite server.
--
-- Migration 025 enabled Row-Level Security on tenant-scoped tables but
-- the application still connects as a Postgres superuser, which
-- *automatically bypasses RLS* (BYPASSRLS is implicit for superusers).
-- The A16 audit identified this as the root reason the RLS layer is
-- structurally inactive at runtime.
--
-- This migration creates the non-superuser role the application must use
-- for runtime work. It does NOT grant BYPASSRLS, so RLS policies apply.
--
-- ── Activation flow ──
--   1. Migrations run as superuser (current_role). This migration is
--      executed at that point and has authority to CREATE ROLE.
--   2. After all migrations complete, the runtime DATABASE_URL is
--      pointed at `bpmn_lite_app` instead of the superuser.
--   3. The server's verify_not_superuser() startup check warns (this
--      session) / errors (A18-Session-3) if it still sees a superuser
--      connection.
--
-- ── Password ──
--   The default password is for development only. Production deployments
--   must override via `ALTER ROLE bpmn_lite_app PASSWORD '<secret>'` (or
--   via environment-driven role provisioning, depending on operational
--   model). Never embed the production password here.
--
-- ── Idempotency ──
--   The IF NOT EXISTS guard makes the migration safe to re-run if the
--   role pre-exists (e.g. created out-of-band in development). The
--   GRANT statements are unconditionally safe to re-issue.
--
-- ── Default privileges ──
--   ALTER DEFAULT PRIVILEGES applies to tables created BY THE ROLE
--   RUNNING THIS MIGRATION (i.e. the migration superuser). Subsequent
--   migrations creating new tables will automatically extend grants
--   to bpmn_lite_app without further intervention.

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bpmn_lite_app') THEN
        CREATE ROLE bpmn_lite_app LOGIN PASSWORD 'bpmn_lite_app_dev_password';
    END IF;
END
$$;

-- Database-level connect privilege (idempotent). GRANT requires a
-- database identifier, so use dynamic SQL for the current database.
DO $$
BEGIN
    EXECUTE format(
        'GRANT CONNECT ON DATABASE %I TO bpmn_lite_app',
        current_database()
    );
END
$$;

-- Schema usage + table CRUD on the public schema.
GRANT USAGE ON SCHEMA public TO bpmn_lite_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO bpmn_lite_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO bpmn_lite_app;

-- Future tables created in this schema by the migration runner inherit
-- these grants automatically.
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO bpmn_lite_app;
ALTER DEFAULT PRIVILEGES IN SCHEMA public
    GRANT USAGE, SELECT ON SEQUENCES TO bpmn_lite_app;
