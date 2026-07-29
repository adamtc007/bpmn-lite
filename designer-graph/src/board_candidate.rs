//! Board-candidate interface — the WS-A.1 slice frozen FIRST, because
//! WS-C's board constructor consumes it (plan §C ordering constraint 1).
//!
//! This module answers exactly two questions for a Designer position:
//! candidate IDENTITY (what may appear on a board, stably addressable and
//! content-hashable) and LEGALITY (which operations/productions are legal
//! at this position). It deliberately answers nothing else — policy
//! filtering, canonical ordering, `NONE_OF_THE_ABOVE` insertion, and the
//! board content hash are WS-C's (V&S §11.7); this crate supplies raw
//! legal candidates only.
//!
//! FREEZE RULE: `canonical_id` strings and `CANDIDATE_SCHEMA_VERSION`
//! are board-hash inputs (I15/I28). Changing an id or a description is a
//! schema-version bump, never a silent edit — the golden test below is
//! the cement.

use serde::{Deserialize, Serialize};

/// Bumped whenever any canonical id or model-facing description changes.
/// Rides every `BoardCandidate` so recorded board hashes stay
/// interpretable across schema evolution (I28).
const CANDIDATE_SCHEMA_VERSION: u32 = 2;

/// The §12.1 atomic graph-operation set. Variant set mirrors the V&S
/// verbatim; the three guard operations mirror the opcode trichotomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum OperationKind {
    AppendNode,
    InsertBefore,
    InsertAfter,
    ReplaceNode,
    Connect,
    CreateBranch,
    CreateRace,
    CreateParallelRegion,
    CloseParallelRegion,
    CreateInclusiveRegion,
    CreateMultiInstanceRegion,
    AttachGuard,
    AttachRearmingGuard,
    AttachRollbackGuard,
    SetGuardTrigger,
    SetGuardBudget,
    SetCorrelationSource,
    CallSubprocess,
    DeleteSubgraph,
}

/// The §12.2 production catalogue (Q4's candidate set; pruned/extended
/// only via a schema-version bump).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProductionId {
    RequestAndWait,
    TimerMessageRace,
    ReminderThenEscalate,
    ParallelChecksAndJoin,
    InterruptingTimeout,
    NonInterruptingNotification,
    HumanReviewWithRework,
    CallDurableSubprocess,
    ForEachWithCeiling,
}

/// Stable, content-hashable candidate identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum CandidateId {
    Operation(OperationKind),
    Production(ProductionId),
    /// The explicit abstention candidate (R2-r1) present on every board.
    /// Owned by the board constructor (utterance-engine), not by any
    /// legality oracle — listed here so no consumer ever has to fake it
    /// with a real operation/production discriminant.
    Abstain,
}

impl OperationKind {
    pub const ALL: [OperationKind; 19] = [
        OperationKind::AppendNode,
        OperationKind::InsertBefore,
        OperationKind::InsertAfter,
        OperationKind::ReplaceNode,
        OperationKind::Connect,
        OperationKind::CreateBranch,
        OperationKind::CreateRace,
        OperationKind::CreateParallelRegion,
        OperationKind::CloseParallelRegion,
        OperationKind::CreateInclusiveRegion,
        OperationKind::CreateMultiInstanceRegion,
        OperationKind::AttachGuard,
        OperationKind::AttachRearmingGuard,
        OperationKind::AttachRollbackGuard,
        OperationKind::SetGuardTrigger,
        OperationKind::SetGuardBudget,
        OperationKind::SetCorrelationSource,
        OperationKind::CallSubprocess,
        OperationKind::DeleteSubgraph,
    ];

    pub fn canonical_id(self) -> &'static str {
        match self {
            OperationKind::AppendNode => "op.append_node",
            OperationKind::InsertBefore => "op.insert_before",
            OperationKind::InsertAfter => "op.insert_after",
            OperationKind::ReplaceNode => "op.replace_node",
            OperationKind::Connect => "op.connect",
            OperationKind::CreateBranch => "op.create_branch",
            OperationKind::CreateRace => "op.create_race",
            OperationKind::CreateParallelRegion => "op.create_parallel_region",
            OperationKind::CloseParallelRegion => "op.close_parallel_region",
            OperationKind::CreateInclusiveRegion => "op.create_inclusive_region",
            OperationKind::CreateMultiInstanceRegion => "op.create_multi_instance_region",
            OperationKind::AttachGuard => "op.attach_guard",
            OperationKind::AttachRearmingGuard => "op.attach_rearming_guard",
            OperationKind::AttachRollbackGuard => "op.attach_rollback_guard",
            OperationKind::SetGuardTrigger => "op.set_guard_trigger",
            OperationKind::SetGuardBudget => "op.set_guard_budget",
            OperationKind::SetCorrelationSource => "op.set_correlation_source",
            OperationKind::CallSubprocess => "op.call_subprocess",
            OperationKind::DeleteSubgraph => "op.delete_subgraph",
        }
    }

    /// The description exactly as supplied to the model (board-hash input).
    pub fn description(self) -> &'static str {
        match self {
            OperationKind::AppendNode => "Append a new node after the current end of a sequence",
            OperationKind::InsertBefore => "Insert a new node before the anchor node",
            OperationKind::InsertAfter => "Places a node on an existing route, after the selected node",
            OperationKind::ReplaceNode => "Replace the anchor node, preserving its connections",
            OperationKind::Connect => "Joins two existing nodes with a typed connector",
            OperationKind::CreateBranch => "Adds a new outgoing route with its own outcome key",
            OperationKind::CreateRace => "Create a first-wins race over declared arms",
            OperationKind::CreateParallelRegion => "Open a parallel fork/join region",
            OperationKind::CloseParallelRegion => "Close an open parallel region at its join",
            OperationKind::CreateInclusiveRegion => "Open an inclusive (conditional multi-branch) region",
            OperationKind::CreateMultiInstanceRegion => "Create a per-element multi-instance region over a bounded collection with a declared maximum",
            OperationKind::AttachGuard => "Attach an interrupting boundary guard to the anchor",
            OperationKind::AttachRearmingGuard => "Attach a non-interrupting (re-arming) boundary guard to the anchor",
            OperationKind::AttachRollbackGuard => "Attach an interrupting rollback guard (data restored to scope open) to the anchor",
            OperationKind::SetGuardTrigger => "Set a guard's arming trigger (timer duration or bounded cycle)",
            OperationKind::SetGuardBudget => "Set a guard's failure budget (overrides the workflow default)",
            OperationKind::SetCorrelationSource => "Set a wait node's message correlation source expression",
            OperationKind::CallSubprocess => "Call a published durable subprocess by pinned reference",
            OperationKind::DeleteSubgraph => "Delete the anchor node or a complete enclosed region",
        }
    }
}

impl ProductionId {
    pub const ALL: [ProductionId; 9] = [
        ProductionId::RequestAndWait,
        ProductionId::TimerMessageRace,
        ProductionId::ReminderThenEscalate,
        ProductionId::ParallelChecksAndJoin,
        ProductionId::InterruptingTimeout,
        ProductionId::NonInterruptingNotification,
        ProductionId::HumanReviewWithRework,
        ProductionId::CallDurableSubprocess,
        ProductionId::ForEachWithCeiling,
    ];

    pub fn canonical_id(self) -> &'static str {
        match self {
            ProductionId::RequestAndWait => "prod.request_and_wait",
            ProductionId::TimerMessageRace => "prod.timer_message_race",
            ProductionId::ReminderThenEscalate => "prod.reminder_then_escalate",
            ProductionId::ParallelChecksAndJoin => "prod.parallel_checks_and_join",
            ProductionId::InterruptingTimeout => "prod.interrupting_timeout",
            ProductionId::NonInterruptingNotification => "prod.non_interrupting_notification",
            ProductionId::HumanReviewWithRework => "prod.human_review_with_rework",
            ProductionId::CallDurableSubprocess => "prod.call_durable_subprocess",
            ProductionId::ForEachWithCeiling => "prod.for_each_with_ceiling",
        }
    }

    /// The description exactly as supplied to the model (board-hash input).
    pub fn description(self) -> &'static str {
        match self {
            ProductionId::RequestAndWait => "Send a request and wait for its correlated response",
            ProductionId::TimerMessageRace => "Race a message arrival against a timer deadline",
            ProductionId::ReminderThenEscalate => "Non-interrupting bounded reminder cycle with an escalation continuation",
            ProductionId::ParallelChecksAndJoin => "Run declared checks in parallel and join on all",
            ProductionId::InterruptingTimeout => "Interrupting timeout guard around the anchored work",
            ProductionId::NonInterruptingNotification => "Fire a notification on a schedule without interrupting the work",
            ProductionId::HumanReviewWithRework => "Human review with bounded, attempt-counted forward rework",
            ProductionId::CallDurableSubprocess => "Invoke a durable subprocess and await its typed outcome",
            ProductionId::ForEachWithCeiling => "For each element of a bounded collection, under a mandatory declared maximum",
        }
    }
}

impl CandidateId {
    pub fn canonical_id(self) -> &'static str {
        match self {
            CandidateId::Operation(op) => op.canonical_id(),
            CandidateId::Production(p) => p.canonical_id(),
            CandidateId::Abstain => "abstain.none_of_the_above",
        }
    }
}

/// One raw legal candidate at a Designer position. WS-C assembles boards
/// from these (adding policy filtering, canonical ordering,
/// `NONE_OF_THE_ABOVE`, and the content hash).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoardCandidate {
    /// NOT part of any hash preimage: `CandidateId`'s serde form leaks
    /// Rust variant names. WS-C's board-hash preimage is
    /// `(canonical_id, description, schema_version)` ONLY — contract
    /// fact, cemented by `descriptions_are_content_cemented` below.
    pub id: CandidateId,
    /// `canonical_id()` duplicated in serialized form so recorded boards
    /// are self-describing without this crate's enum layout.
    pub canonical_id: String,
    /// Description exactly as supplied to the model — board-hash input.
    pub description: String,
    pub schema_version: u32,
}

impl BoardCandidate {
    pub fn new(id: CandidateId) -> Self {
        let description = match id {
            CandidateId::Operation(op) => op.description(),
            CandidateId::Production(p) => p.description(),
            CandidateId::Abstain => "None of the available options matches this request",
        };
        BoardCandidate {
            id,
            canonical_id: id.canonical_id().to_owned(),
            description: description.to_owned(),
            schema_version: CANDIDATE_SCHEMA_VERSION,
        }
    }
}

/// Position-dependent legality (V&S §11.7: "the board changes with
/// position"). Implemented by the WS-A.1 DAG schema; consumed by WS-C's
/// board constructor. `anchor = None` means the whole-graph position
/// (empty graph / no selection).
pub trait LegalityOracle {
    /// Opaque node key type of the implementing schema.
    type NodeKey;

    fn legal_operations(&self, anchor: Option<&Self::NodeKey>) -> Vec<OperationKind>;
    fn legal_productions(&self, anchor: Option<&Self::NodeKey>) -> Vec<ProductionId>;

    /// Assembled raw candidates for a position, in canonical-id order
    /// (deterministic input to WS-C; WS-C still applies its own canonical
    /// ordering over the merged board).
    fn legal_candidates(&self, anchor: Option<&Self::NodeKey>) -> Vec<BoardCandidate> {
        let mut out: Vec<BoardCandidate> = self
            .legal_operations(anchor)
            .into_iter()
            .map(|op| BoardCandidate::new(CandidateId::Operation(op)))
            .chain(
                self.legal_productions(anchor)
                    .into_iter()
                    .map(|p| BoardCandidate::new(CandidateId::Production(p))),
            )
            .collect();
        out.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
        out.dedup_by(|a, b| a.canonical_id == b.canonical_id);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// CEMENT: canonical ids are unique and exactly this golden set.
    /// These strings are board-hash inputs (I15/I28) — any change here is
    /// a CANDIDATE_SCHEMA_VERSION bump plus a deliberate edit of this
    /// golden list, never a drive-by rename.
    #[test]
    fn canonical_ids_are_unique_and_golden() {
        let ids: Vec<&str> = OperationKind::ALL
            .iter()
            .map(|op| op.canonical_id())
            .chain(ProductionId::ALL.iter().map(|p| p.canonical_id()))
            .collect();
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "duplicate canonical id");
        assert_eq!(ids.len(), 28);

        let golden: BTreeSet<&str> = [
            "op.append_node",
            "op.insert_before",
            "op.insert_after",
            "op.replace_node",
            "op.connect",
            "op.create_branch",
            "op.create_race",
            "op.create_parallel_region",
            "op.close_parallel_region",
            "op.create_inclusive_region",
            "op.create_multi_instance_region",
            "op.attach_guard",
            "op.attach_rearming_guard",
            "op.attach_rollback_guard",
            "op.set_guard_trigger",
            "op.set_guard_budget",
            "op.set_correlation_source",
            "op.call_subprocess",
            "op.delete_subgraph",
            "prod.request_and_wait",
            "prod.timer_message_race",
            "prod.reminder_then_escalate",
            "prod.parallel_checks_and_join",
            "prod.interrupting_timeout",
            "prod.non_interrupting_notification",
            "prod.human_review_with_rework",
            "prod.call_durable_subprocess",
            "prod.for_each_with_ceiling",
        ]
        .into_iter()
        .collect();
        assert_eq!(unique, golden, "canonical id set drifted from golden");
        assert_eq!(CANDIDATE_SCHEMA_VERSION, 2);
    }

    /// CEMENT (review F6): descriptions are board-hash inputs — a
    /// description edit without a CANDIDATE_SCHEMA_VERSION bump breaks
    /// this hash, not just a comment. To change: bump the version AND
    /// update the golden hex deliberately.
    #[test]
    fn descriptions_are_content_cemented() {
        let mut preimage = String::new();
        for op in OperationKind::ALL {
            preimage.push_str(op.canonical_id());
            preimage.push('\x1f');
            preimage.push_str(op.description());
            preimage.push('\x1e');
        }
        for p in ProductionId::ALL {
            preimage.push_str(p.canonical_id());
            preimage.push('\x1f');
            preimage.push_str(p.description());
            preimage.push('\x1e');
        }
        // Review C3: the Abstain candidate's id+description are
        // board-hash inputs too — cemented here with the rest.
        preimage.push_str(CandidateId::Abstain.canonical_id());
        preimage.push('\x1f');
        preimage.push_str("None of the available options matches this request");
        preimage.push('\x1e');
        preimage.push_str(&CANDIDATE_SCHEMA_VERSION.to_string());
        let hash = blake3::hash(preimage.as_bytes()).to_hex().to_string();
        assert_eq!(
            hash, GOLDEN_DESCRIPTION_HASH,
            "candidate id/description content drifted without a schema-version bump"
        );
    }

    const GOLDEN_DESCRIPTION_HASH: &str = "d6b84d029b4fe6c419cd6a64dbe9e16693e2442243eaf133db114609c9c12097";

    /// Legality-oracle default assembly is deterministic and sorted.
    #[test]
    fn legal_candidates_assembly_is_deterministic_and_sorted() {
        struct Everything;
        impl LegalityOracle for Everything {
            type NodeKey = ();
            fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
                OperationKind::ALL.to_vec()
            }
            fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
                ProductionId::ALL.to_vec()
            }
        }
        let a = Everything.legal_candidates(None);
        let b = Everything.legal_candidates(None);
        assert_eq!(a.len(), 28);
        let ids_a: Vec<&str> = a.iter().map(|c| c.canonical_id.as_str()).collect();
        let ids_b: Vec<&str> = b.iter().map(|c| c.canonical_id.as_str()).collect();
        assert_eq!(ids_a, ids_b);
        let mut sorted = ids_a.clone();
        sorted.sort();
        assert_eq!(ids_a, sorted, "default assembly must be canonical-id sorted");

        // Dedup (review F6): a repeating oracle yields no duplicate entries.
        struct Doubled;
        impl LegalityOracle for Doubled {
            type NodeKey = ();
            fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
                let mut v = OperationKind::ALL.to_vec();
                v.extend(OperationKind::ALL);
                v
            }
            fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
                vec![]
            }
        }
        assert_eq!(Doubled.legal_candidates(None).len(), 19);
    }
}
