#![no_main]

use libfuzzer_sys::fuzz_target;

#[allow(dead_code)]
#[path = "../../src/proposal.rs"]
mod proposal;

const MAX_INPUT_BYTES: usize = 16 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }
    let text = String::from_utf8_lossy(data);
    let quoted = proposal::quoted_names(&text);
    assert!(quoted.len() <= text.chars().count());
    let durations = proposal::durations(&text);
    assert!(durations.len() <= text.split_whitespace().count());
    let _ = proposal::followed_count(&text, &["times", "branches", "items", "retries"]);
    let _ = proposal::parse_condition(&text);
    let sanitized = proposal::sanitize_identifier(&text);
    assert!(sanitized
        .chars()
        .all(|character| character.is_alphanumeric() || character == '_'));
});
