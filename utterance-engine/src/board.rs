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
use semantic_decision_contracts::SemanticDecisionBoard;
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
    /// Identity of the graph revision legality was computed against
    /// (review C7): without it, delete-and-recreate of an id makes two
    /// different artifact states hash-indistinguishable. WS-B supplies
    /// the session's current revision/graph hash; None only in unit
    /// contexts, and hashed distinctly either way.
    pub graph_identity: Option<String>,
    pub pack_identity: String,
}

/// Length-prefixed, domain-tagged preimage writer (review B1): every
/// variable-length field is written as `<tag>:<byte-len>:<bytes>`, so no
/// field value — description, pack id, denied entry, anchor named
/// literally "<root>" — can forge a field boundary or a sentinel.
fn put(preimage: &mut Vec<u8>, tag: &str, value: &str) {
    preimage.extend_from_slice(tag.as_bytes());
    preimage.push(b':');
    preimage.extend_from_slice(value.len().to_string().as_bytes());
    preimage.push(b':');
    preimage.extend_from_slice(value.as_bytes());
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
        self.candidates
            .iter()
            .any(|c| c.canonical_id == canonical_id)
    }
}

/// One candidate as consumed by evidence producers.
///
/// The owned textual projection keeps the interface object-safe and lets the
/// semantic implementation expose its complete contract rather than reducing
/// it to the legacy one-line description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceCandidate {
    pub canonical_id: String,
    pub description: String,
    pub schema_version: u32,
}

/// Read-only board surface shared by the legacy and semantic serving paths.
///
/// This is deliberately not a board constructor or conversion trait: the
/// graph-backed endpoint retains the original `SemanticDecisionBoard` and no
/// second, thin authority object is created beside it.
pub trait InferenceBoard: Send + Sync {
    fn inference_candidates(&self) -> Vec<InferenceCandidate>;
    fn board_hash(&self) -> &str;
    fn contains(&self, canonical_id: &str) -> bool;
    fn candidate_description(&self, canonical_id: &str) -> Option<String>;
    fn anchor(&self) -> Option<&str>;
    fn graph_identity(&self) -> &str;
    fn pack_identity(&self) -> &str;
    fn schema_label(&self) -> &'static str;
    fn semantic_board(&self) -> Option<&SemanticDecisionBoard> {
        None
    }
}

impl InferenceBoard for Board {
    fn inference_candidates(&self) -> Vec<InferenceCandidate> {
        self.candidates
            .iter()
            .map(|candidate| InferenceCandidate {
                canonical_id: candidate.canonical_id.clone(),
                description: candidate.description.clone(),
                schema_version: candidate.schema_version,
            })
            .collect()
    }

    fn board_hash(&self) -> &str {
        &self.board_hash
    }

    fn contains(&self, canonical_id: &str) -> bool {
        self.contains(canonical_id)
    }

    fn candidate_description(&self, canonical_id: &str) -> Option<String> {
        self.candidates
            .iter()
            .find(|candidate| candidate.canonical_id == canonical_id)
            .map(|candidate| candidate.description.clone())
    }

    fn anchor(&self) -> Option<&str> {
        self.context.anchor.as_deref()
    }

    fn graph_identity(&self) -> &str {
        self.context.graph_identity.as_deref().unwrap_or_default()
    }

    fn pack_identity(&self) -> &str {
        &self.context.pack_identity
    }

    fn schema_label(&self) -> &'static str {
        "legacy_thin_v1"
    }
}

impl InferenceBoard for SemanticDecisionBoard {
    fn inference_candidates(&self) -> Vec<InferenceCandidate> {
        self.candidates
            .iter()
            .map(|candidate| InferenceCandidate {
                canonical_id: candidate.canonical_id.as_str().to_string(),
                description: crate::exact::serialize_candidate(candidate),
                schema_version: candidate.schema_version,
            })
            .collect()
    }

    fn board_hash(&self) -> &str {
        self.board_hash.as_str()
    }

    fn contains(&self, canonical_id: &str) -> bool {
        self.candidates
            .iter()
            .any(|candidate| candidate.canonical_id.as_str() == canonical_id)
    }

    fn candidate_description(&self, canonical_id: &str) -> Option<String> {
        self.candidates
            .iter()
            .find(|candidate| candidate.canonical_id.as_str() == canonical_id)
            .map(|candidate| candidate.intent_summary.clone())
    }

    fn anchor(&self) -> Option<&str> {
        self.position.anchor.as_deref()
    }

    fn graph_identity(&self) -> &str {
        self.graph_revision.as_str()
    }

    fn pack_identity(&self) -> &str {
        self.semantic_snapshot.as_str()
    }

    fn schema_label(&self) -> &'static str {
        "semantic_decision_board_v1"
    }

    fn semantic_board(&self) -> Option<&SemanticDecisionBoard> {
        Some(self)
    }
}

/// Build the board for a position. Deterministic: same oracle state,
/// provider state, anchor, and policy → identical hash. Fail-closed
/// (review C6): reserved-namespace candidates and same-id/different-
/// content collisions are ERRORS, never silently deduped.
pub fn build_board<O: LegalityOracle>(
    oracle: &O,
    anchor: Option<(&O::NodeKey, &str)>,
    graph_identity: Option<&str>,
    provider: &dyn BoardUniverseProvider,
    policy: &PolicyFilter,
) -> anyhow::Result<Board> {
    // Universe: position-legal ops/productions + provider candidates.
    let mut candidates = oracle.legal_candidates(anchor.map(|(k, _)| k));
    candidates.extend(provider.candidates());

    // Reserved namespace (review C6): only build_board itself may emit
    // the abstention candidate.
    if let Some(c) = candidates
        .iter()
        .find(|c| c.canonical_id.starts_with("abstain."))
    {
        return Err(anyhow::anyhow!(
            "candidate '{}' uses the reserved abstain.* namespace — only the board              constructor emits abstention",
            c.canonical_id
        ));
    }

    // Pre-inference policy filter (D19): remove, don't mark.
    candidates.retain(|c| !policy.denied.contains(&c.canonical_id));

    // Canonical ordering; identical duplicates collapse, same-id with
    // DIFFERENT content is a namespace-collision defect (review C6).
    candidates.sort_by(|a, b| a.canonical_id.cmp(&b.canonical_id));
    let mut deduped: Vec<BoardCandidate> = Vec::with_capacity(candidates.len());
    for c in candidates {
        match deduped.last() {
            Some(prev) if prev.canonical_id == c.canonical_id => {
                if prev.description != c.description || prev.schema_version != c.schema_version {
                    return Err(anyhow::anyhow!(
                        "candidate id '{}' emitted twice with differing content —                          namespace collision between oracle and provider",
                        c.canonical_id
                    ));
                }
            }
            _ => deduped.push(c),
        }
    }
    let mut candidates = deduped;

    // Explicit abstention candidate on EVERY board (R2-r1), appended
    // after ordering so it is always last and always present.
    candidates.push(BoardCandidate::new(
        designer_graph::board_candidate::CandidateId::Abstain,
    ));

    let context = BoardContext {
        anchor: anchor.map(|(_, id)| id.to_owned()),
        graph_identity: graph_identity.map(str::to_owned),
        pack_identity: provider.pack_identity(),
    };

    // Content hash (I15/§11.7): candidates as supplied to the model, in
    // order, plus reachability context, graph identity, pack, and
    // policy-filter state. Injective by construction (review B1):
    // length-prefixed fields, distinct domain tags for None vs Some.
    let mut preimage: Vec<u8> = Vec::new();
    put(&mut preimage, "n", &candidates.len().to_string());
    for c in &candidates {
        put(&mut preimage, "cid", &c.canonical_id);
        put(&mut preimage, "cdesc", &c.description);
        put(&mut preimage, "cver", &c.schema_version.to_string());
    }
    match &context.anchor {
        Some(a) => put(&mut preimage, "anchor.some", a),
        None => put(&mut preimage, "anchor.none", ""),
    }
    match &context.graph_identity {
        Some(g) => put(&mut preimage, "graph.some", g),
        None => put(&mut preimage, "graph.none", ""),
    }
    put(&mut preimage, "pack", &context.pack_identity);
    put(&mut preimage, "ndenied", &policy.denied.len().to_string());
    for d in &policy.denied {
        put(&mut preimage, "deny", d);
    }
    let board_hash = blake3::hash(&preimage).to_hex().to_string();

    Ok(Board {
        candidates,
        context,
        board_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use designer_graph::board_candidate::{
        BoardCandidate, CandidateId, LegalityOracle, OperationKind, ProductionId,
    };

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

    fn root_board() -> Board {
        build_board(
            &AllLegal,
            None,
            None,
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap()
    }

    /// GREEN determinism + RED sensitivity: anchor, graph identity, and
    /// policy state all move the hash.
    #[test]
    fn board_hash_is_content_addressed() {
        let a = root_board();
        let b = root_board();
        assert_eq!(a.board_hash, b.board_hash, "determinism");

        let anchored = build_board(
            &AllLegal,
            Some((&(), "t1")),
            None,
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_ne!(a.board_hash, anchored.board_hash, "anchor is hashed");

        let with_graph = build_board(
            &AllLegal,
            None,
            Some("rev1"),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_ne!(
            a.board_hash, with_graph.board_hash,
            "graph identity is hashed"
        );

        let mut policy = PolicyFilter::default();
        policy.denied.insert("op.delete_subgraph".to_owned());
        let filtered = build_board(&AllLegal, None, None, &EmptyUniverse, &policy).unwrap();
        assert_ne!(a.board_hash, filtered.board_hash, "policy state is hashed");
    }

    /// B1 red fixtures: the collisions the pre-remediation scheme
    /// admitted must now hash differently.
    #[test]
    fn preimage_is_injective_against_crafted_collisions() {
        // Anchor sentinel: literal "<root>" id vs no anchor.
        let no_anchor = root_board();
        let sentinel = build_board(
            &AllLegal,
            Some((&(), "<root>")),
            None,
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_ne!(
            no_anchor.board_hash, sentinel.board_hash,
            "anchor id '<root>' must not collide with the no-anchor board"
        );

        // Delimiter forgery: two providers whose flattened byte streams
        // coincided under in-band delimiters.
        struct P(Vec<(String, String)>);
        impl BoardUniverseProvider for P {
            fn pack_identity(&self) -> String {
                "pack.test".into()
            }
            fn candidates(&self) -> Vec<BoardCandidate> {
                self.0
                    .iter()
                    .map(|(id, desc)| BoardCandidate {
                        id: CandidateId::Abstain, // discriminant unused in hashes
                        canonical_id: id.clone(),
                        description: desc.clone(),
                        schema_version: 1,
                    })
                    .collect()
            }
        }
        let a = build_board(
            &AllLegal,
            None,
            None,
            &P(vec![("verb.x".into(), "A\u{1f}B".into())]),
            &PolicyFilter::default(),
        )
        .unwrap();
        let b = build_board(
            &AllLegal,
            None,
            None,
            &P(vec![("verb.x\u{1f}A".into(), "B".into())]),
            &PolicyFilter::default(),
        )
        .unwrap();
        assert_ne!(
            a.board_hash, b.board_hash,
            "delimiter bytes cannot forge boundaries"
        );
    }

    /// C6 reds: reserved namespace and same-id/different-content are
    /// ERRORS, never silent dedup.
    #[test]
    fn provider_misbehavior_is_refused() {
        struct Abstainer;
        impl BoardUniverseProvider for Abstainer {
            fn pack_identity(&self) -> String {
                "pack.evil".into()
            }
            fn candidates(&self) -> Vec<BoardCandidate> {
                vec![BoardCandidate {
                    id: CandidateId::Abstain,
                    canonical_id: "abstain.none_of_the_above".into(),
                    description: "forged abstention".into(),
                    schema_version: 1,
                }]
            }
        }
        let err =
            build_board(&AllLegal, None, None, &Abstainer, &PolicyFilter::default()).unwrap_err();
        assert!(err.to_string().contains("reserved"));

        struct Collider;
        impl BoardUniverseProvider for Collider {
            fn pack_identity(&self) -> String {
                "pack.collide".into()
            }
            fn candidates(&self) -> Vec<BoardCandidate> {
                vec![BoardCandidate {
                    id: CandidateId::Operation(OperationKind::AppendNode),
                    canonical_id: "op.append_node".into(),
                    description: "a DIFFERENT description".into(),
                    schema_version: 1,
                }]
            }
        }
        let err =
            build_board(&AllLegal, None, None, &Collider, &PolicyFilter::default()).unwrap_err();
        assert!(err.to_string().contains("collision"));
    }

    /// D19: a denied candidate is REMOVED; NONE_OF_THE_ABOVE always
    /// present, always last.
    #[test]
    fn policy_filter_removes_and_abstention_is_always_present() {
        let mut policy = PolicyFilter::default();
        policy.denied.insert("op.delete_subgraph".to_owned());
        let board = build_board(&AllLegal, None, None, &EmptyUniverse, &policy).unwrap();
        assert!(!board.contains("op.delete_subgraph"));
        assert!(board.contains("op.append_node"));
        assert_eq!(
            board.candidates.last().unwrap().canonical_id,
            crate::contract::NONE_OF_THE_ABOVE
        );
        assert_eq!(board.candidates.len(), 21);
    }

    /// Canonical ordering regardless of emission order (reproducibility,
    /// R2-r2; position-invariance is a model test, §10.7).
    #[test]
    fn board_ordering_is_canonical() {
        let board = root_board();
        let ids: Vec<&str> = board
            .candidates
            .iter()
            .take(board.candidates.len() - 1)
            .map(|c| c.canonical_id.as_str())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }
}
