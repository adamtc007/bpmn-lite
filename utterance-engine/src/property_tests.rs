//! Property cements for the deterministic mapper contracts.

use proptest::prelude::*;
use semantic_decision_contracts::{
    ActionClass, ArgumentKind, BoardHash, CandidateSemanticSlice, CanonicalCandidateId,
    DomainIdentity, EvidenceRecordHash, GraphRevision, HarmClass, PhraseEvidence, PhraseRole,
    ProposalStatus, ProposalWorkbook, ResolvedPosition, SlotRequirement, SlotValueState,
    SnapshotIdentity, WorkbookId, WorkbookSlot,
};

use crate::contract::{rank_canonically, FiniteScore, RankedCandidate};
use crate::exact::{governed_exact, ExactMatch};

mod gameboard {
    //! Property cements for the design-game model (Phase 8, EOP-PLAN-BPMN-
    //! GAMEBOARD-001 §14). Each case grounds directly in `DesignPosition::
    //! new`'s own documented field lists (`move_set_hash` over graph_revision
    //! / semantic_snapshot / focus / policy / compiler_profile / move ids;
    //! `state_id` additionally over history_hash) rather than re-deriving
    //! them - the goal is to catch a future accidental change to those field
    //! lists, not to reimplement the hashing.

    use proptest::prelude::*;

    use bpmn_lite_compiler::IRNode;
    use designer_graph::ops::{apply, Operation};
    use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
    use semantic_decision_contracts::{
        ApplicabilityState, DesignFocus, FeedbackOptionKind, FiniteScore as GameboardFiniteScore,
        GraphElementRef, MoveAttemptId, MoveEvidence, ProducerIdentity, GAMEBOARD_SCHEMA_VERSION,
    };
    use uuid::Uuid;

    use crate::board::PolicyFilter;
    use crate::bpmn_board::{
        build_bpmn_design_position, build_bpmn_semantic_board, decide_bpmn_game_disposition,
        explain_bpmn_candidate,
    };

    fn anchored_task() -> (DesignerDag, NodeKey) {
        let start = NodeKey(Uuid::from_u128(1));
        let task = NodeKey(Uuid::from_u128(2));
        let mut dag = DesignerDag::new("property-fuzz");
        dag.seed(
            start,
            IRNode::Start { id: "start".into() },
            Provenance::default(),
        )
        .unwrap();
        dag = apply(
            &dag,
            Operation::AppendNode {
                anchor: start,
                key: task,
                node: IRNode::ServiceTask {
                    id: "task-1".into(),
                    name: "Review".into(),
                    task_type: "review".into(),
                    loop_origin: None,
                },
                edge_id: "flow-1".into(),
            },
            Provenance::default(),
        )
        .unwrap()
        .candidate;
        (dag, task)
    }

    const ANCHOR_CANDIDATES: [&str; 13] = [
        "op.attach_guard",
        "op.attach_rearming_guard",
        "op.connect",
        "op.create_inclusive_region",
        "op.create_multi_instance_region",
        "op.create_parallel_region",
        "op.insert_after",
        "op.insert_before",
        "op.replace_node",
        "prod.interrupting_timeout",
        "prod.non_interrupting_notification",
        "prod.reminder_then_escalate",
        "prod.request_and_wait",
    ];

    fn policy_from_mask(mask: u16) -> PolicyFilter {
        let mut filter = PolicyFilter::default();
        for (index, candidate) in ANCHOR_CANDIDATES.iter().enumerate() {
            if mask & (1 << index) != 0 {
                filter.denied.insert((*candidate).to_string());
            }
        }
        filter
    }

    proptest! {
        #[test]
        fn legal_move_set_is_deterministic_and_canonically_ordered_by_move_id(
            revision_suffix in "[a-f0-9]{4}",
        ) {
            let (dag, task) = anchored_task();
            let revision = format!("{}{}", "a".repeat(60), revision_suffix);
            let board = build_bpmn_semantic_board(
                &dag,
                Some((task, "task-1")),
                &revision,
                &PolicyFilter::default(),
            )
            .unwrap();
            let build = || {
                build_bpmn_design_position(
                    &dag,
                    &board,
                    &revision,
                    &"b".repeat(64),
                    "property-compiler-v1",
                    &"c".repeat(64),
                    DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
                    None,
                )
                .unwrap()
            };
            let first = build();
            let second = build();
            prop_assert_eq!(&first, &second);

            let ids = first
                .legal_moves()
                .iter()
                .map(|legal_move| legal_move.move_id().as_str().to_string())
                .collect::<Vec<_>>();
            let mut sorted = ids.clone();
            sorted.sort();
            prop_assert_eq!(ids, sorted);
        }

        #[test]
        fn move_set_hash_is_sensitive_to_focus_policy_revision_and_profile_drift(
            mask_a in 0u16..(1 << ANCHOR_CANDIDATES.len()),
            mask_b in 0u16..(1 << ANCHOR_CANDIDATES.len()),
            profile_a in "[a-z]{3,8}",
            profile_b in "[a-z]{3,8}",
        ) {
            prop_assume!(mask_a != mask_b);
            prop_assume!(profile_a != profile_b);
            let (dag, task) = anchored_task();
            let revision_a = "a".repeat(64);
            let revision_b = "b".repeat(64);

            let position = |revision: &str, mask: u16, profile: &str, focus_id: &str| {
                let filter = policy_from_mask(mask);
                let board =
                    build_bpmn_semantic_board(&dag, Some((task, "task-1")), revision, &filter)
                        .unwrap();
                build_bpmn_design_position(
                    &dag,
                    &board,
                    revision,
                    &"b".repeat(64),
                    profile,
                    &"c".repeat(64),
                    DesignFocus::element(GraphElementRef::new(focus_id).unwrap()),
                    None,
                )
                .unwrap()
            };

            let base = position(&revision_a, mask_a, &profile_a, "task-1");

            // Policy drift alone (revision/profile/focus held fixed).
            let policy_drift = position(&revision_a, mask_b, &profile_a, "task-1");
            prop_assert_ne!(base.move_set_hash(), policy_drift.move_set_hash());

            // Compiler-profile drift alone.
            let profile_drift = position(&revision_a, mask_a, &profile_b, "task-1");
            prop_assert_ne!(base.move_set_hash(), profile_drift.move_set_hash());

            // Graph-revision drift alone.
            let revision_drift = position(&revision_b, mask_a, &profile_a, "task-1");
            prop_assert_ne!(base.move_set_hash(), revision_drift.move_set_hash());

            // Focus drift alone: changes move_set_hash but never the legal
            // moves themselves, since the board's own candidate set (not
            // `DesignFocus`) determines `legal_moves()`.
            let focus_drift = position(&revision_a, mask_a, &profile_a, "start");
            prop_assert_ne!(base.move_set_hash(), focus_drift.move_set_hash());
            prop_assert_eq!(base.legal_moves(), focus_drift.legal_moves());
        }

        #[test]
        fn history_hash_never_changes_legal_moves_or_move_set_hash(
            history_a in "[a-f0-9]{64}",
            history_b in "[a-f0-9]{64}",
        ) {
            prop_assume!(history_a != history_b);
            let (dag, task) = anchored_task();
            let revision = "a".repeat(64);
            let board = build_bpmn_semantic_board(
                &dag,
                Some((task, "task-1")),
                &revision,
                &PolicyFilter::default(),
            )
            .unwrap();
            let position = |history: &str| {
                build_bpmn_design_position(
                    &dag,
                    &board,
                    &revision,
                    &"b".repeat(64),
                    "property-compiler-v1",
                    history,
                    DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
                    None,
                )
                .unwrap()
            };
            let first = position(&history_a);
            let second = position(&history_b);
            // The position's own content identity still differs (history is
            // part of `state_id`) ...
            prop_assert_ne!(first.state_id(), second.state_id());
            // ... but legality itself - what the palette actually offers -
            // must never be a function of retained history/belief.
            prop_assert_eq!(first.legal_moves(), second.legal_moves());
            prop_assert_eq!(first.move_set_hash(), second.move_set_hash());
        }

        #[test]
        fn off_board_duplicate_or_incomplete_evidence_is_always_refused(
            mutation in 0u8..3,
            fabricated_suffix in "[a-f0-9]{8}",
        ) {
            let (dag, task) = anchored_task();
            let revision = "a".repeat(64);
            let board = build_bpmn_semantic_board(
                &dag,
                Some((task, "task-1")),
                &revision,
                &PolicyFilter::default(),
            )
            .unwrap();
            let position = build_bpmn_design_position(
                &dag,
                &board,
                &revision,
                &"b".repeat(64),
                "property-compiler-v1",
                &"c".repeat(64),
                DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
                None,
            )
            .unwrap();
            let belief = semantic_decision_contracts::DesignBelief::new(
                GAMEBOARD_SCHEMA_VERSION,
                position.state_id().clone(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                ProducerIdentity::new("property.empty-belief.v1").unwrap(),
            )
            .unwrap();
            let evidence_for = |move_id: &semantic_decision_contracts::LegalMoveId| {
                MoveEvidence::new(
                    GAMEBOARD_SCHEMA_VERSION,
                    move_id.clone(),
                    Vec::new(),
                    GameboardFiniteScore::new(0.1).unwrap(),
                    GameboardFiniteScore::new(0.1).unwrap(),
                    Vec::new(),
                    ProducerIdentity::new("property.evidence.v1").unwrap(),
                )
                .unwrap()
            };
            let mut evidence = position
                .legal_moves()
                .iter()
                .map(|legal_move| evidence_for(legal_move.move_id()))
                .collect::<Vec<_>>();
            match mutation {
                0 => {
                    // Incomplete: drop one entry.
                    evidence.pop();
                }
                1 => {
                    // Duplicate: repeat the first entry, still missing another.
                    let first = evidence[0].clone();
                    evidence.pop();
                    evidence.push(first);
                }
                _ => {
                    // Off-board: append evidence for a move never on this
                    // position, on top of an otherwise-complete set.
                    let off_board = semantic_decision_contracts::LegalMoveId::new(
                        "0".repeat(56) + &fabricated_suffix,
                    )
                    .unwrap();
                    evidence.push(evidence_for(&off_board));
                }
            }
            let result = decide_bpmn_game_disposition(
                &board,
                &position,
                &evidence,
                &belief,
                "property fuzz utterance",
                MoveAttemptId::new("attempt-property-off-board").unwrap(),
                &[],
                None,
            );
            prop_assert!(result.is_err());
        }

        #[test]
        fn feedback_recoveries_resolve_to_legal_moves_or_governed_focus_change(
            mask in 0u16..(1 << ANCHOR_CANDIDATES.len()),
            candidate_index in 0usize..ANCHOR_CANDIDATES.len(),
        ) {
            let (dag, task) = anchored_task();
            let revision = "a".repeat(64);
            let filter = policy_from_mask(mask);
            let board =
                build_bpmn_semantic_board(&dag, Some((task, "task-1")), &revision, &filter)
                    .unwrap();
            let position = build_bpmn_design_position(
                &dag,
                &board,
                &revision,
                &"b".repeat(64),
                "property-compiler-v1",
                &"c".repeat(64),
                DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
                None,
            )
            .unwrap();
            let guidance = explain_bpmn_candidate(
                &board,
                &position,
                ANCHOR_CANDIDATES[candidate_index],
                &filter,
            )
            .unwrap();
            for recovery in guidance.recoveries() {
                match recovery.move_id() {
                    Some(move_id) => prop_assert!(position
                        .legal_moves()
                        .iter()
                        .any(|legal_move| legal_move.move_id() == move_id)),
                    None => prop_assert_eq!(recovery.kind(), FeedbackOptionKind::ChangeFocus),
                }
            }
        }

        #[test]
        fn policy_hidden_explanation_never_names_the_hidden_candidate(
            mask in 0u16..(1 << ANCHOR_CANDIDATES.len()),
            candidate_index in 0usize..ANCHOR_CANDIDATES.len(),
        ) {
            let (dag, task) = anchored_task();
            let revision = "a".repeat(64);
            let candidate_id = ANCHOR_CANDIDATES[candidate_index];
            // Force this candidate hidden regardless of the fuzzed mask, but
            // still let the mask vary which *other* candidates are denied -
            // the leak-freedom guarantee must hold no matter what else is
            // hidden alongside it.
            let mut filter = policy_from_mask(mask);
            filter.denied.insert(candidate_id.to_string());
            let board =
                build_bpmn_semantic_board(&dag, Some((task, "task-1")), &revision, &filter)
                    .unwrap();
            let position = build_bpmn_design_position(
                &dag,
                &board,
                &revision,
                &"b".repeat(64),
                "property-compiler-v1",
                &"c".repeat(64),
                DesignFocus::element(GraphElementRef::new("task-1").unwrap()),
                None,
            )
            .unwrap();
            let guidance =
                explain_bpmn_candidate(&board, &position, candidate_id, &filter).unwrap();
            prop_assert_eq!(guidance.applicability(), ApplicabilityState::PolicyHidden);
            prop_assert!(guidance.explanation().parameters().is_empty());
            let rendered =
                serde_json::to_string(&(guidance.explanation(), guidance.recoveries())).unwrap();
            prop_assert!(!rendered.contains(candidate_id));
        }
    }
}

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
) -> semantic_decision_contracts::SemanticDecisionBoard {
    semantic_decision_contracts::SemanticDecisionBoard::new(
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
