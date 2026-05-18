-- Migration 031: add pool_id to tenants
--
-- Column is added without FK constraint here; the FK is added in 032 after
-- the default pool row is seeded (can't reference a row that doesn't exist yet).
-- Existing rows get the DEFAULT 'default' pool.

ALTER TABLE tenants
    ADD COLUMN IF NOT EXISTS pool_id TEXT NOT NULL DEFAULT 'default';
