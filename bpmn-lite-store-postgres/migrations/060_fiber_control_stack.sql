-- 060: Persist fiber control stacks (D1, V&S §2/§4).
--
-- V4's concurrency words populate `Fiber.control_stack` (ordered handles
-- into the snapshot's concurrency table). Until now the store had no
-- column for it: save silently dropped the stack and load fabricated an
-- empty one, so ANY parallel region broke kernel invariant K-2
-- ("fibre is a member of armed record X but does not have it on its
-- control stack") after the first persistence round trip. Memory-store
-- runs never noticed because nothing left process memory.
--
-- The '[]' default is exact for pre-existing rows: every row written
-- before this migration had its control stack dropped at save time, so
-- there is nothing truthful to backfill — in-flight parallel instances
-- from before this migration were already unrecoverable.
ALTER TABLE fibers
    ADD COLUMN IF NOT EXISTS control_stack JSONB NOT NULL DEFAULT '[]';
