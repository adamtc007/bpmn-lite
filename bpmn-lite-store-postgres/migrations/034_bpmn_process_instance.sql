-- v0.6 §8.4 — long-lived BPMN process instance (Commitment B scope).
--
-- One row per BPMN workflow process started via bpmn-lite. Distinct
-- from the inner `process_instances` table (which is the fiber VM's
-- per-callout execution state). This is the *outer* workflow DAG:
--
--   start-event → service-task → exclusive-gateway → service-task → end-event
--
-- The status column drives the executor's blocking states. Two-stage
-- durability is reflected in WaitingOnSubmission vs WaitingOnInvocation:
--
--   Created           — instance row inserted, executor hasn't started walking
--   Running           — actively walking intra-process nodes (gateways, etc.)
--   WaitingOnSubmission — committed a callout to outbox; awaiting SubmissionAck
--                         (waiting_on_callout_id set; waiting_on_execution_id NULL)
--   WaitingOnInvocation — receiver acked; awaiting result delivery
--                         (waiting_on_execution_id set; waiting_on_callout_id may
--                         still be set for diagnostics)
--   Completed         — reached an end-event; completed_at + end_status filled
--   Failed            — fatal error (VerbFailed, VersionMismatch, …); failure_reason set
CREATE TABLE bpmn_process_instance (
    id                      UUID        PRIMARY KEY,
    workflow_id             TEXT        NOT NULL,

    current_node            TEXT        NOT NULL,
    status                  TEXT        NOT NULL
                            CHECK (status IN (
                                'Created',
                                'Running',
                                'WaitingOnSubmission',
                                'WaitingOnInvocation',
                                'Completed',
                                'Failed'
                            )),
    variables               JSONB       NOT NULL DEFAULT '{}'::jsonb,

    waiting_on_callout_id   UUID,
    waiting_on_execution_id UUID,

    started_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_advanced_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at            TIMESTAMPTZ,

    end_status              TEXT,
    failure_reason          TEXT
);

CREATE INDEX idx_bpmn_pi_status
    ON bpmn_process_instance (status);

CREATE INDEX idx_bpmn_pi_waiting_callout
    ON bpmn_process_instance (waiting_on_callout_id)
    WHERE waiting_on_callout_id IS NOT NULL;

CREATE INDEX idx_bpmn_pi_waiting_execution
    ON bpmn_process_instance (waiting_on_execution_id)
    WHERE waiting_on_execution_id IS NOT NULL;
