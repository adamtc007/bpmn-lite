-- ffi_invocation_record: per-call audit log for FFI invocations (A2 §9).
-- Append-only (write Pending then UPDATE to terminal outcome). Crash-
-- recovery sweeps for stale Pending rows older than a configurable
-- threshold and applies the template's Idempotency policy.

CREATE TABLE ffi_invocation_record (
    invocation_id               UUID         PRIMARY KEY,
    caller_process_instance_id  UUID         NOT NULL,
    caller_task_id              TEXT         NOT NULL,
    caller_pc                   INTEGER      NOT NULL,
    template_id                 BYTEA        NOT NULL,
    owner_type                  TEXT         NOT NULL,
    tenant_id                   TEXT         NOT NULL,
    invoked_at                  TIMESTAMPTZ  NOT NULL,
    input_payload               BYTEA        NOT NULL,
    outcome_kind                TEXT         NOT NULL
                                             CHECK (outcome_kind IN ('pending', 'success', 'no_match', 'incident')),
    output_payload              BYTEA,
    trace_payload               BYTEA,
    error_payload               BYTEA
);

CREATE INDEX ffi_invocation_instance
    ON ffi_invocation_record(caller_process_instance_id, invoked_at);

CREATE INDEX ffi_invocation_template
    ON ffi_invocation_record(template_id, tenant_id);

CREATE INDEX ffi_invocation_pending
    ON ffi_invocation_record(outcome_kind, invoked_at)
    WHERE outcome_kind = 'pending';
