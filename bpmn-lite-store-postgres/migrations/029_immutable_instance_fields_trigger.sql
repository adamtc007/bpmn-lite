-- A19 (revised) — Database-enforced immutability on process_instances.
--
-- The identity fields of a process instance are set at creation and must
-- never change. This trigger rejects any UPDATE that attempts to alter:
--
--   instance_id        — primary key; must never change
--   tenant_id          — ownership; tampering would reassign the instance
--   bytecode_version   — the compiled program the instance runs
--   process_key        — the process definition identifier
--   entry_id           — originating runbook entry
--   runbook_id         — originating runbook
--   created_at         — creation timestamp
--   integrity_hash     — birth certificate; written once, never overwritten
--
-- mutable fields (domain_payload, domain_payload_hash, session_stack, flags,
-- counters, join_expected, state, correlation_id, quarantine_state) are not
-- restricted here — the engine updates them legitimately during execution.
--
-- Design rationale: enforcement here eliminates the need for application-layer
-- hash re-verification on every scheduler pickup or gRPC handler call. The DB
-- rejects tampering at write time, once, rather than the application re-checking
-- at read time, repeatedly, for every instance operation.
--
-- Note: superusers can bypass triggers. This trigger operates within the
-- bpmn_lite_app role model. Direct superuser writes to production Postgres
-- are an operational control problem, not a code problem.

CREATE OR REPLACE FUNCTION process_instances_enforce_immutable_fields()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.instance_id != OLD.instance_id THEN
        RAISE EXCEPTION 'process_instances.instance_id is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.tenant_id != OLD.tenant_id THEN
        RAISE EXCEPTION 'process_instances.tenant_id is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.bytecode_version != OLD.bytecode_version THEN
        RAISE EXCEPTION 'process_instances.bytecode_version is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.process_key != OLD.process_key THEN
        RAISE EXCEPTION 'process_instances.process_key is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.entry_id != OLD.entry_id THEN
        RAISE EXCEPTION 'process_instances.entry_id is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.runbook_id != OLD.runbook_id THEN
        RAISE EXCEPTION 'process_instances.runbook_id is immutable (instance %)', OLD.instance_id;
    END IF;
    IF NEW.created_at != OLD.created_at THEN
        RAISE EXCEPTION 'process_instances.created_at is immutable (instance %)', OLD.instance_id;
    END IF;
    -- integrity_hash: immutable once set. Allow NULL → value (first write on
    -- pre-A19 rows) but never allow value → different value or value → NULL.
    IF OLD.integrity_hash IS NOT NULL
       AND (NEW.integrity_hash IS NULL OR NEW.integrity_hash != OLD.integrity_hash) THEN
        RAISE EXCEPTION 'process_instances.integrity_hash is immutable once set (instance %)', OLD.instance_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER process_instances_immutable_fields
BEFORE UPDATE ON process_instances
FOR EACH ROW EXECUTE FUNCTION process_instances_enforce_immutable_fields();
