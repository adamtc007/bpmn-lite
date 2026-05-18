use crate::contracts::ContractRegistry;
use crate::dto::WorkflowGraphDto;
use crate::export_bpmn::dto_to_bpmn_xml;
use crate::lints::{lint_contracts, LintDiagnostic, LintLevel};
use crate::registry::{SourceFormat, TemplateState, TemplateStore, WorkflowTemplate};
use crate::{dto_to_ir, validate, yaml};
use anyhow::{anyhow, Result};
use bpmn_lite_compiler::{lowering, verifier};
use bpmn_lite_store::store::ProcessStore;
use bpmn_lite_types::CompiledProgram;
use std::fmt::Write;

/// Phase 2.7 inversion entrypoint: pure DTO → CompiledProgram.
///
/// Runs the same validate → dto_to_ir → verify → lower → verify_bytecode
/// pipeline the retired `BpmnLiteEngine::compile_from_dto` used, but
/// returns the program **without** persisting. Caller (engine
/// test, future gRPC RPC) hands the result to
/// `engine.store_compiled_program(program)` to land it in
/// `ProcessStore`.
pub fn compile_program_from_dto(dto: &WorkflowGraphDto) -> Result<CompiledProgram> {
    let errors = validate::validate_dto(dto);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .iter()
            .map(|e| format!("[{}] {}", e.rule, e.message))
            .collect();
        return Err(anyhow!("DTO validation failed: {}", msgs.join("; ")));
    }

    let ir = dto_to_ir::dto_to_ir(dto)?;

    let verify_errors = verifier::verify(&ir);
    if !verify_errors.is_empty() {
        let msgs: Vec<String> = verify_errors.iter().map(|e| e.message.clone()).collect();
        return Err(anyhow!("Verification failed:\n{}", msgs.join("\n")));
    }

    let program = lowering::lower(&ir)?;

    let bytecode_errors = verifier::verify_bytecode(&program);
    if !bytecode_errors.is_empty() {
        let msgs: Vec<String> = bytecode_errors.iter().map(|e| e.message.clone()).collect();
        return Err(anyhow!(
            "Bytecode verification failed:\n{}",
            msgs.join("\n")
        ));
    }

    Ok(program)
}

/// Phase 2.7 inversion entrypoint: YAML → CompiledProgram.
///
/// Parses YAML to DTO, then runs `compile_program_from_dto`.
pub fn compile_program_from_yaml(yaml_str: &str) -> Result<CompiledProgram> {
    let dto = yaml::parse_workflow_yaml(yaml_str)?;
    compile_program_from_dto(&dto)
}

/// Phase 2.7 inversion entrypoint: YAML → publish (atomic).
///
/// Runs the full publish pipeline (parse → validate → lint →
/// compile → hash), then persists the compiled program to
/// `ProcessStore` and the template to `TemplateStore`. The two
/// writes are sequential — if the template write fails, no Draft
/// row is left behind. Program writes are idempotent (keyed by
/// bytecode hash) so retries are safe.
///
/// Replaces the retired `BpmnLiteEngine::compile_and_publish`
/// method. Now lives here so the engine has no authoring edge.
pub async fn compile_and_publish(
    yaml_str: &str,
    options: PublishOptions,
    template_store: &dyn TemplateStore,
    process_store: &dyn ProcessStore,
) -> Result<PublishResult> {
    let result = publish_workflow(yaml_str, options)?;

    process_store
        .store_program(result.program.bytecode_version, &result.program)
        .await?;

    template_store.save(&result.template).await?;

    Ok(result)
}

/// Options for the publish pipeline.
pub struct PublishOptions {
    pub template_key: String,
    pub template_version: u32,
    pub process_key: String,
    pub source_format: SourceFormat,
    pub contract_registry: Option<ContractRegistry>,
    pub generate_bpmn: bool,
    pub verb_registry_hash: Option<String>,
}

/// Result of a successful publish pipeline run.
#[derive(Debug)]
pub struct PublishResult {
    pub template: WorkflowTemplate,
    pub program: bpmn_lite_types::CompiledProgram,
    pub lint_diagnostics: Vec<LintDiagnostic>,
}

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{:02x}", b);
        acc
    })
}

/// Single-step publish pipeline. Truly atomic — no intermediate Draft artifact.
///
/// Pipeline:
/// 1. `parse_workflow_yaml()` → DTO
/// 2. `validate_dto()` → reject if errors
/// 3. `lint_contracts()` → collect diagnostics, reject if any Error-level
/// 4. `dto_to_ir()` → IRGraph
/// 5. `verifier::verify()` → reject if errors
/// 6. `lowering::lower()` → CompiledProgram
/// 7. `bytecode_version = blake3(serialized program bytecode)`
/// 8. Extract `task_manifest` from CompiledProgram
/// 9. Optional: `dto_to_bpmn_xml()` if `generate_bpmn=true`
/// 10. Build WorkflowTemplate with `state=Published, published_at=now`
///
/// Steps 1-10 are pure/sync (no persistence). The caller (`compile_and_publish`)
/// persists: (a) program to ProcessStore, (b) template to TemplateStore.
pub fn publish_workflow(yaml_str: &str, options: PublishOptions) -> Result<PublishResult> {
    // 1. Parse YAML → DTO
    let dto = yaml::parse_workflow_yaml(yaml_str)?;

    // 2. Validate DTO
    let validation_errors = validate::validate_dto(&dto);
    if !validation_errors.is_empty() {
        let msgs: Vec<String> = validation_errors
            .iter()
            .map(|e| format!("[{}] {}", e.rule, e.message))
            .collect();
        return Err(anyhow!("Validation failed:\n{}", msgs.join("\n")));
    }

    // 3. Lint contracts (if registry provided)
    let lint_diagnostics = if let Some(ref registry) = options.contract_registry {
        let diags = lint_contracts(&dto, registry);
        // Reject if any Error-level diagnostics
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| matches!(d.level, LintLevel::Error))
            .collect();
        if !errors.is_empty() {
            let msgs: Vec<String> = errors
                .iter()
                .map(|d| format!("[{}] {}", d.rule, d.message))
                .collect();
            return Err(anyhow!("Lint errors block publish:\n{}", msgs.join("\n")));
        }
        diags
    } else {
        vec![]
    };

    // 4. DTO → IR
    let ir = dto_to_ir::dto_to_ir(&dto)?;

    // 5. Verify IR
    let verify_errors = verifier::verify(&ir);
    if !verify_errors.is_empty() {
        let msgs: Vec<String> = verify_errors.iter().map(|e| e.message.clone()).collect();
        return Err(anyhow!("Verification failed:\n{}", msgs.join("\n")));
    }

    // 6. Lower to bytecode
    let program = lowering::lower(&ir)?;

    // 7. Bytecode verification
    let bytecode_errors = verifier::verify_bytecode(&program);
    if !bytecode_errors.is_empty() {
        let msgs: Vec<String> = bytecode_errors.iter().map(|e| e.message.clone()).collect();
        return Err(anyhow!(
            "Bytecode verification failed:\n{}",
            msgs.join("\n")
        ));
    }

    // 8. Compute bytecode_version hash from program bytecode only
    //    (debug_map and manifest excluded from hash for stability)
    let bytecode_version = compute_bytecode_hash(&program);

    // 9. Extract task_manifest
    let task_manifest = program.task_manifest.clone();

    // 10. Optional BPMN XML export
    let bpmn_xml = if options.generate_bpmn {
        Some(dto_to_bpmn_xml(&dto)?)
    } else {
        None
    };

    // 11. Build template — state=Published, published_at=now
    let now = now_ms();
    let template = WorkflowTemplate {
        template_key: options.template_key,
        template_version: options.template_version,
        process_key: options.process_key,
        bytecode_version,
        state: TemplateState::Published,
        source_format: options.source_format,
        dto_snapshot: dto,
        task_manifest,
        bpmn_xml,
        summary_md: None,
        verb_registry_hash: options.verb_registry_hash,
        created_at: now,
        published_at: Some(now),
    };

    Ok(PublishResult {
        template,
        program,
        lint_diagnostics,
    })
}

/// Compute a deterministic hex BLAKE3 hash of the program bytecode.
/// Excludes debug_map and task_manifest from the hash for stability.
fn compute_bytecode_hash(program: &bpmn_lite_types::CompiledProgram) -> String {
    let mut hasher = blake3::Hasher::new();
    for instr in &program.program {
        hasher.update(format!("{:?}", instr).as_bytes());
    }
    hex_encode(hasher.finalize().as_bytes())
}

/// Helper: shorthand for compile_and_publish without the DTO.
/// Returns the DTO for callers that need it.
pub fn parse_and_validate_yaml(yaml_str: &str) -> Result<WorkflowGraphDto> {
    let dto = yaml::parse_workflow_yaml(yaml_str)?;
    let errors = validate::validate_dto(&dto);
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .iter()
            .map(|e| format!("[{}] {}", e.rule, e.message))
            .collect();
        return Err(anyhow!("Validation failed:\n{}", msgs.join("\n")));
    }
    Ok(dto)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{ContractRegistry, VerbContract};
    use std::collections::HashSet;

    const MINIMAL_YAML: &str = r#"
id: publish_test
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: task_a
    task_type: do_work
  - kind: End
    id: end
edges:
  - from: start
    to: task_a
  - from: task_a
    to: end
"#;

    fn default_options() -> PublishOptions {
        PublishOptions {
            template_key: "test_wf".to_string(),
            template_version: 1,
            process_key: "test_proc".to_string(),
            source_format: SourceFormat::Yaml,
            contract_registry: None,
            generate_bpmn: false,
            verb_registry_hash: None,
        }
    }

    /// T-PUB-6: Minimal YAML publishes successfully, state=Published.
    #[test]
    fn t_pub_6_minimal_publish() {
        let result = publish_workflow(MINIMAL_YAML, default_options()).unwrap();
        assert_eq!(result.template.state, TemplateState::Published);
        assert!(result.template.published_at.is_some());
        assert_eq!(result.template.template_key, "test_wf");
        assert_eq!(result.template.template_version, 1);
        assert!(!result.template.bytecode_version.is_empty());
        assert!(result.template.bpmn_xml.is_none()); // generate_bpmn=false
    }

    /// T-PUB-7: Lint Error-level blocks publish.
    #[test]
    fn t_pub_7_lint_error_blocks() {
        // Create a workflow with a condition flag that has no upstream writer
        let yaml = r#"
id: lint_fail
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: check
    task_type: check_sanctions
  - kind: ExclusiveGateway
    id: xor
  - kind: ServiceTask
    id: proceed
    task_type: proceed
  - kind: End
    id: end
edges:
  - from: start
    to: check
  - from: check
    to: xor
  - from: xor
    to: proceed
    condition:
      flag: nonexistent_flag
      op: "=="
      value: true
  - from: xor
    to: end
    is_default: true
  - from: proceed
    to: end
"#;

        // Registry with contract for check_sanctions but it doesn't write nonexistent_flag
        let mut registry = ContractRegistry::new();
        registry.register(VerbContract {
            task_type: "check_sanctions".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: ["sanctions_clear".to_string()].into_iter().collect(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        });
        registry.register(VerbContract {
            task_type: "proceed".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: HashSet::new(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        });

        let options = PublishOptions {
            contract_registry: Some(registry),
            ..default_options()
        };

        let result = publish_workflow(yaml, options);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Lint errors"));
    }

    /// T-PUB-8: Same YAML → identical bytecode_version (deterministic).
    #[test]
    fn t_pub_8_deterministic_bytecode() {
        let r1 = publish_workflow(MINIMAL_YAML, default_options()).unwrap();
        let r2 = publish_workflow(MINIMAL_YAML, default_options()).unwrap();
        assert_eq!(
            r1.template.bytecode_version, r2.template.bytecode_version,
            "Same YAML should produce identical bytecode hash"
        );
    }

    /// T-PUB-9: Artifact set: dto_snapshot + task_manifest + bytecode_version present.
    #[test]
    fn t_pub_9_artifact_set() {
        let mut options = default_options();
        options.generate_bpmn = true;
        options.verb_registry_hash = Some("registry_abc".to_string());

        let result = publish_workflow(MINIMAL_YAML, options).unwrap();
        let tpl = &result.template;

        // dto_snapshot present
        assert!(!tpl.dto_snapshot.nodes.is_empty());

        // task_manifest present and non-empty
        assert!(!tpl.task_manifest.is_empty());
        assert!(tpl.task_manifest.contains(&"do_work".to_string()));

        // bytecode_version present
        assert!(!tpl.bytecode_version.is_empty());

        // BPMN XML generated
        assert!(tpl.bpmn_xml.is_some());
        assert!(tpl.bpmn_xml.as_ref().unwrap().contains("<bpmn:"));

        // verb_registry_hash forwarded
        assert_eq!(tpl.verb_registry_hash.as_deref(), Some("registry_abc"));
    }
}
