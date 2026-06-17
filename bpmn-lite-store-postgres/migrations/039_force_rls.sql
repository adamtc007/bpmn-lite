-- Migration 039: Force row-level security on the five core RLS tables.
-- By default, PostgreSQL RLS policies do not apply to the table owner.
-- Enabling FORCE ROW LEVEL SECURITY ensures RLS is applied to table owners
-- if they are not superusers, protecting against accidental ownership bypass.

ALTER TABLE process_instances FORCE ROW LEVEL SECURITY;
ALTER TABLE job_queue FORCE ROW LEVEL SECURITY;
ALTER TABLE ffi_template FORCE ROW LEVEL SECURITY;
ALTER TABLE ffi_invocation_record FORCE ROW LEVEL SECURITY;
ALTER TABLE incidents FORCE ROW LEVEL SECURITY;
