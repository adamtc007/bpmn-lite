//! Property cements for the deterministic mapper contracts.

use proptest::prelude::*;
use sem_os_ontology::verb_contract::{ActionClass, HarmClass};
use sem_os_policy::decision_board::{
    ArgumentKind, BoardHash, CandidateSemanticSlice, CanonicalCandidateId, DomainIdentity,
    EvidenceRecordHash, GraphRevision, PhraseEvidence, PhraseRole, ProposalStatus,
    ProposalWorkbook, ResolvedPosition, SlotRequirement, SlotValueState, SnapshotIdentity,
    WorkbookId, WorkbookSlot,
};

use crate::contract::{rank_canonically, FiniteScore, RankedCandidate};
use crate::exact::{governed_exact, ExactMatch};

fn candidate(id: &str, phrase: &str) -> CandidateSemanticSlice {
    CandidateSemanticSlice {
        canonical_id: CanonicalCandidateId::new(id).unwrap(),
        schema_version: 1,
        title: id.to_string(),
        intent_summary: format!("intent {id}"),
        action_class: ActionClass::Create,
        applicability: "when position legal".into(),
        effect: "changes the graph".into(),
        arguments: Vec::new(),
        phrases: vec![PhraseEvidence {
            text: phrase.to_string(),
            locale: "en-GB".into(),
            role: PhraseRole::Canonical,
            provenance: "property-test".into(),
        }],
        positive_examples: vec![format!("example {id}")],
        negative_contrasts: Vec::new(),
        risk: HarmClass::Reversible,
        adapter_payload_hash: format!("payload-{id}"),
    }
}

fn board(
    candidates: Vec<CandidateSemanticSlice>,
) -> sem_os_policy::decision_board::SemanticDecisionBoard {
    sem_os_policy::decision_board::SemanticDecisionBoard::new(
        1,
        DomainIdentity::new("bpmn.property").unwrap(),
        SnapshotIdentity::new("snapshot-property").unwrap(),
        GraphRevision::new("revision-property").unwrap(),
        ResolvedPosition {
            anchor: None,
            context_hash: "context-property".into(),
        },
        candidates,
        "policy-property".into(),
    )
    .unwrap()
}

fn workbook(needs_argument: bool) -> ProposalWorkbook {
    let slots = needs_argument
        .then(|| WorkbookSlot {
            name: "name".into(),
            kind: ArgumentKind::Identifier,
            requirement: SlotRequirement::Required,
            value: SlotValueState::Missing,
            provenance: None,
            clarification_prompt: "Which name?".into(),
        })
        .into_iter()
        .collect();
    ProposalWorkbook::new(
        1,
        WorkbookId::new("workbook-property").unwrap(),
        1,
        BoardHash::new("b".repeat(64)).unwrap(),
        GraphRevision::new("revision-property").unwrap(),
        CanonicalCandidateId::new("op.append_node").unwrap(),
        slots,
        EvidenceRecordHash::new("e".repeat(64)).unwrap(),
    )
    .unwrap()
}

fn statuses() -> [ProposalStatus; 7] {
    [
        ProposalStatus::NeedsArguments,
        ProposalStatus::ReadyForDryRun,
        ProposalStatus::DryRunRefused,
        ProposalStatus::ReadyForRatification,
        ProposalStatus::Ratified,
        ProposalStatus::Rejected,
        ProposalStatus::Expired,
    ]
}

proptest! {
    #[test]
    fn canonical_context_encoding_is_injective_for_admitted_strings(
        pack_a in "[ -~]{0,40}",
        graph_a in "[ -~]{0,40}",
        pack_b in "[ -~]{0,40}",
        graph_b in "[ -~]{0,40}",
    ) {
        prop_assume!((pack_a.as_str(), graph_a.as_str()) != (pack_b.as_str(), graph_b.as_str()));
        let first = crate::context::ContextProjection::new(pack_a, graph_a, None, vec![]).unwrap();
        let second = crate::context::ContextProjection::new(pack_b, graph_b, None, vec![]).unwrap();
        prop_assert_ne!(first.serialize_canonical(), second.serialize_canonical());
        prop_assert_ne!(first.hash(), second.hash());
    }

    #[test]
    fn semantic_board_hash_is_permutation_invariant(
        left in "[a-z][a-z0-9_]{0,10}",
        right in "[a-z][a-z0-9_]{0,10}",
    ) {
        prop_assume!(left != right);
        let left_id = format!("op.{left}");
        let right_id = format!("op.{right}");
        let forward = board(vec![candidate(&left_id, &left), candidate(&right_id, &right)]);
        let reverse = board(vec![candidate(&right_id, &right), candidate(&left_id, &left)]);
        prop_assert_eq!(forward, reverse);
    }

    #[test]
    fn phrase_collisions_are_canonical_and_never_choose_one(
        phrase in "[ -~]{0,256}",
    ) {
        let phrase = if crate::exact::normalize_phrase(&phrase).is_empty() {
            "governed phrase".to_string()
        } else {
            phrase
        };
        let semantic = board(vec![candidate("op.zed", &phrase), candidate("op.alpha", &phrase)]);
        prop_assert_eq!(
            governed_exact(&semantic, &phrase),
            ExactMatch::Collision(vec!["op.alpha".into(), "op.zed".into()])
        );
    }

    #[test]
    fn arbitrary_finite_scores_have_a_total_canonical_order(
        raw in prop::collection::vec(any::<f64>().prop_filter("finite", |value| value.is_finite()), 0..64),
    ) {
        let mut ranking = raw
            .iter()
            .enumerate()
            .map(|(index, value)| RankedCandidate {
                candidate_id: format!("candidate-{index:02}"),
                score: FiniteScore::new(*value).unwrap(),
            })
            .collect::<Vec<_>>();
        rank_canonically(&mut ranking);
        let canonically_ordered = ranking.windows(2).all(|pair| {
            pair[0].score.get() > pair[1].score.get()
                || (pair[0].score == pair[1].score
                    && pair[0].candidate_id <= pair[1].candidate_id)
        });
        prop_assert!(canonically_ordered);
    }

    #[test]
    fn workbook_transitions_never_mutate_on_refusal(
        starts_needing_argument in any::<bool>(),
        targets in prop::collection::vec(0usize..7, 0..32),
    ) {
        let mut workbook = workbook(starts_needing_argument);
        for target in targets {
            let before = workbook.clone();
            let result = workbook.transition(statuses()[target]);
            if result.is_err() {
                prop_assert_eq!(&workbook, &before);
            }
            let encoded = serde_json::to_vec(&workbook).unwrap();
            let decoded: ProposalWorkbook = serde_json::from_slice(&encoded).unwrap();
            prop_assert_eq!(&decoded, &workbook);
        }
    }

    #[test]
    fn arbitrary_text_never_panics_exact_or_pair_serialization(text in ".{0,4096}") {
        let semantic = board(vec![candidate("op.one", "governed phrase")]);
        let _ = governed_exact(&semantic, &text);
        let context = crate::context::minimal("pack.property", "revision-property");
        let pair = crate::pair::serialize_candidate_pair(
            &text,
            &context,
            &semantic.candidates[0],
            crate::pair::PairTokenBudget::default(),
        )
        .unwrap();
        prop_assert!(pair.side_a.split_whitespace().count() <= 128);
        prop_assert!(pair.side_b.split_whitespace().count() <= 128);
    }
}
