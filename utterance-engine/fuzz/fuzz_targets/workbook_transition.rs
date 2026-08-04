#![no_main]

use libfuzzer_sys::fuzz_target;
use sem_os_policy::decision_board::{
    BoardHash, CanonicalCandidateId, EvidenceRecordHash, GraphRevision, ProposalStatus,
    ProposalWorkbook, WorkbookId,
};

const MAX_INPUT_BYTES: usize = 4 * 1024;

fn status(byte: u8) -> ProposalStatus {
    match byte % 7 {
        0 => ProposalStatus::NeedsArguments,
        1 => ProposalStatus::ReadyForDryRun,
        2 => ProposalStatus::DryRunRefused,
        3 => ProposalStatus::ReadyForRatification,
        4 => ProposalStatus::Ratified,
        5 => ProposalStatus::Rejected,
        _ => ProposalStatus::Expired,
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let mut workbook = ProposalWorkbook::new(
        1,
        WorkbookId::new("fuzz-workbook").unwrap(),
        1,
        BoardHash::new("b".repeat(64)).unwrap(),
        GraphRevision::new("fuzz-revision").unwrap(),
        CanonicalCandidateId::new("op.append_node").unwrap(),
        Vec::new(),
        EvidenceRecordHash::new("e".repeat(64)).unwrap(),
    )
    .unwrap();
    for byte in data {
        let before = workbook.clone();
        if workbook.transition(status(*byte)).is_err() {
            assert_eq!(workbook, before);
        }
        let encoded = serde_json::to_vec(&workbook).unwrap();
        let decoded: ProposalWorkbook = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, workbook);
    }
});
