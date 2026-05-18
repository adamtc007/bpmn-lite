-- Tenant directory.
--
-- Every tenant that has ever started a process is registered here.
-- The scheduler reads this table to enumerate active tenants for
-- its tick loop. No RLS — it's a directory, not tenant-owned data.
--
-- `ensure_tenant(tenant_id)` is called from `atomic_start` so the row
-- appears on first process creation. The "default" tenant registers
-- automatically on first use; nothing needs to pre-populate it.

CREATE TABLE IF NOT EXISTS tenants (
    tenant_id    TEXT        PRIMARY KEY,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
