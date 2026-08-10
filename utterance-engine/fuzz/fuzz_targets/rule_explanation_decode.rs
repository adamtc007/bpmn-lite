#![no_main]

use libfuzzer_sys::fuzz_target;
use semantic_decision_contracts::{
    filter_rule_explanations, DisclosureClass, ExplanationParameter, MessageKey, RuleCode,
    RuleExplanation, GAMEBOARD_SCHEMA_VERSION,
};

const MAX_INPUT_BYTES: usize = 8 * 1024;

fn disclosure(selector: u8) -> DisclosureClass {
    match selector % 5 {
        0 => DisclosureClass::Public,
        1 => DisclosureClass::Authenticated,
        2 => DisclosureClass::Restricted,
        3 => DisclosureClass::PolicyHidden,
        _ => DisclosureClass::Technical,
    }
}

fn all_disclosures() -> [DisclosureClass; 5] {
    [
        DisclosureClass::Public,
        DisclosureClass::Authenticated,
        DisclosureClass::Restricted,
        DisclosureClass::PolicyHidden,
        DisclosureClass::Technical,
    ]
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT_BYTES {
        return;
    }

    // 1. Raw hostile-byte decode: arbitrary bytes must never panic the
    // decoder, and anything it does admit must round-trip losslessly.
    if let Ok(explanation) = serde_json::from_slice::<RuleExplanation>(data) {
        let encoded =
            serde_json::to_vec(&explanation).expect("accepted rule explanation re-encodes");
        let decoded: RuleExplanation =
            serde_json::from_slice(&encoded).expect("self-produced rule explanation decodes");
        assert_eq!(decoded, explanation);
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), encoded);
    }

    if data.len() < 2 {
        return;
    }

    // 2. Hostile field-level construction: fuzzed rule code / message key /
    // provenance / parameters (including deliberately duplicated parameter
    // names) and every disclosure class.
    let selected_disclosure = disclosure(data[0]);
    let param_count = (data[1] % 4) as usize;
    let mut chunks = data[2..]
        .split(|byte| *byte == 0)
        .map(String::from_utf8_lossy)
        .map(|value| value.into_owned());
    let mut next = || chunks.next().unwrap_or_default();

    let rule_code_text = next();
    let message_key_text = next();
    let provenance_text = next();
    let mut parameters = Vec::new();
    let mut duplicate_names = false;
    for index in 0..param_count {
        let name = next();
        let value = next();
        if index > 0 && parameters.last().is_some_and(|(n, _): &(String, String)| n == &name) {
            duplicate_names = true;
        }
        parameters.push((name, value));
    }

    let rule_code = RuleCode::new(rule_code_text.clone());
    let message_key = MessageKey::new(message_key_text.clone());
    let built_parameters = parameters
        .iter()
        .map(|(name, value)| ExplanationParameter::new(name.clone(), value.clone()))
        .collect::<Result<Vec<_>, _>>();

    let (Ok(rule_code), Ok(message_key), Ok(built_parameters)) =
        (rule_code, message_key, built_parameters)
    else {
        return;
    };
    if duplicate_names {
        // A parameter set with duplicate names must be refused, never
        // silently deduplicated or panicked on.
        assert!(RuleExplanation::new(
            GAMEBOARD_SCHEMA_VERSION,
            rule_code,
            message_key,
            built_parameters,
            provenance_text.clone(),
            selected_disclosure,
        )
        .is_err());
        return;
    }

    let Ok(explanation) = RuleExplanation::new(
        GAMEBOARD_SCHEMA_VERSION,
        rule_code,
        message_key,
        built_parameters,
        provenance_text,
        selected_disclosure,
    ) else {
        // Only the empty/control-character provenance case can still fail here.
        return;
    };
    assert_eq!(explanation.disclosure(), selected_disclosure);
    assert_eq!(explanation.parameters().len(), parameters.len());

    let encoded = serde_json::to_vec(&explanation).unwrap();
    let decoded: RuleExplanation = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, explanation);

    // 3. Disclosure filtering never leaks an explanation outside the
    // caller's allowed set, for every possible allow-list.
    let all = all_disclosures();
    for mask in 0u8..32 {
        let allowed = all
            .iter()
            .enumerate()
            .filter(|(bit, _)| mask & (1 << bit) != 0)
            .map(|(_, class)| *class)
            .collect::<Vec<_>>();
        let visible = filter_rule_explanations(std::slice::from_ref(&explanation), &allowed);
        if allowed.contains(&explanation.disclosure()) {
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0], &explanation);
        } else {
            assert!(visible.is_empty());
        }
    }
});
