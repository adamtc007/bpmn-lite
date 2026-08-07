//! Exhaustive BPMN-owned semantic projection of the Designer catalogue.
//!
//! Designer owns candidate identity and positional legality. This module owns
//! the model-visible BPMN meaning of every identity and maps it into the shared
//! SemOS contracts without introducing a second candidate taxonomy.

use std::{collections::BTreeSet, sync::OnceLock};

use designer_graph::board_candidate::{CandidateId, OperationKind, ProductionId};
use semantic_decision_contracts::{
    ArgumentSpec, CandidateSemanticSlice, CanonicalCandidateId, DomainIdentity, GraphRevision,
    MessageKey, NegativeContrast, PhraseEvidence, ResolvedPosition, RuleCode,
    SemanticDecisionBoard, SnapshotIdentity,
};
use semantic_pack::{
    admit_pack, CapabilityId, CompiledCapability, CompiledPack, ConfigValue, PackBytes,
};

pub(crate) const BPMN_SEMANTIC_SCHEMA_VERSION: u32 = 1;
pub(crate) const APPLICABILITY_RULE_CODE: &str = "bpmn.applicability";
pub(crate) const ARGUMENT_RULE_CODE: &str = "bpmn.argument.required";
pub(crate) const COMPILER_REFUSAL_RULE_CODE: &str = "bpmn.compiler.refused";
pub(crate) const EVIDENCE_RULE_CODE: &str = "bpmn.evidence";
pub(crate) const POLICY_HIDDEN_RULE_CODE: &str = "bpmn.policy.hidden";

/// Deterministic binding capability associated with a semantic candidate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinderSupport {
    Supported,
    NeedsWorkbook,
    NotRepresentable,
}

impl BinderSupport {
    fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::NeedsWorkbook => "needs_workbook",
            Self::NotRepresentable => "not_representable",
        }
    }
}

/// One BPMN semantic contract paired with its deterministic binder capability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BpmnCandidateSpec {
    pub semantic: CandidateSemanticSlice,
    pub binder_support: BinderSupport,
    pub adapter_binding: String,
}

pub(crate) fn compiled_semantic_pack() -> &'static CompiledPack {
    static PACK: OnceLock<CompiledPack> = OnceLock::new();
    PACK.get_or_init(|| {
        let pack = admit_pack(PackBytes::new(
            "utterance-engine/config/bpmn-semantic-pack.yaml",
            include_bytes!("../config/bpmn-semantic-pack.yaml"),
        ))
        .expect("the embedded BPMN semantic pack must pass shared admission");
        validate_bpmn_governance(&pack)
            .expect("the embedded BPMN pack must close every adapter governance resource");
        pack
    })
}

fn validate_bpmn_governance(pack: &CompiledPack) -> Result<(), String> {
    let declared = pack
        .evidence()
        .features
        .iter()
        .map(|feature| feature.lane)
        .collect::<BTreeSet<_>>();
    let required = semantic_decision_contracts::EvidenceLane::ALL
        .into_iter()
        .collect::<BTreeSet<_>>();
    if declared != required {
        return Err("BPMN evidence policy must declare every shared lane exactly once".into());
    }
    for code in [
        APPLICABILITY_RULE_CODE,
        ARGUMENT_RULE_CODE,
        COMPILER_REFUSAL_RULE_CODE,
        EVIDENCE_RULE_CODE,
        POLICY_HIDDEN_RULE_CODE,
    ] {
        let code = RuleCode::new(code).map_err(|error| error.to_string())?;
        let explanation = pack
            .rule_explanation(&code)
            .ok_or_else(|| format!("missing governed explanation `{}`", code.as_str()))?;
        if code.as_str() != EVIDENCE_RULE_CODE && explanation.feedback_options.is_empty() {
            return Err(format!(
                "governed explanation `{}` has no recovery",
                code.as_str()
            ));
        }
        for option in &explanation.feedback_options {
            if pack.feedback_option(option).is_none() {
                return Err(format!(
                    "governed explanation `{}` has dangling recovery `{}`",
                    code.as_str(),
                    option.as_str()
                ));
            }
        }
    }
    for capability in pack.capabilities() {
        if capability.phrases.is_empty() {
            return Err(format!(
                "candidate `{}` has no governed cues",
                capability.id
            ));
        }
        for argument in &capability.arguments {
            if argument.required && argument.clarification_prompt.trim().is_empty() {
                return Err(format!(
                    "candidate `{}` has an ungoverned required argument",
                    capability.id
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn rule_source(code: &str) -> &'static semantic_pack::RuleExplanationSource {
    compiled_semantic_pack()
        .rule_explanation(&RuleCode::new(code).expect("BPMN rule code is valid"))
        .expect("BPMN governance validation guarantees the rule")
}

pub(crate) fn feedback_source(id: &str) -> &'static semantic_pack::FeedbackOptionSource {
    compiled_semantic_pack()
        .feedback_option(&MessageKey::new(id).expect("BPMN feedback id is valid"))
        .expect("BPMN governance validation guarantees the feedback option")
}

fn compiled_candidate_spec(capability: &CompiledCapability) -> BpmnCandidateSpec {
    let binder_support = match capability.extensions.get("bpmn.binder_support") {
        Some(ConfigValue::Text(value)) if value == "supported" => BinderSupport::Supported,
        Some(ConfigValue::Text(value)) if value == "needs_workbook" => BinderSupport::NeedsWorkbook,
        Some(ConfigValue::Text(value)) if value == "not_representable" => {
            BinderSupport::NotRepresentable
        }
        value => panic!(
            "candidate {} has invalid bpmn.binder_support extension: {value:?}",
            capability.id
        ),
    };
    let expected_binding = format!("bpmn.designer.{}", capability.id);
    assert_eq!(
        capability.adapter_binding.as_str(),
        expected_binding,
        "candidate {} must bind through its stable BPMN adapter ID",
        capability.id
    );
    let adapter_payload_hash = blake3::hash(
        format!(
            "bpmn-adapter-payload-v{BPMN_SEMANTIC_SCHEMA_VERSION}:{}:{}",
            capability.id,
            binder_support.label()
        )
        .as_bytes(),
    )
    .to_hex()
    .to_string();
    BpmnCandidateSpec {
        semantic: CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new(capability.id.as_str())
                .expect("admitted pack capability IDs are valid shared identities"),
            schema_version: BPMN_SEMANTIC_SCHEMA_VERSION,
            title: capability.title.clone(),
            intent_summary: capability.intent_summary.clone(),
            action_class: capability.action_class,
            applicability: capability.applicability.clone(),
            effect: capability.effect.clone(),
            arguments: capability
                .arguments
                .iter()
                .map(|argument| ArgumentSpec {
                    name: argument.name.to_string(),
                    kind: argument.kind,
                    required: argument.required,
                    clarification_prompt: argument.clarification_prompt.clone(),
                })
                .collect(),
            phrases: capability
                .phrases
                .iter()
                .map(|phrase| PhraseEvidence {
                    text: phrase.text.clone(),
                    locale: phrase.locale.clone(),
                    role: phrase.role,
                    provenance: phrase.provenance.clone(),
                })
                .collect(),
            positive_examples: capability.positive_examples.clone(),
            negative_contrasts: capability
                .negative_contrasts
                .iter()
                .map(|contrast| NegativeContrast {
                    candidate_id: CanonicalCandidateId::new(contrast.candidate_id.as_str())
                        .expect("admitted contrast IDs are valid shared identities"),
                    distinction: contrast.distinction.clone(),
                })
                .collect(),
            risk: capability.risk,
            adapter_payload_hash,
        },
        binder_support,
        adapter_binding: capability.adapter_binding.as_str().to_string(),
    }
}

fn compiled_specs() -> Vec<BpmnCandidateSpec> {
    compiled_semantic_pack()
        .capabilities()
        .map(compiled_candidate_spec)
        .collect()
}

fn compiled_spec_by_id(canonical_id: &str) -> Option<BpmnCandidateSpec> {
    let id = CapabilityId::new(canonical_id).ok()?;
    compiled_semantic_pack()
        .capability(&id)
        .map(compiled_candidate_spec)
}

pub(crate) fn candidate_spec_by_canonical_id(canonical_id: &str) -> Option<BpmnCandidateSpec> {
    compiled_spec_by_id(canonical_id)
}

pub(crate) fn candidate_spec(candidate: CandidateId) -> Option<BpmnCandidateSpec> {
    (candidate != CandidateId::Abstain)
        .then(|| candidate.canonical_id())
        .and_then(compiled_spec_by_id)
}

pub(crate) fn all_specs() -> Vec<BpmnCandidateSpec> {
    compiled_specs()
}

/// The Phase 1 coverage rule: every `OperationKind` and `ProductionId` in
/// `designer-graph`'s candidate catalogue must have exactly one semantic
/// spec, no more and no less. Returns the missing and/or extra canonical
/// ids on failure so a caller (a future startup check, a test, or a CLI
/// gate) gets a localized diagnostic rather than a bare boolean.
pub(crate) fn validate_registry_coverage(
    specs: &[BpmnCandidateSpec],
) -> Result<(), RegistryCoverageError> {
    let registered = specs
        .iter()
        .map(|spec| spec.semantic.canonical_id.as_str())
        .collect::<BTreeSet<_>>();
    let catalogue = OperationKind::ALL
        .iter()
        .map(|candidate| candidate.canonical_id())
        .chain(
            ProductionId::ALL
                .iter()
                .map(|candidate| candidate.canonical_id()),
        )
        .collect::<BTreeSet<_>>();

    let missing = catalogue
        .difference(&registered)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let extra = registered
        .difference(&catalogue)
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        Ok(())
    } else {
        Err(RegistryCoverageError { missing, extra })
    }
}

/// Diagnostic for [`validate_registry_coverage`]: names exactly which
/// catalogue ids the registry is missing a spec for, and which registered
/// ids do not correspond to any real `OperationKind`/`ProductionId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegistryCoverageError {
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

impl std::fmt::Display for RegistryCoverageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BPMN semantic registry does not exactly cover the designer-graph catalogue: missing={:?}, extra={:?}",
            self.missing, self.extra
        )
    }
}

impl std::error::Error for RegistryCoverageError {}

pub(crate) fn semantic_snapshot_identity() -> SnapshotIdentity {
    let specs = all_specs();
    // Fail closed rather than silently serving a shrunken/widened board:
    // a coverage regression here means either a new designer-graph
    // candidate has no semantic contract yet (would be silently absent
    // from every future board) or the registry carries a phantom id (would
    // be silently offered as if it were real). Both are startup-fatal, not
    // a degraded-mode condition.
    validate_registry_coverage(&specs)
        .expect("BPMN semantic registry must exactly cover the designer-graph candidate catalogue");
    let profile = SemanticDecisionBoard::new(
        BPMN_SEMANTIC_SCHEMA_VERSION,
        DomainIdentity::new("bpmn.designer.semantic-profile")
            .expect("profile domain is a valid identity"),
        SnapshotIdentity::new("bpmn-semantic-profile-preimage-v1")
            .expect("profile preimage identity is valid"),
        GraphRevision::new("designer-candidate-schema-v3")
            .expect("candidate schema identity is valid"),
        ResolvedPosition {
            anchor: None,
            context_hash: format!(
                "full-bpmn-semantic-profile:{}",
                compiled_semantic_pack().receipt().artifact_hash.as_str()
            ),
        },
        specs.into_iter().map(|spec| spec.semantic).collect(),
        "semantic-profile-content-address-v1".to_string(),
    )
    .expect("the exhaustive BPMN semantic profile is valid");
    SnapshotIdentity::new(format!(
        "bpmn-semantic-profile-v1:{}",
        profile.board_hash.as_str()
    ))
    .expect("generated snapshot identity is valid")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use semantic_decision_contracts::{
        ArgumentKind, DomainIdentity, GraphRevision, ResolvedPosition, SemanticDecisionBoard,
    };

    use super::*;

    fn board_from_specs(specs: Vec<BpmnCandidateSpec>) -> SemanticDecisionBoard {
        SemanticDecisionBoard::new(
            1,
            DomainIdentity::new("bpmn.designer").unwrap(),
            semantic_snapshot_identity(),
            GraphRevision::new("rev-1").unwrap(),
            ResolvedPosition {
                anchor: Some("task-1".into()),
                context_hash: "context-1".into(),
            },
            specs.into_iter().map(|spec| spec.semantic).collect(),
            "policy-1".into(),
        )
        .unwrap()
    }

    #[test]
    fn semantic_registry_exhaustively_covers_designer_catalogue() {
        let specs = all_specs();
        assert_eq!(
            validate_registry_coverage(&specs),
            Ok(()),
            "the real coverage rule must admit the full registry"
        );

        assert_eq!(specs.len(), 26);
        assert!(specs.iter().all(|spec| {
            !spec.semantic.title.is_empty()
                && !spec.semantic.intent_summary.is_empty()
                && !spec.semantic.applicability.is_empty()
                && !spec.semantic.effect.is_empty()
                && !spec.semantic.arguments.is_empty()
                && !spec.semantic.phrases.is_empty()
                && !spec.semantic.positive_examples.is_empty()
                && !spec.semantic.negative_contrasts.is_empty()
        }));
    }

    #[test]
    fn every_executable_semantic_candidate_has_one_mutation_implementation() {
        let executable = all_specs()
            .into_iter()
            .filter(|spec| spec.binder_support != BinderSupport::NotRepresentable)
            .map(|spec| spec.semantic.canonical_id.as_str().to_string())
            .collect::<BTreeSet<_>>();
        let implemented = crate::legal_moves::MATERIALIZED_CANDIDATE_IDS
            .iter()
            .map(|candidate| (*candidate).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(executable, implemented);
        assert_eq!(
            implemented.len(),
            crate::legal_moves::MATERIALIZED_CANDIDATE_IDS.len(),
            "the mutation table must not contain duplicate candidate identities"
        );
    }

    #[test]
    fn nearest_neighbour_contrasts_are_reciprocal() {
        let by_id = all_specs()
            .into_iter()
            .map(|spec| (spec.semantic.canonical_id.as_str().to_string(), spec))
            .collect::<BTreeMap<_, _>>();
        let pairs = [
            ("op.append_node", "op.insert_after"),
            ("op.insert_before", "op.replace_node"),
            ("op.connect", "op.create_branch"),
            ("op.attach_guard", "op.attach_rearming_guard"),
            ("op.set_guard_trigger", "op.set_guard_budget"),
            ("op.create_parallel_region", "op.create_inclusive_region"),
            (
                "prod.interrupting_timeout",
                "prod.non_interrupting_notification",
            ),
        ];
        for (left, right) in pairs {
            for (source, target) in [(left, right), (right, left)] {
                assert!(by_id[source]
                    .semantic
                    .negative_contrasts
                    .iter()
                    .any(|contrast| contrast.candidate_id.as_str() == target));
            }
        }
    }

    #[test]
    fn registry_snapshot_is_content_addressed_and_stable() {
        assert_eq!(semantic_snapshot_identity(), semantic_snapshot_identity());
        assert_eq!(
            (
                semantic_snapshot_identity().as_str().to_owned(),
                compiled_semantic_pack()
                    .receipt()
                    .artifact_hash
                    .as_str()
                    .to_owned(),
            ),
            (
                "bpmn-semantic-profile-v1:0ff6747b192aa4df8181b17d0d922d779994a161cadc39c4d3ca36fd7568edc8".to_owned(),
                "c3dd92720bb671729970c6ef3530b79572c6b8bdd8be2b4d548f8717e5fa0d2a".to_owned(),
            )
        );
    }

    #[test]
    fn checked_in_pack_receipt_detects_source_and_binding_drift() {
        let receipt = compiled_semantic_pack().receipt();
        let lock = include_str!("../config/bpmn-semantic-pack.lock");
        assert_eq!(
            receipt.source_hash.as_str(),
            "4764ffb9a402d910abd635d5c4c1a21512107c600dbf60c12aee95b362dd0d68"
        );
        assert!(lock.contains(receipt.source_hash.as_str()));
        assert!(lock.contains(receipt.artifact_hash.as_str()));
        assert_eq!(receipt.adapter_bindings.len(), 26);
        for binding in &receipt.adapter_bindings {
            assert!(lock.contains(&format!("  - {binding}\n")));
        }
    }

    #[test]
    fn governed_phrase_index_metrics_are_reported() {
        let mut owners = BTreeMap::<String, BTreeSet<String>>::new();
        for spec in all_specs() {
            for phrase in spec.semantic.phrases {
                owners
                    .entry(crate::exact::normalize_phrase(&phrase.text))
                    .or_default()
                    .insert(spec.semantic.canonical_id.as_str().to_string());
            }
        }
        let collision_keys = owners.values().filter(|ids| ids.len() > 1).count();
        let unique_keys = owners.len() - collision_keys;
        let collision_rate = collision_keys as f64 / owners.len() as f64;
        println!(
            "governed exact metrics: phrase_keys={} unique={} collisions={} collision_rate={:.6}",
            owners.len(),
            unique_keys,
            collision_keys,
            collision_rate
        );
        assert!(unique_keys > 0);
        assert!(collision_rate.is_finite());
    }

    #[test]
    fn every_model_visible_semantic_dimension_moves_the_board_hash() {
        let base_specs = all_specs();
        let base = board_from_specs(base_specs.clone());

        let mut title = base_specs.clone();
        title[0].semantic.title.push_str(" changed");
        let mut argument = base_specs.clone();
        argument[0].semantic.arguments[0].kind = ArgumentKind::Text;
        let mut phrase = base_specs.clone();
        phrase[0].semantic.phrases[0].text.push_str(" changed");
        let mut contrast = base_specs;
        contrast[0].semantic.negative_contrasts[0]
            .distinction
            .push_str(" changed");

        for changed in [title, argument, phrase, contrast] {
            assert_ne!(base.board_hash, board_from_specs(changed).board_hash);
        }
    }

    #[test]
    fn test_only_incomplete_registry_is_refused_by_coverage_rule() {
        // Red receipt for the Phase 1 gate: a registry missing one
        // candidate must be refused by the SAME validator the green case
        // above depends on -- not by an inline assertion that happens to
        // agree with it. Popping the last entry (sorted by insertion,
        // i.e. `OperationKind::ALL` then `ProductionId::ALL`) removes the
        // final `ProductionId` spec.
        let mut incomplete = all_specs();
        let removed = incomplete.pop().expect("registry is non-empty");

        let error = validate_registry_coverage(&incomplete)
            .expect_err("a registry missing a real candidate must be refused");
        assert_eq!(error.extra, Vec::<String>::new());
        assert_eq!(
            error.missing,
            vec![removed.semantic.canonical_id.as_str().to_string()],
            "the diagnostic must name exactly the missing candidate"
        );

        // The same validator must also catch a registered id that does
        // not correspond to any real catalogue entry -- not just a
        // short registry.
        let mut extra = all_specs();
        let mut phantom = extra[0].clone();
        phantom.semantic = CandidateSemanticSlice {
            canonical_id: CanonicalCandidateId::new("op.not_a_real_candidate").unwrap(),
            ..phantom.semantic
        };
        extra.push(phantom);
        let error = validate_registry_coverage(&extra)
            .expect_err("a registered id absent from the real catalogue must be refused");
        assert_eq!(error.missing, Vec::<String>::new());
        assert_eq!(error.extra, vec!["op.not_a_real_candidate".to_string()]);
    }
}
