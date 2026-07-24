// Deliberate-violation fixture for check-transition-read-safety.sh's
// self-test. `some_new_hazard` is not in the script's HELPER_FUNCTIONS or
// SAFE_FUNCTIONS allowlist, and reads `.concurrency_table()` directly
// instead of going through `fetch_record_in_transition` — the lint must
// fire against this or it has gone toothless.

fn some_new_hazard(
    snapshot: &Snapshot,
    changes: &mut Changes,
    handle: RecordId,
) -> Option<ConcurrencyRecord> {
    let record = snapshot.concurrency_table().get(handle).cloned();
    changes.concurrency_mutations.push(ConcurrencyMutation::Retire(handle));
    record
}
