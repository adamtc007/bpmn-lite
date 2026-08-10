#![no_main]

use std::sync::atomic::{AtomicU16, Ordering};

use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    BoardPath, CanonicalCandidateId, DesignFocus, FocusAbsenceReason, GameDomainId,
    GraphContentHash, GraphRevision, HistoryHash, LegalMove, SnapshotIdentity,
    GAMEBOARD_SCHEMA_VERSION,
};
use utterance_engine::{
    BoundedOfflineRankerRequest, ResolverBoardPacket, ResolverBoundaryRefusal, ResolverVariant,
    MAX_OFFLINE_CANDIDATE_BYTES, MAX_OFFLINE_PROMPT_BYTES, MAX_OFFLINE_TOKEN_BUDGET,
};

static SHAPES: AtomicU16 = AtomicU16::new(0);

fn observe(index: u8, label: &str) {
    let bit = 1_u16 << index;
    if SHAPES.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        eprintln!("semantic-counter model_boundary={label}");
    }
}

fn position(move_count: usize) -> semantic_decision_contracts::DesignPosition {
    let revision = GraphRevision::new("model-boundary-revision-1").unwrap();
    let moves = (0..move_count)
        .map(|index| {
            LegalMove::new(
                GAMEBOARD_SCHEMA_VERSION,
                CanonicalCandidateId::new(format!("operation.candidate_{index}")).unwrap(),
                revision.clone(),
                false,
                None,
                Vec::new(),
                Vec::new(),
                None,
            )
            .unwrap()
        })
        .collect();
    semantic_decision_contracts::DesignPosition::new(
        GAMEBOARD_SCHEMA_VERSION,
        GameDomainId::new("domain.model-boundary").unwrap(),
        BoardPath::new(vec!["root".into(), "model-boundary".into()]).unwrap(),
        SnapshotIdentity::new("snapshot-model-boundary-v1").unwrap(),
        revision,
        GraphContentHash::new("a".repeat(64)).unwrap(),
        "compiler-model-boundary-v1",
        "policy-model-boundary-v1",
        None,
        DesignFocus::absent(FocusAbsenceReason::NotProvided),
        HistoryHash::new("b".repeat(64)).unwrap(),
        moves,
    )
    .unwrap()
}

fuzz_target!(|data: &[u8]| {
    let move_count = usize::from(data.first().copied().unwrap_or(1) % 8) + 1;
    let position = position(move_count);
    let ids = position
        .legal_moves()
        .iter()
        .map(|legal_move| legal_move.move_id().clone())
        .collect::<Vec<_>>();
    let unicode_bytes = data.get(3..).unwrap_or_default();
    let unicode =
        String::from_utf8_lossy(&unicode_bytes[..unicode_bytes.len().min(128)]).into_owned();
    let base_text = if unicode.trim().is_empty() {
        "候補 🧭".to_string()
    } else {
        format!("候補 🧭 {unicode}")
    };
    let mut candidates = ids
        .iter()
        .enumerate()
        .map(|(index, move_id)| {
            (
                move_id.clone(),
                format!("{base_text} {index}"),
                usize::from(data.get(index + 1).copied().unwrap_or(1) % 8) + 1,
            )
        })
        .collect::<Vec<_>>();
    let bundle = GraphContentHash::new("c".repeat(64)).unwrap();
    let mut card = bundle.clone();
    let mut prompt = format!("意図 🧭 {unicode}");
    let mut prompt_tokens = 1_usize;
    let mut token_budget = MAX_OFFLINE_TOKEN_BUDGET;
    let mode = data.get(1).copied().unwrap_or_default() % 9;
    let expected = match mode {
        0 => {
            observe(0, "valid_unicode_full_board");
            None
        }
        1 => {
            observe(1, "empty_candidate");
            candidates[0].1.clear();
            candidates[0].2 = 0;
            Some(ResolverBoundaryRefusal::EmptyCandidateText)
        }
        2 => {
            observe(2, "oversized_candidate");
            candidates[0].1 = "x".repeat(MAX_OFFLINE_CANDIDATE_BYTES + 1);
            Some(ResolverBoundaryRefusal::OversizedCandidateText)
        }
        3 => {
            observe(3, "token_budget");
            prompt_tokens = MAX_OFFLINE_TOKEN_BUDGET + 1;
            Some(ResolverBoundaryRefusal::TokenBudgetExceeded)
        }
        4 => {
            observe(4, "bundle_card_mismatch");
            card = GraphContentHash::new("d".repeat(64)).unwrap();
            Some(ResolverBoundaryRefusal::BundleCardMismatch)
        }
        5 => {
            observe(5, "incomplete_board");
            candidates.pop();
            Some(ResolverBoundaryRefusal::IncompleteBoard)
        }
        6 => {
            observe(6, "duplicate_move");
            if candidates.len() == 1 {
                candidates.push(candidates[0].clone());
            } else {
                candidates[1].0 = candidates[0].0.clone();
            }
            Some(ResolverBoundaryRefusal::DuplicateMove)
        }
        7 => {
            observe(7, "oversized_prompt");
            prompt = "p".repeat(MAX_OFFLINE_PROMPT_BYTES + 1);
            Some(ResolverBoundaryRefusal::OversizedPromptText)
        }
        _ => {
            observe(8, "declared_budget_bound");
            token_budget = MAX_OFFLINE_TOKEN_BUDGET + 1;
            Some(ResolverBoundaryRefusal::TokenBudgetExceeded)
        }
    };
    let request = BoundedOfflineRankerRequest::new(
        &position,
        prompt,
        prompt_tokens,
        candidates,
        token_budget,
        bundle,
        card,
    );
    if let Some(expected) = expected {
        assert_eq!(request, Err(expected));
    } else {
        let request = request.unwrap();
        assert_eq!(request.candidates().len(), position.legal_moves().len());
        assert_eq!(request.position_id(), position.state_id());
        assert_eq!(request.move_set_hash(), position.move_set_hash());
    }

    let mut scores = ids
        .iter()
        .enumerate()
        .map(|(index, move_id)| {
            (
                move_id.clone(),
                f64::from(data.get(index + 2).copied().unwrap_or_default()) / 255.0,
            )
        })
        .collect::<Vec<_>>();
    let admitted =
        ResolverBoardPacket::ranked(&position, ResolverVariant::StructuredChoice, scores.clone())
            .unwrap();
    scores.reverse();
    let replay =
        ResolverBoardPacket::ranked(&position, ResolverVariant::StructuredChoice, scores).unwrap();
    assert_eq!(admitted, replay);

    let mut non_finite = ids
        .iter()
        .map(|move_id| (move_id.clone(), 0.0))
        .collect::<Vec<_>>();
    non_finite[0].1 = if data.get(2).copied().unwrap_or_default() & 1 == 0 {
        f64::NAN
    } else {
        f64::INFINITY
    };
    assert_eq!(
        ResolverBoardPacket::ranked(&position, ResolverVariant::BoundedPromptOffline, non_finite,),
        Err(ResolverBoundaryRefusal::NonFiniteScore)
    );
    let refused = ResolverBoardPacket::refused(
        &position,
        ResolverVariant::BoundedPromptOffline,
        ResolverBoundaryRefusal::ModelRefusal,
    )
    .unwrap();
    assert!(refused.scores().is_empty());
    assert_eq!(
        refused.refusal(),
        Some(ResolverBoundaryRefusal::ModelRefusal)
    );
});
