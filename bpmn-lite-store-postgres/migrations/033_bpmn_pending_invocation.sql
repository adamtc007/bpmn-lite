-- v0.6 §8.3 — caller-side pending-call registry.
--
-- bpmn-lite owns this table. One row per in-flight cross-domain
-- callout submitted by the BPMN executor. Two-stage durability:
--
-- Stage 1 (caller commits intent): row inserted with `callout_id` +
--   `idempotency_key` set, `execution_id` = NULL, `ack_received_at` = NULL.
--   The matching outbox row is inserted in the same transaction, and
--   the process instance transitions to `WaitingOnSubmission(callout_id)`.
--
-- Stage 2 (ack received): the outbox sender updates this row with the
--   receiver-assigned `execution_id` + `ack_received_at`, and
--   transitions the process to `WaitingOnInvocation(execution_id)`.
--
-- Stage 3 (result received): the row is deleted, atomic with the
--   process advance (bpmn-lite-bus-handler::ProcessAdvancer).
CREATE TABLE bpmn_pending_invocation (
    callout_id          UUID        PRIMARY KEY,            -- caller-side identity (always present)

    process_instance_id UUID        NOT NULL,
    node_id             TEXT        NOT NULL,

    target_domain       TEXT        NOT NULL,
    verb_id             TEXT        NOT NULL,

    idempotency_key     UUID        NOT NULL UNIQUE,
    execution_id        UUID        UNIQUE,                 -- nullable; filled after SubmissionAck

    submitted_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    ack_received_at     TIMESTAMPTZ,                        -- when execution_id was recorded
    timeout_at          TIMESTAMPTZ
);

CREATE INDEX idx_pending_process
    ON bpmn_pending_invocation (process_instance_id);

CREATE INDEX idx_pending_execution
    ON bpmn_pending_invocation (execution_id)
    WHERE execution_id IS NOT NULL;

CREATE INDEX idx_pending_timeout
    ON bpmn_pending_invocation (timeout_at)
    WHERE timeout_at IS NOT NULL;
