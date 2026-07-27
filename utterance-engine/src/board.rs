//! Board construction service (V&S §11.7; WS-C C-now item 2).
//!
//! The board is the central safety boundary (I15). Construction contract,
//! in pipeline order — every stage deterministic:
//!
//!   universe (LegalityOracle + provider) → policy filter (PRE-inference,
//!   D19: disclosure minimisation — the model never sees descriptions of
//!   operations the user cannot invoke) → canonical ordering →
//!   NONE_OF_THE_ABOVE appended → content hash over the EXACT board.
//!
//! The hash preimage covers: candidate (canonical_id, description,
//! schema_version) triples in canonical order, the reachability context
//! (anchor), the pack identity, and the policy-filter state — per §11.7
//! "not merely a version number". Repl RECHECKS policy on every proposal
//! regardless (the pre-filter is hygiene, never the gate).
//!
//! Plan §E2: the universe beyond ops/productions (domain verbs, effects,
//! messages) arrives through `BoardUniverseProvider`; the sealed pack
//! becomes a drop-in provider behind the same trait when T3 lands.

use designer_graph::board_candidate::{BoardCandidate, LegalityOracle};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// E2 provider trait: pack-scoped additions to the board universe
/// (domain verbs, effects, messages, decisions) beyond the graph
/// operations/productions the `LegalityOracle` supplies. Implementations
/// must be deterministic for a given pack state.
pub trait BoardUniverseProvider {
    /// Stable identity of the providing pack/registry state (content
    /// hash or pinned version string) — part of the board hash.
    fn pack_identity(&self) -> String;
    /// Additional candidates, already carrying canonical ids in a
    /// provider-owned namespace (e.g. `verb.*`, `effect.*`).
    fn candidates(&self) -> Vec<BoardCandidate>;
}

/// A provider for boards that need nothing beyond graph
/// operations/productions (WS-C bring-up; also the honest state before
/// T3's sealed pack exists).
pub struct EmptyUniverse;
impl BoardUniverseProvider for EmptyUniverse {
    fn pack_identity(&self) -> String {
        "pack.none".to_owned()
    }
    fn candidates(&self) -> Vec<BoardCandidate> {
        Vec::new()
    }
}

/// Pre-inference policy filter state (D19). v1 mechanism: a deny-set of
/// canonical ids. The FILTER runs here (candidates removed before the
/// model sees them); the D19-rider DENIAL RENDERING lives in the
/// disposition layer — an off-board request resolves as
/// off-board/out-of-scope, never as "that operation exists but is
/// forbidden".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PolicyFilter {
    /// Canonical ids the current principal may not see. BTreeSet: the
    /// serialized/hashed form is order-canonical by construction.
    pub denied: BTreeSet<String>,
}

/// The reachability context a board was built against (part of the
/// hash; Q29 interim rule — boards are built only against a RESOLVED
/// position, clarify-first when the subject is uncertain).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoardContext {
    /// BPMN id of the anchor node, or None for the whole-graph position.
    pub anchor: Option<String>,
    pub pack_identity: String,
}

/// The exact, content-addressed inference board (I15).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Board {
    /// Canonically ordered; always ends with the NONE_OF_THE_ABOVE
    /// abstention candidate (R2-r1).
    pub candidates: Vec<BoardCandidate>,
    pub context: BoardContext,
    /// Hex blake3 over the §11.7 closure — see `build_board`.
    pub board_hash: String,
}

impl Board {
    pub fn contains(&self, canonical_id: &str) -> bool {
        self.candidates.iter().any(|c| c.canonical_id == canonical_id)
    }
}

/// Build the board for a position. Deterministic: same oracle state,
/// provider state, anchor, and policy → identical hash.
pub fn build_board<O: LegalityOracle>(
    oracle: &O,
    anchor: Option<&O::NodeKey>,
    anchor_id: Option<&str>,
    provider: &dyn BoardUniverseProvider,
    policy: &PolicyFilter,
) -> Board {
    // Universe: position-legal ops/productions + provider candidates.
    let mut candidates = oracle.legal_candidates(anchor);
    candidates.extend(provider.candidates());

    // Pre-inference policy filter (D19): remove, don't mark.
    candidates.retain(|c| !policy.denied.contains(&c.canonical_id));

    // Canonical ordering across the merged universe.
    candidates.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
    candidates.dedup_by(|a, b| a.canonical_id == b.canonical_id);

    // Explicit abstention candidate on EVERY board (R2-r1), appended
    // after ordering so it is always last and always present.
    candidates.push(BoardCandidate::new(
        designer_graph::board_candidate::CandidateId::Abstain,
    ));

    let context = BoardContext {
        anchor: anchor_id.map(str::to_owned),
        pack_identity: provider.pack_identity(),
    };

    // Content hash (I15/§11.7): candidates as supplied to the model, in
    // order, plus reachability context, pack, and policy-filter state.
    let mut preimage = String::new();
    for c in &candidates {
        preimage.push_str(&c.canonical_id);
        preimage.push('\x1f');
        preimage.push_str(&c.description);
        preimage.push('\x1f');
        preimage.push_str(&c.schema_version.to_string());
        preimage.push('\x1e');
    }
    preimage.push_str("anchor:");
    preimage.push_str(context.anchor.as_deref().unwrap_or("<root>"));
    preimage.push('\x1e');
    preimage.push_str("pack:");
    preimage.push_str(&context.pack_identity);
    preimage.push('\x1e');
    preimage.push_str("denied:");
    for d in &policy.denied {
        preimage.push_str(d);
        preimage.push('\x1f');
    }
    let board_hash = blake3::hash(preimage.as_bytes()).to_hex().to_string();

    Board {
        candidates,
        context,
        board_hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use designer_graph::board_candidate::{OperationKind, ProductionId};

    struct AllLegal;
    impl LegalityOracle for AllLegal {
        type NodeKey = ();
        fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
            OperationKind::ALL.to_vec()
        }
        fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
            ProductionId::ALL.to_vec()
        }
    }

    /// GREEN determinism + RED sensitivity: same inputs → same hash;
    /// changed anchor or policy state → different hash (the hash is a
    /// content address, not a version label).
    #[test]
    fn board_hash_is_content_addressed() {
        let a = build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default());
        let b = build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default());
        assert_eq!(a.board_hash, b.board_hash, "determinism");

        let anchored = build_board(&AllLegal, None, Some("t1"), &EmptyUniverse, &PolicyFilter::default());
        assert_ne!(a.board_hash, anchored.board_hash, "anchor is hashed");

        let mut policy = PolicyFilter::default();
        policy.denied.insert("op.delete_subgraph".to_owned());
        let filtered = build_board(&AllLegal, None, None, &EmptyUniverse, &policy);
        assert_ne!(a.board_hash, filtered.board_hash, "policy state is hashed");
    }

    /// D19: a denied candidate is REMOVED — absent from the board the
    /// model sees, not marked forbidden. And NONE_OF_THE_ABOVE is on
    /// every board, last.
    #[test]
    fn policy_filter_removes_and_abstention_is_always_present() {
        let mut policy = PolicyFilter::default();
        policy.denied.insert("op.delete_subgraph".to_owned());
        let board = build_board(&AllLegal, None, None, &EmptyUniverse, &policy);
        assert!(!board.contains("op.delete_subgraph"), "denied candidate must be absent");
        assert!(board.contains("op.append_node"), "undenied candidates remain");
        assert_eq!(
            board.candidates.last().unwrap().canonical_id,
            crate::contract::NONE_OF_THE_ABOVE,
            "abstention candidate always present, always last"
        );
        // 28 legal - 1 denied + NOTA = 28
        assert_eq!(board.candidates.len(), 28);
    }

    /// Canonical ordering is by canonical_id regardless of oracle or
    /// provider emission order (position-invariance is a MODEL test,
    /// §10.7 — ordering here is for reproducibility, R2-r2).
    #[test]
    fn board_ordering_is_canonical() {
        let board = build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default());
        let ids: Vec<&str> = board
            .candidates
            .iter()
            .take(board.candidates.len() - 1) // NOTA appended last by design
            .map(|c| c.canonical_id.as_str())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}
