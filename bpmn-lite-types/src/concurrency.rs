//! D1 — control stack + concurrency table (EOP-VS-BPMN-ISA-002 §2, §4).
//!
//! Two canonical facts, bound by an invariant (K-2) maintained exclusively
//! by word execution inside `kernel::apply`:
//!   - what am I inside?   -> `Fiber::control_stack` (`Vec<Handle>`)
//!   - who is inside me?   -> `ConcurrencyTable` membership
//!
//! `Addr` (artifact address) and `RecordId` (runtime handle) are distinct
//! types with no conversion between them — the static-structure /
//! dynamic-activation law from V&S §4 ("Artifact addresses never appear in
//! runtime state; runtime handles never appear in artifacts") is a compile
//! error, not a convention. See the `no_addr_to_record_id_conversion`
//! doctest below for the proof.
//!
//! V1 declares this surface as typed record-keeping. The words that
//! allocate/retire/mutate records (`GUARD>`, `RACE{`, `FORK`, `JOIN`, ...)
//! land in V4; V1's `TransitionBuilder` only carries the mutation deltas.

use crate::types::{Addr, FlagKey, JoinId, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// A runtime concurrency-table record identity. Deterministically derived
/// (context-derived, stable under replay per D1's determinism rider) —
/// never a raw `Uuid::new_v4()`. Distinct from `Addr`: see module docs.
///
/// V1.1 gate: a deliberate `Addr` -> `RecordId` conversion attempt,
/// committed as a compile-fail test. No `From`/`Into` impl exists between
/// the two types, and they wrap incompatible primitives (`u32` vs `Uuid`),
/// so this is a hard compiler error, not a lint.
///
/// ```compile_fail
/// use bpmn_lite_types::RecordId;
/// use bpmn_lite_types::Addr;
///
/// let addr = Addr::new(1);
/// let _record_id: RecordId = addr.into();
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct RecordId(Uuid);

impl RecordId {
    pub const fn new(id: Uuid) -> Self {
        Self(id)
    }

    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A fibre's reference to a concurrency-table record — literally a
/// `RecordId` pushed on `Fiber::control_stack` (V&S §2, "Handle"). Not a
/// second type: the glossary distinguishes the term by role, not by value.
pub type Handle = RecordId;

/// The kind of a concurrency-table record (V&S §5 word inventory + §5
/// deferred-to-v3 admissions). `Compensation` is uninhabited for v2 — no
/// v2 word constructs it — but the discriminant exists now so retirement
/// (which must *archive*, not delete, compensation-kind records) needs no
/// future migration.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum RecordKind {
    /// `GUARD>` / `<GUARD`. `interrupting = true` unwinds members on
    /// trigger; `false` (`GUARD-N>` / `<GUARD-N`) spawns the handler
    /// without unwinding.
    Guard { interrupting: bool },
    /// `RACE{` / `}RACE` — first-wins over N armed alternatives.
    Race,
    /// `FORK n` / `JOIN` — arrival-counter barrier.
    Barrier,
    /// Deferred to v3 (V&S §5). No v2 word allocates this variant.
    Compensation,
}

/// Lifecycle state of a concurrency-table record.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum RecordState {
    /// Open: members may still join, the handler may still fire.
    Armed,
    /// Closed by its pairing word (`<GUARD`, `}RACE`, last `JOIN`) or by
    /// cancellation. Retained rather than removed where `kind` demands
    /// history (compensation, v3).
    Retired,
}

/// Barrier arrival counters (K-3: `0 <= count <= arity`, retirement exactly
/// at zero). `Race`-kind records also carry a non-default value here
/// (`arity = count = arm_count` at open, per `V2RaceOpen`) — K-3's literal
/// bound is stated for `Barrier` specifically, but the same 0-at-birth-
/// would-be-a-bug shape holds for `Race` by construction (kernel comment on
/// `Instr::V2RaceOpen`). `Guard`- and `Compensation`-kind records leave this
/// at `RecordCounters::default()` — no v2 word ever sets it for them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub struct RecordCounters {
    pub arity: u32,
    pub count: u32,
}

/// One concurrency-table record: `{ id, kind, members, handler, state,
/// counters }` per V&S §4. `members` is a `BTreeSet` for canonical
/// ordering (Ring 1 requires BTreeMap/BTreeSet-only, fixed field order).
///
/// `rollback_domain_payload`/`rollback_domain_payload_hash`/
/// `rollback_flags`/`rollback_join_expected`/`rollback_session_stack`
/// (A18, superseding V4.1's original ruling): populated ONLY by
/// `V2GuardR`'s opening word — never by `V2Guard`/`V2GuardN`, which A18
/// re-derives as control-only opcodes with no data disposition. V4.1's
/// original text called the rollback snapshot "a standard lifecycle
/// snapshot" captured unconditionally by every guard-kind opener; A18's
/// root diagnosis is exactly that this conflated control disposition
/// (interrupting/non-interrupting: unwind-or-not, spawn-handler-or-not)
/// with data disposition (restore-or-not), which are orthogonal and need
/// independent opcodes. `V2CancelScope` (restricted to `V2GuardR`-opened
/// handles, V-10's companion check) and automatic rollback-on-definitive-
/// failure both restore all five fields together — A3's rollback-set rule:
/// `domain_payload`, business-meaningful `flags`, and `join_expected` are
/// restored; `ProcessInstance.counters` (loop/retry bounds) are
/// deliberately NOT captured or restored here — restoring them would let
/// a failing scope retry unboundedly, defeating their own bound. The
/// kernel is pure (E4) and cannot reach the store's `payload_history`
/// mid-transition, so the actual snapshot values (not just a hash) must
/// travel here for the restore to be a pure function of already-available
/// snapshot state. `rollback_session_stack` is stored as opaque serialized
/// JSON text (`serde_json::to_string` of `SessionStackState`), the same
/// "preserve opaquely, never parsed by the VM" treatment `domain_payload`
/// itself already gets on `ProcessInstance` — `SessionStackState`'s own
/// `workspace_stack: Vec<serde_json::Value>` field can only round-trip
/// through this module's canonical-encoding primitives via the fallible
/// `encode_canonical_json` path (arbitrary caller-supplied JSON, e.g. a
/// non-finite float), and `CanonicalEncode::canonical_encode` is
/// infallible by trait signature — so this field is encoded as an opaque
/// canonical string, exactly like `rollback_domain_payload`, rather than
/// structurally. This is deliberately narrower than
/// `RecordKind::Compensation`'s full "reverse-order handler execution"
/// (§5) — that record kind stays uninhabited by v2 (see its own doc
/// comment); this is a property of `V2GuardR` scopes specifically.
///
/// `opened_at` (V&S §15, v0.7 ruling F): the static bytecode `Addr` of the
/// `Guard`-kind record's own opening word (`V2Guard`/`V2GuardN`), `None`
/// for every other kind. This is NOT a §4 violation — an `Addr` recorded
/// on runtime state that the machine never reads to decide execution is
/// the same category as an `Addr` in a journal event; only a runtime
/// handle steering control flow is what §4 forbids. Ruling F needs it
/// because `RecordId` does not survive a guard's re-open (the record
/// retires with the rollback cascade) but its static `Addr` does, and the
/// store-side repeated-failure budget must be keyed by something that
/// outlives one activation.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ConcurrencyRecord {
    pub id: RecordId,
    pub kind: RecordKind,
    pub members: BTreeSet<Uuid>,
    pub handler: Option<Addr>,
    pub state: RecordState,
    pub counters: RecordCounters,
    pub rollback_domain_payload: Option<Box<str>>,
    pub rollback_domain_payload_hash: Option<[u8; 32]>,
    /// A3 rollback-set (A18): business-meaningful orchestration flags at
    /// `V2GuardR` open time. `None` for every other guard-kind record.
    pub rollback_flags: Option<BTreeMap<FlagKey, Value>>,
    /// A3 rollback-set (A18): dynamic join-expected counts at `V2GuardR`
    /// open time. `None` for every other guard-kind record.
    pub rollback_join_expected: Option<BTreeMap<JoinId, u16>>,
    /// A3 rollback-set (A18): `SessionStackState` at `V2GuardR` open time,
    /// serialized opaquely (see the struct doc comment for why). `None`
    /// for every other guard-kind record.
    pub rollback_session_stack: Option<Box<str>>,
    pub opened_at: Option<Addr>,
    /// §18 v0.10 ruling I / BoundaryError v2 migration: armed error-match
    /// routes for a `V2Guard`/`V2GuardN`/`V2GuardR` record, populated by
    /// `V2GuardArmError`. Empty for a guard with no error arms (the common
    /// case — most guards are timer-armed or unarmed). Sorted specific-code-
    /// first, catch-all (`error_code: None`) last at construction time (same
    /// precedence v1's deleted `error_route_map` used) so `apply_job_failure`'s
    /// match walk is a plain `iter().find()`.
    pub error_routes: Vec<(Option<Box<str>>, Addr)>,
}

impl ConcurrencyRecord {
    pub fn new(id: RecordId, kind: RecordKind) -> Self {
        Self {
            id,
            kind,
            members: BTreeSet::new(),
            handler: None,
            state: RecordState::Armed,
            counters: RecordCounters::default(),
            rollback_domain_payload: None,
            rollback_domain_payload_hash: None,
            rollback_flags: None,
            rollback_join_expected: None,
            rollback_session_stack: None,
            opened_at: None,
            error_routes: Vec::new(),
        }
    }
}

/// Rejects a `ConcurrencyRecord` that violates a shape invariant this module
/// documents for its `kind` (pub-scope audit, 2026-07-29). Narrower than
/// K-1/K-2 (checked by `bpmn-lite-kernel::check_k_invariants`, which needs
/// the fibre map alongside the table and so cannot run at single-record
/// insert time) — this checks only what a lone record can prove about
/// itself:
///
/// - `Barrier`/`Race`: `counters.count <= counters.arity` (K-3's bound;
///   `Race` is included because the same shape holds for it by
///   construction — see `RecordCounters`'s doc comment), and `opened_at`/
///   the five `rollback_*` fields must all be unset (no v2 word sets them
///   for these kinds).
/// - `Guard`: `counters` must be `RecordCounters::default()` (unused for
///   this kind), `opened_at` must be set (every v2 guard opener sets it),
///   the five `rollback_*` fields must be all-set-together or all-unset-
///   together (never partial — a half-populated rollback snapshot is
///   unusable), and a non-interrupting guard (`interrupting: false`, i.e.
///   `V2GuardN`) must have them all unset (A18: `V2GuardN` is never
///   rollback-eligible).
/// - `Compensation`: rejected outright — uninhabited in v2, no v2 word
///   constructs it (see `RecordKind`'s doc comment), so an insert
///   attempting one is definitionally not legitimate kernel output.
///
/// Called at the store-commit boundary (`bpmn-lite-store`,
/// `bpmn-lite-store-postgres`), the one place a `ConcurrencyMutation::Insert`
/// can reach persistence without having passed through `kernel::apply`'s own
/// Ring 3 shadow check first — confirmed via `bpmn-lite-store-postgres`'s own
/// test suite, which hand-crafts `Transition`s with directly-constructed
/// `ConcurrencyRecord`s and commits them without going through the kernel at
/// all.
pub fn validate_concurrency_record_shape(
    record: &ConcurrencyRecord,
) -> Result<(), ConcurrencyValidationError> {
    let id = record.id;
    let rollback_fields_present = [
        record.rollback_domain_payload.is_some(),
        record.rollback_domain_payload_hash.is_some(),
        record.rollback_flags.is_some(),
        record.rollback_join_expected.is_some(),
        record.rollback_session_stack.is_some(),
    ];
    let any_rollback_set = rollback_fields_present.contains(&true);
    let all_rollback_set = rollback_fields_present.iter().all(|&set| set);

    match record.kind {
        RecordKind::Compensation => Err(ConcurrencyValidationError::CompensationUninhabited { id }),
        RecordKind::Barrier | RecordKind::Race => {
            let RecordCounters { arity, count } = record.counters;
            if count > arity {
                Err(ConcurrencyValidationError::CountersOutOfBounds {
                    id,
                    kind: record.kind,
                    count,
                    arity,
                })
            } else if record.opened_at.is_some() {
                Err(ConcurrencyValidationError::UnexpectedOpenedAt { id, kind: record.kind })
            } else if any_rollback_set {
                Err(ConcurrencyValidationError::RollbackFieldsOnNonGuard { id, kind: record.kind })
            } else {
                Ok(())
            }
        }
        RecordKind::Guard { interrupting } => {
            if record.counters != RecordCounters::default() {
                Err(ConcurrencyValidationError::UnexpectedCounters {
                    id,
                    kind: record.kind,
                    arity: record.counters.arity,
                    count: record.counters.count,
                })
            } else if record.opened_at.is_none() {
                Err(ConcurrencyValidationError::GuardMissingOpenedAt { id })
            } else if any_rollback_set && !all_rollback_set {
                Err(ConcurrencyValidationError::PartialRollbackSnapshot { id })
            } else if !interrupting && any_rollback_set {
                Err(ConcurrencyValidationError::RollbackFieldsOnGuardN { id })
            } else {
                Ok(())
            }
        }
    }
}

/// Errors from [`validate_concurrency_record_shape`] and the store-layer
/// `Retire`/`Remove`-on-missing-id checks that accompany it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum ConcurrencyValidationError {
    #[error("record {id}: {kind:?} counters out of bounds (count={count} > arity={arity})")]
    CountersOutOfBounds {
        id: RecordId,
        kind: RecordKind,
        count: u32,
        arity: u32,
    },
    #[error("record {id}: kind {kind:?} must not carry non-default counters (arity={arity}, count={count})")]
    UnexpectedCounters {
        id: RecordId,
        kind: RecordKind,
        arity: u32,
        count: u32,
    },
    #[error("record {id}: kind {kind:?} must not carry opened_at (Guard-only field)")]
    UnexpectedOpenedAt { id: RecordId, kind: RecordKind },
    #[error("record {id}: kind {kind:?} must not carry rollback_* fields (Guard-only, A18)")]
    RollbackFieldsOnNonGuard { id: RecordId, kind: RecordKind },
    #[error("record {id}: rollback_* fields must be populated all-together or not at all")]
    PartialRollbackSnapshot { id: RecordId },
    #[error("record {id}: rollback_* fields set on a non-interrupting Guard (V2GuardN is never rollback-eligible, A18)")]
    RollbackFieldsOnGuardN { id: RecordId },
    #[error("record {id}: Guard-kind record missing opened_at")]
    GuardMissingOpenedAt { id: RecordId },
    #[error("record {id}: RecordKind::Compensation is uninhabited in v2 -- no v2 word constructs it, rejecting insert")]
    CompensationUninhabited { id: RecordId },
    #[error("cannot retire record {0}: not present in the concurrency table")]
    RetireMissing(RecordId),
    #[error("cannot remove record {0}: not present in the concurrency table")]
    RemoveMissing(RecordId),
}

/// The snapshot-resident table of concurrency records, keyed by record ID
/// (V&S §2, "Concurrency table"). `BTreeMap` for the same canonical-form
/// reason as `members` above.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct ConcurrencyTable(BTreeMap<RecordId, ConcurrencyRecord>);

impl ConcurrencyTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, record: ConcurrencyRecord) {
        self.0.insert(record.id, record);
    }

    pub fn remove(&mut self, id: RecordId) -> Option<ConcurrencyRecord> {
        self.0.remove(&id)
    }

    pub fn get(&self, id: RecordId) -> Option<&ConcurrencyRecord> {
        self.0.get(&id)
    }

    pub fn get_mut(&mut self, id: RecordId) -> Option<&mut ConcurrencyRecord> {
        self.0.get_mut(&id)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&RecordId, &ConcurrencyRecord)> {
        self.0.iter()
    }

    /// Dry-run validate a sequence of mutations against this table without
    /// mutating it: every `Insert` must pass
    /// [`validate_concurrency_record_shape`], and every `Retire`/`Remove`
    /// must target a record present as of its position in the sequence — so
    /// an `Insert` followed later, in the same batch, by a `Retire`/`Remove`
    /// for that same id is valid (mirrors e.g. a nested-guard close staged
    /// within one transition). Store commit paths call this against the
    /// currently-persisted table, before applying the identical mutations
    /// for real, so a validation failure surfaces before any state is
    /// persisted — see `validate_concurrency_record_shape`'s doc comment for
    /// why this check needs to live at the store boundary at all.
    pub fn validate_mutations(
        &self,
        mutations: &[ConcurrencyMutation],
    ) -> Result<(), ConcurrencyValidationError> {
        let mut scratch = self.clone();
        for mutation in mutations {
            match mutation {
                ConcurrencyMutation::Insert(record) => {
                    validate_concurrency_record_shape(record)?;
                    scratch.insert((**record).clone());
                }
                ConcurrencyMutation::Retire(id) => {
                    scratch
                        .get_mut(*id)
                        .ok_or(ConcurrencyValidationError::RetireMissing(*id))?
                        .state = RecordState::Retired;
                }
                ConcurrencyMutation::Remove(id) => {
                    scratch
                        .remove(*id)
                        .ok_or(ConcurrencyValidationError::RemoveMissing(*id))?;
                }
            }
        }
        Ok(())
    }
}

/// Deltas the `TransitionBuilder` accumulates against the concurrency
/// table. V1 declares the surface; V4's words are the sole producers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ConcurrencyMutation {
    /// Boxed (clippy `large_enum_variant`): `ConcurrencyRecord` grew past
    /// the size-difference threshold against `Retire`/`Remove`'s bare
    /// `RecordId` once the BoundaryError v2 migration added
    /// `error_routes`. Every existing match site still works unchanged —
    /// field access on a `&Box<ConcurrencyRecord>` binding auto-derefs;
    /// only construction sites needed `Box::new(record)` and the few
    /// `record.clone()` sites feeding a `ConcurrencyRecord`-typed sink
    /// needed `(*record).clone()`.
    Insert(Box<ConcurrencyRecord>),
    Retire(RecordId),
    Remove(RecordId),
}

/// A control-stack push/pop against one fibre. V1 declares the surface;
/// V4's words are the sole producers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ControlStackDelta {
    Push { fiber_id: Uuid, handle: Handle },
    Pop { fiber_id: Uuid, handle: Handle },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrency_table_round_trips_through_canonical_json() {
        let mut table = ConcurrencyTable::new();
        let id = RecordId::new(Uuid::from_u128(1));
        table.insert(ConcurrencyRecord::new(id, RecordKind::Barrier));
        let bytes = serde_json::to_vec(&table).unwrap();
        let decoded: ConcurrencyTable = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.get(id).map(|r| r.kind), Some(RecordKind::Barrier));
    }

    #[test]
    fn compensation_kind_is_constructible_but_unused_by_v2() {
        // Admission requirement (V&S §5): the discriminant exists now.
        // Nothing in v2 word execution allocates it — this test only
        // proves the type is complete, not that it is reachable.
        let id = RecordId::new(Uuid::from_u128(2));
        let record = ConcurrencyRecord::new(id, RecordKind::Compensation);
        assert_eq!(record.state, RecordState::Armed);
    }

    /// V1.1 gate anchor — the actual proof is the `compile_fail` doctest
    /// on `RecordId`'s doc comment (doctests don't run inside
    /// `#[cfg(test)]` modules, since rustdoc builds without `--cfg test`).
    #[test]
    fn addr_to_record_id_conversion_is_a_compile_error() {}

    // ── validate_concurrency_record_shape (pub-scope audit, 2026-07-29) ──

    fn v2fork_style_barrier(id: RecordId, arity: u32, count: u32) -> ConcurrencyRecord {
        let mut record = ConcurrencyRecord::new(id, RecordKind::Barrier);
        record.counters = RecordCounters { arity, count };
        record
    }

    fn v2race_open_style_race(id: RecordId, arity: u32, count: u32) -> ConcurrencyRecord {
        let mut record = ConcurrencyRecord::new(id, RecordKind::Race);
        record.counters = RecordCounters { arity, count };
        record
    }

    fn v2guard_style_guard(id: RecordId, interrupting: bool) -> ConcurrencyRecord {
        ConcurrencyRecord {
            opened_at: Some(crate::types::Addr::new(1)),
            ..ConcurrencyRecord::new(id, RecordKind::Guard { interrupting })
        }
    }

    fn v2guard_r_style_guard(id: RecordId) -> ConcurrencyRecord {
        ConcurrencyRecord {
            rollback_domain_payload: Some("{}".into()),
            rollback_domain_payload_hash: Some([0u8; 32]),
            rollback_flags: Some(BTreeMap::new()),
            rollback_join_expected: Some(BTreeMap::new()),
            rollback_session_stack: Some("[]".into()),
            opened_at: Some(crate::types::Addr::new(1)),
            ..ConcurrencyRecord::new(id, RecordKind::Guard { interrupting: true })
        }
    }

    #[test]
    fn validate_accepts_every_real_kernel_construction_shape() {
        let id = RecordId::new(Uuid::from_u128(10));
        assert!(validate_concurrency_record_shape(&v2fork_style_barrier(id, 3, 3)).is_ok());
        assert!(validate_concurrency_record_shape(&v2fork_style_barrier(id, 3, 0)).is_ok());
        assert!(validate_concurrency_record_shape(&v2race_open_style_race(id, 2, 2)).is_ok());
        assert!(validate_concurrency_record_shape(&v2guard_style_guard(id, true)).is_ok());
        assert!(validate_concurrency_record_shape(&v2guard_style_guard(id, false)).is_ok());
        assert!(validate_concurrency_record_shape(&v2guard_r_style_guard(id)).is_ok());
    }

    #[test]
    fn validate_rejects_compensation_outright() {
        let id = RecordId::new(Uuid::from_u128(11));
        let record = ConcurrencyRecord::new(id, RecordKind::Compensation);
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::CompensationUninhabited { id })
        );
    }

    #[test]
    fn validate_rejects_barrier_count_over_arity() {
        let id = RecordId::new(Uuid::from_u128(12));
        let record = v2fork_style_barrier(id, 2, 3);
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::CountersOutOfBounds {
                id,
                kind: RecordKind::Barrier,
                count: 3,
                arity: 2,
            })
        );
    }

    #[test]
    fn validate_rejects_race_count_over_arity() {
        let id = RecordId::new(Uuid::from_u128(13));
        let record = v2race_open_style_race(id, 1, 2);
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::CountersOutOfBounds {
                id,
                kind: RecordKind::Race,
                count: 2,
                arity: 1,
            })
        );
    }

    #[test]
    fn validate_rejects_guard_missing_opened_at() {
        let id = RecordId::new(Uuid::from_u128(14));
        let record = ConcurrencyRecord::new(id, RecordKind::Guard { interrupting: true });
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::GuardMissingOpenedAt { id })
        );
    }

    #[test]
    fn validate_rejects_guard_with_partial_rollback_snapshot() {
        let id = RecordId::new(Uuid::from_u128(15));
        let mut record = v2guard_style_guard(id, true);
        record.rollback_domain_payload = Some("{}".into()); // only one of five set
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::PartialRollbackSnapshot { id })
        );
    }

    #[test]
    fn validate_rejects_guard_n_carrying_a_rollback_snapshot() {
        let id = RecordId::new(Uuid::from_u128(16));
        let mut record = v2guard_style_guard(id, false);
        record.rollback_domain_payload = Some("{}".into());
        record.rollback_domain_payload_hash = Some([0u8; 32]);
        record.rollback_flags = Some(BTreeMap::new());
        record.rollback_join_expected = Some(BTreeMap::new());
        record.rollback_session_stack = Some("[]".into());
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::RollbackFieldsOnGuardN { id })
        );
    }

    #[test]
    fn validate_rejects_barrier_carrying_rollback_fields() {
        let id = RecordId::new(Uuid::from_u128(17));
        let mut record = v2fork_style_barrier(id, 1, 1);
        record.rollback_domain_payload = Some("{}".into());
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::RollbackFieldsOnNonGuard {
                id,
                kind: RecordKind::Barrier,
            })
        );
    }

    #[test]
    fn validate_rejects_guard_with_nonzero_counters() {
        let id = RecordId::new(Uuid::from_u128(18));
        let mut record = v2guard_style_guard(id, true);
        record.counters = RecordCounters { arity: 1, count: 1 };
        assert_eq!(
            validate_concurrency_record_shape(&record),
            Err(ConcurrencyValidationError::UnexpectedCounters {
                id,
                kind: RecordKind::Guard { interrupting: true },
                arity: 1,
                count: 1,
            })
        );
    }

    // ── ConcurrencyTable::validate_mutations ──────────────────────────────

    #[test]
    fn validate_mutations_rejects_retire_of_a_missing_id() {
        let table = ConcurrencyTable::new();
        let id = RecordId::new(Uuid::from_u128(20));
        assert_eq!(
            table.validate_mutations(&[ConcurrencyMutation::Retire(id)]),
            Err(ConcurrencyValidationError::RetireMissing(id))
        );
    }

    #[test]
    fn validate_mutations_rejects_remove_of_a_missing_id() {
        let table = ConcurrencyTable::new();
        let id = RecordId::new(Uuid::from_u128(21));
        assert_eq!(
            table.validate_mutations(&[ConcurrencyMutation::Remove(id)]),
            Err(ConcurrencyValidationError::RemoveMissing(id))
        );
    }

    #[test]
    fn validate_mutations_accepts_insert_then_retire_in_the_same_batch() {
        let table = ConcurrencyTable::new();
        let id = RecordId::new(Uuid::from_u128(22));
        let record = v2guard_style_guard(id, true);
        let mutations = vec![
            ConcurrencyMutation::Insert(Box::new(record)),
            ConcurrencyMutation::Retire(id),
        ];
        assert!(table.validate_mutations(&mutations).is_ok());
    }

    #[test]
    fn validate_mutations_rejects_a_malformed_insert() {
        let table = ConcurrencyTable::new();
        let id = RecordId::new(Uuid::from_u128(23));
        let record = ConcurrencyRecord::new(id, RecordKind::Compensation);
        let mutations = vec![ConcurrencyMutation::Insert(Box::new(record))];
        assert_eq!(
            table.validate_mutations(&mutations),
            Err(ConcurrencyValidationError::CompensationUninhabited { id })
        );
    }
}
