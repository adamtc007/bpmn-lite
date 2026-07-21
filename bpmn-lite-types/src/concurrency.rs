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

use crate::types::Addr;
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
/// `rollback_domain_payload`/`rollback_domain_payload_hash` (V4.1, Adam-
/// ratified): a `Guard`-kind record captures the process instance's
/// `domain_payload` at the moment its opening word (`V2Guard`/`V2GuardN`)
/// executes — "a standard lifecycle snapshot," not opt-in, not a distinct
/// word. `V2CancelScope` restores it. This is deliberately narrower than
/// `RecordKind::Compensation`'s full "reverse-order handler execution"
/// (§5) — that record kind stays uninhabited by v2 (see its own doc
/// comment); this is a property of `Guard` scopes specifically. The
/// kernel is pure (E4) and cannot reach the store's `payload_history`
/// mid-transition, so the actual payload text (not just its hash) must
/// travel here for `V2CancelScope` to restore it as a pure function of
/// already-available snapshot state.
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
    Insert(ConcurrencyRecord),
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
