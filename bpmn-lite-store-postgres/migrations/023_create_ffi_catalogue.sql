-- ffi_template: cross-vocabulary FFI template catalogue (A2 §6).
-- Templates are content-addressed (template_id = BLAKE3 32 bytes) and
-- immutable after publication. Tenant-scoped; GLOBAL tenant is the
-- nil UUID '00000000-...-000000000000'.
--
-- The schema layout follows A2 §6 exactly. RLS policies are NOT enabled
-- here; tenancy enforcement is A16 work.

CREATE TABLE ffi_template (
    template_id         BYTEA        PRIMARY KEY,
    template_uuidv7     UUID         NOT NULL UNIQUE,
    owner_type          TEXT         NOT NULL,
    owner_metadata      BYTEA        NOT NULL,
    input_schema_json   JSONB        NOT NULL,
    output_schema_json  JSONB        NOT NULL,
    idempotency_json    JSONB        NOT NULL,
    tenant_id           TEXT         NOT NULL,
    published_at        TIMESTAMPTZ  NOT NULL DEFAULT now(),
    publisher           TEXT         NOT NULL
);

CREATE INDEX ffi_template_owner_tenant
    ON ffi_template(owner_type, tenant_id);

CREATE INDEX ffi_template_tenant
    ON ffi_template(tenant_id);
