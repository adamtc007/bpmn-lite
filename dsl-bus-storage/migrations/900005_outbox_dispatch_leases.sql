-- T8.6: lease outbox rows in a short transaction, then release all row
-- locks before network dispatch begins.
ALTER TABLE dsl_bus.outbox
    DROP CONSTRAINT IF EXISTS outbox_status_check;
ALTER TABLE dsl_bus.outbox
    ADD CONSTRAINT outbox_status_check
    CHECK (status IN ('pending', 'dispatching', 'submitted', 'retrying', 'failed'));

ALTER TABLE dsl_bus.outbox
    ADD COLUMN claim_owner TEXT,
    ADD COLUMN claim_token UUID,
    ADD COLUMN claim_until TIMESTAMPTZ,
    ADD CONSTRAINT outbox_claim_shape CHECK (
        (claim_owner IS NULL) = (claim_token IS NULL)
        AND (claim_owner IS NULL) = (claim_until IS NULL)
    );

CREATE INDEX idx_dsl_bus_outbox_claimable
    ON dsl_bus.outbox (tenant_id, next_attempt_at, id)
    WHERE status IN ('pending', 'dispatching');

-- Down path:
-- DROP INDEX dsl_bus.idx_dsl_bus_outbox_claimable;
-- ALTER TABLE dsl_bus.outbox DROP CONSTRAINT outbox_claim_shape;
-- ALTER TABLE dsl_bus.outbox DROP COLUMN claim_owner, DROP COLUMN claim_token,
--   DROP COLUMN claim_until;
-- Restore the former status CHECK only after converting dispatching rows back
-- to pending. Rolling back abandons active dispatch leases.
