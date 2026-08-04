-- Phase 2 (F-04 remediation, docs/todo/zed_agent_execution_lease_remediation_plan.md):
-- transition release/renewal must be keyed on a unique acquisition
-- identity, not owner alone. Owner strings are reused across concurrent
-- operations from the same engine instance and across expiry boundaries,
-- so an owner-only release cannot tell a stale (already-superseded)
-- acquisition apart from the live one it just replaced — see
-- test_phase0_f04_same_owner_aba_release_clears_new_claim.
ALTER TABLE workflow_instances
    ADD COLUMN IF NOT EXISTS lease_token TEXT;
