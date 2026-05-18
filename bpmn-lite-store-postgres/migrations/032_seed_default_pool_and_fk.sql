-- Migration 032: seed default pool row and enforce FK
--
-- Insert the default pool first, then add and validate the FK so all existing
-- tenants (pool_id = 'default') satisfy the constraint immediately.

INSERT INTO tenant_pools (pool_id, pool_type, description)
VALUES ('default', 'default', 'Default shared bpmn-lite worker pool')
ON CONFLICT (pool_id) DO NOTHING;

ALTER TABLE tenants
    ADD CONSTRAINT IF NOT EXISTS tenants_pool_id_fk
    FOREIGN KEY (pool_id) REFERENCES tenant_pools (pool_id)
    ON DELETE RESTRICT;
