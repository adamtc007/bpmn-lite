//! Private extraction-only typed argument evidence.

use std::collections::BTreeSet;

use semantic_decision_contracts::{ArgumentKind, LegalMove, SlotValue};

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExtractedArgument {
    kind: ArgumentKind,
    confidence: f64,
    provenance: &'static str,
}

impl ExtractedArgument {
    fn new(kind: ArgumentKind, confidence: f64, provenance: &'static str) -> Self {
        Self {
            kind,
            confidence,
            provenance,
        }
    }
}

/// Extract typed observations without binding or mutating a workbook.
pub(super) fn extract(utterance: &str) -> Vec<ExtractedArgument> {
    let normalized = utterance.to_lowercase();
    let tokens = normalized
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let mut extracted = Vec::new();
    let has_duration = tokens
        .windows(2)
        .any(|pair| pair[0].parse::<u64>().is_ok() && is_duration_unit(pair[1]));
    let has_standalone_count = tokens.iter().enumerate().any(|(index, token)| {
        token.parse::<u32>().is_ok()
            && tokens
                .get(index + 1)
                .is_none_or(|next| !is_duration_unit(next))
    });
    if has_standalone_count {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::Count,
            1.0,
            "bpmn.extract.count.v1",
        ));
    }
    if has_duration {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::Duration,
            1.0,
            "bpmn.extract.duration.v1",
        ));
    }
    if tokens
        .iter()
        .any(|token| matches!(*token, "true" | "false" | "yes" | "no"))
    {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::Boolean,
            0.8,
            "bpmn.extract.boolean.v1",
        ));
    }
    if normalized.contains("==")
        || normalized.contains("!=")
        || normalized.contains('>')
        || normalized.contains('<')
        || tokens
            .iter()
            .any(|token| matches!(*token, "if" | "when" | "unless"))
    {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::Condition,
            0.8,
            "bpmn.extract.condition.v1",
        ));
    }
    if tokens
        .iter()
        .any(|token| matches!(*token, "data" | "field" | "variable" | "collection"))
    {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::DataReference,
            0.6,
            "bpmn.extract.data-reference.v1",
        ));
    }
    if tokens
        .iter()
        .any(|token| matches!(*token, "subprocess" | "workflow" | "process"))
    {
        extracted.push(ExtractedArgument::new(
            ArgumentKind::SubprocessReference,
            0.6,
            "bpmn.extract.subprocess-reference.v1",
        ));
    }
    extracted
}

fn is_duration_unit(token: &str) -> bool {
    matches!(
        token,
        "ms" | "millisecond"
            | "milliseconds"
            | "second"
            | "seconds"
            | "minute"
            | "minutes"
            | "hour"
            | "hours"
            | "day"
            | "days"
    )
}

pub(super) fn score(
    legal_move: &LegalMove,
    utterance: &str,
    extracted: &[ExtractedArgument],
) -> f64 {
    let utterance = utterance.to_lowercase();
    let observed = extracted
        .iter()
        .map(|argument| argument.kind)
        .collect::<BTreeSet<_>>();
    let relevant = legal_move
        .arguments()
        .iter()
        .filter(|argument| argument.required())
        .collect::<Vec<_>>();
    if relevant.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    for argument in &relevant {
        let explicit_node = match argument.value() {
            Some(SlotValue::NodeReference(reference)) => {
                utterance.contains(&reference.to_lowercase())
            }
            _ => false,
        };
        if explicit_node {
            total += 1.0;
        } else if observed.contains(&argument.kind()) {
            total += extracted
                .iter()
                .filter(|extracted| extracted.kind == argument.kind())
                .map(|extracted| {
                    debug_assert!(!extracted.provenance.is_empty());
                    extracted.confidence
                })
                .fold(0.0, f64::max);
        } else if matches!(
            argument.kind(),
            ArgumentKind::Identifier | ArgumentKind::Text
        ) && !utterance.trim().is_empty()
        {
            total += 0.1;
        }
    }
    (total / relevant.len() as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_and_count_are_separate_extracted_observations() {
        let duration = extract("wait 5 minutes");
        assert!(duration
            .iter()
            .any(|value| value.kind == ArgumentKind::Duration));
        assert!(!duration
            .iter()
            .any(|value| value.kind == ArgumentKind::Count));
        let count = extract("create 3 branches");
        assert!(count.iter().any(|value| value.kind == ArgumentKind::Count));
        assert!(!count
            .iter()
            .any(|value| value.kind == ArgumentKind::Duration));
    }
}
