#![no_main]

use libfuzzer_sys::fuzz_target;
use designer_graph::schema::DesignerDag;
use utterance_engine::board::PolicyFilter;
use utterance_engine::retrieval::Tier0Retriever as _;

const MAX_INPUT_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let dag = DesignerDag::new("fuzz-public-facade");
    let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag,
        None,
        &"a".repeat(64),
        &PolicyFilter::default(),
    )
    .expect("empty public board is admitted");
    let result = utterance_engine::retrieval::LexicalTier0
        .retrieve(&text, &board)
        .expect("public lexical retrieval is total over an admitted board");
    assert_eq!(result.ranking.len(), board.candidates.len());
});
