#![no_main]

use libfuzzer_sys::fuzz_target;
use utterance_engine::board::PolicyFilter;
use utterance_engine::exact::{governed_exact, ExactMatch};

const MAX_INPUT_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let utterance = String::from_utf8_lossy(data);
    let state = utterance_engine::fixtures::enumeration_classes()
        .expect("fixed enumeration fixtures")
        .into_iter()
        .find(|state| state.class_id == "mid_sequence_task")
        .expect("mid-sequence fixture");
    let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &state.dag,
        state.anchor_key.zip(state.anchor_id),
        "fuzz-phrase-index",
        &PolicyFilter::default(),
    )
    .expect("fixed semantic board");
    if let ExactMatch::Collision(ids) = governed_exact(&board, &utterance) {
        assert!(ids.len() >= 2);
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(ids.iter().all(|id| board
            .candidates
            .iter()
            .any(|candidate| candidate.canonical_id.as_str() == id)));
    }
});
