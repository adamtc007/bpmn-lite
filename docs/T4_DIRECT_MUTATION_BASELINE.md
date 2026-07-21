# T4 direct-mutation burn-down baseline

T4 marks 28 `ProcessStore` mutation methods for deletion in T7. The three
composite compatibility methods (`atomic_start`, `atomic_complete`, and
`commit_tick`) already delegate to `commit_transition`; they remain counted
because T7 must migrate their callers and delete the aliases.

The remaining methods cover direct instance, fiber, join, job, dedupe,
message, event, payload, incident, and quarantine writes. T7's exit gate is
zero: the methods and their compatibility conversion must be deleted, not
un-deprecated or suppressed.

Changing the count requires completing the matching T7 burn-down work and
updating this ledger toward zero; functional acceptance remains the fenced
transition and atomicity tests rather than a text-count check.
