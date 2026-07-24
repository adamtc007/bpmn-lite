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
/// use bpmn_lite_types::concurrency::RecordId;
/// use bpmn_lite_types::types::Addr;
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
/// at zero). Unused by non-barrier record kinds.
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
}
