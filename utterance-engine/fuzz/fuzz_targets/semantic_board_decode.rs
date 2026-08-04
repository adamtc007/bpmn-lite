#![no_main]

use libfuzzer_sys::fuzz_target;
use sem_os_policy::decision_board::SemanticDecisionBoard;

const MAX_INPUT_BYTES: usize = 64 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    if let Ok(board) = serde_json::from_slice::<SemanticDecisionBoard>(data) {
        let encoded = serde_json::to_vec(&board).expect("accepted semantic board re-encodes");
        let decoded: SemanticDecisionBoard =
            serde_json::from_slice(&encoded).expect("self-produced semantic board decodes");
        assert_eq!(decoded, board);
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    }
});
