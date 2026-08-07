#![no_main]

use candle_core::Device;
use libfuzzer_sys::fuzz_target;
use utterance_engine::trained_ranker::{Base, TrainedRanker};

const MAX_INPUT_BYTES: usize = 1024;

#[derive(Clone, Copy)]
enum RefusedRoute {
    LegacyCandidateText,
    LegacyPairText,
    UnsealedPairIdentity,
}

impl RefusedRoute {
    fn from_tape(data: &[u8]) -> Self {
        match data.first().copied().unwrap_or_default() % 3 {
            0 => Self::LegacyCandidateText,
            1 => Self::LegacyPairText,
            _ => Self::UnsealedPairIdentity,
        }
    }

    fn expected_diagnostic(self) -> &'static str {
        match self {
            Self::LegacyCandidateText => "candidate_serializer_id mismatch",
            Self::LegacyPairText => "pair_serializer_id mismatch",
            Self::UnsealedPairIdentity => "pair_serializer_hash mismatch",
        }
    }
}

fn admitted_prefix() -> serde_json::Value {
    let budget = utterance_engine::pair::PairTokenBudget::default();
    serde_json::json!({
        "corpus_schema_id": utterance_engine::corpus_schema::CORPUS_SCHEMA_ID,
        "corpus_schema_hash": utterance_engine::corpus_schema::corpus_schema_hash(),
        "semantic_board_schema_version": 1,
        "semantic_pack_identity": utterance_engine::bpmn_board::bpmn_semantic_snapshot_identity(),
        "turn_serializer_id": utterance_engine::pair::TURN_SERIALIZER_ID,
        "turn_serializer_hash": utterance_engine::pair::turn_serializer_hash(),
        "candidate_serializer_id": utterance_engine::exact::CANDIDATE_SERIALIZER_ID,
        "candidate_serializer_hash": utterance_engine::exact::candidate_serializer_hash(),
        "pair_serializer_id": utterance_engine::pair::PAIR_SERIALIZER_ID,
        "pair_serializer_hash": utterance_engine::pair::pair_serializer_hash(),
        "max_length": 256,
        "side_a_tokens": budget.side_a_tokens,
        "side_b_tokens": budget.side_b_tokens,
    })
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let route = RefusedRoute::from_tape(data);
    let hostile_suffix = String::from_utf8_lossy(data);
    let mut card = admitted_prefix();
    match route {
        RefusedRoute::LegacyCandidateText => {
            card["candidate_serializer_id"] =
                format!("legacy.description.v2:{hostile_suffix}").into();
        }
        RefusedRoute::LegacyPairText => {
            card["pair_serializer_id"] =
                format!("legacy.query-candidate.v2:{hostile_suffix}").into();
        }
        RefusedRoute::UnsealedPairIdentity => {
            card["pair_serializer_hash"] = format!("unsealed:{hostile_suffix}").into();
        }
    }

    // The target deliberately supplies a route-invalid card, so admission
    // must refuse before tokenizer/weights reads, model construction, HF
    // cache/network access, or scoring. The directory identity is explicit
    // and process-local; hostile bytes affect only the bounded JSON value.
    let dir = std::env::temp_dir().join(format!(
        "bpmn-lite-v3-route-admission-fuzz-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create process-local fuzz scratch directory");
    std::fs::write(
        dir.join("training_card.json"),
        serde_json::to_vec(&card).expect("card encodes"),
    )
    .expect("write process-local fuzz card");

    let error = match TrainedRanker::load(Base::ModernbertBase, &dir, &Device::Cpu) {
        Ok(_) => panic!("route-invalid semantic-v3 bundle was admitted"),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains(route.expected_diagnostic()),
        "route refusal drifted or occurred after the intended admission boundary: {error:#}"
    );
});
