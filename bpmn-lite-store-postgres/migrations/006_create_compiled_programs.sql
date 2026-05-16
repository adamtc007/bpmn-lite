CREATE TABLE compiled_programs (
    bytecode_version  BYTEA PRIMARY KEY,
    program           JSONB NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
