use super::dto::WorkflowGraphDto;
use anyhow::Result;

/// Parse a YAML string into a WorkflowGraphDto.
///
/// Validation is NOT performed here — call `validate_dto()` or use
/// `compile_from_dto()` / `compile_from_yaml()` on the engine which
/// validates before IR conversion.
pub fn parse_workflow_yaml(yaml_str: &str) -> Result<WorkflowGraphDto> {
    let dto: WorkflowGraphDto = serde_yaml::from_str(yaml_str)?;
    Ok(dto)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::*;

    #[test]
    fn test_basic_yaml_parse() {
        let yaml = r#"
id: test-workflow
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
        let dto = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(dto.id, "test-workflow");
        assert_eq!(dto.nodes.len(), 3);
        assert_eq!(dto.edges.len(), 2);
    }

    #[test]
    fn test_yaml_with_conditions() {
        let yaml = r#"
id: xor-workflow
nodes:
  - kind: Start
    id: start
  - kind: ExclusiveGateway
    id: gw
  - kind: ServiceTask
    id: task_a
    task_type: do_a
  - kind: ServiceTask
    id: task_b
    task_type: do_b
  - kind: End
    id: end
edges:
  - from: start
    to: gw
  - from: gw
    to: task_a
    condition:
      flag: approved
      op: "=="
      value: true
  - from: gw
    to: task_b
    is_default: true
  - from: task_a
    to: end
  - from: task_b
    to: end
"#;
        let dto = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(dto.nodes.len(), 5);

        // Verify condition parsed as struct
        let cond_edge = &dto.edges[1];
        assert!(cond_edge.condition.is_some());
        let cond = cond_edge.condition.as_ref().unwrap();
        assert_eq!(cond.flag, "approved");
        assert!(matches!(cond.op, FlagOp::Eq));
        assert!(matches!(cond.value, FlagValue::Bool(true)));

        // Verify default edge
        let default_edge = &dto.edges[2];
        assert!(default_edge.is_default);
    }

    /// FlagCondition must be a struct, not a bare string.
    #[test]
    fn test_bare_string_condition_fails() {
        let yaml = r#"
id: bad
nodes:
  - kind: Start
    id: start
  - kind: End
    id: end
edges:
  - from: start
    to: end
    condition: "x == true"
"#;
        let result = parse_workflow_yaml(yaml);
        assert!(
            result.is_err(),
            "Bare string condition should fail deserialization"
        );
    }

    #[test]
    fn test_yaml_with_human_wait() {
        let yaml = r#"
id: human-workflow
nodes:
  - kind: Start
    id: start
  - kind: HumanWait
    id: approval
    task_kind: manager_approval
  - kind: End
    id: end
edges:
  - from: start
    to: approval
  - from: approval
    to: end
"#;
        let dto = parse_workflow_yaml(yaml).unwrap();
        assert_eq!(dto.nodes.len(), 3);
        match &dto.nodes[1] {
            NodeDto::HumanWait {
                task_kind,
                corr_key_source,
                ..
            } => {
                assert_eq!(task_kind, "manager_approval");
                assert_eq!(corr_key_source, "instance_id"); // default
            }
            other => panic!("Expected HumanWait, got {:?}", other),
        }
    }

    #[test]
    fn test_yaml_with_error_edge() {
        let yaml = r#"
id: error-workflow
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: task_a
    task_type: do_work
  - kind: ServiceTask
    id: escalation
    task_type: handle_error
  - kind: End
    id: end
edges:
  - from: start
    to: task_a
  - from: task_a
    to: end
  - from: task_a
    to: escalation
    on_error:
      error_code: BIZ_001
      retries: 0
  - from: escalation
    to: end
"#;
        let dto = parse_workflow_yaml(yaml).unwrap();
        let error_edge = &dto.edges[2];
        assert!(error_edge.on_error.is_some());
        let on_err = error_edge.on_error.as_ref().unwrap();
        assert_eq!(on_err.error_code, "BIZ_001");
        assert_eq!(on_err.retries, 0);
    }

    // ── Save-as-template Step 2 (2026-07-30) ──────────────────────────

    /// GREEN: a DataObject declaration parses from YAML and passes
    /// validate_dto; RED (V16): wiring a sequence flow into it is
    /// rejected; RED (V1): a duplicate data-object id is rejected.
    #[test]
    fn data_object_parses_validates_and_rejects_misuse() {
        use crate::validate::validate_dto;
        use bpmn_lite_types::{DataObjectRole, DataObjectType, PrimitiveType};

        let yaml = r#"
id: dobj_yaml
nodes:
  - kind: Start
    id: start
  - kind: ServiceTask
    id: task_a
    task_type: do_work
  - kind: End
    id: end
  - kind: DataObject
    id: corr_flag
    name: corr_flag
    type_decl:
      kind: primitive
      data: string
edges:
  - from: start
    to: task_a
  - from: task_a
    to: end
"#;
        let dto = parse_workflow_yaml(yaml).unwrap();
        let dobj = dto
            .nodes
            .iter()
            .find_map(|n| match n {
                crate::dto::NodeDto::DataObject {
                    id,
                    type_decl,
                    role,
                    ..
                } if id == "corr_flag" => Some((type_decl.clone(), role.clone())),
                _ => None,
            })
            .expect("DataObject must parse from YAML");
        assert_eq!(dobj.0, DataObjectType::Primitive(PrimitiveType::String));
        // role omitted in YAML → defaults to Internal.
        assert_eq!(dobj.1, DataObjectRole::Internal);
        assert!(validate_dto(&dto).is_empty(), "clean graph must validate");

        // RED (V16): a sequence flow into a DataObject is rejected.
        let mut bad_edge = dto.clone();
        bad_edge.edges.push(crate::dto::EdgeDto {
            from: "task_a".to_string(),
            to: "corr_flag".to_string(),
            condition: None,
            is_default: false,
            on_error: None,
        });
        let errors = validate_dto(&bad_edge);
        assert!(
            errors.iter().any(|e| e.rule == "V16"),
            "edge into a DataObject must trip V16: {errors:?}"
        );

        // RED (V1): duplicate data-object id is rejected.
        let mut dup = dto.clone();
        dup.nodes.push(crate::dto::NodeDto::DataObject {
            id: "corr_flag".to_string(),
            name: "corr_flag_again".to_string(),
            type_decl: DataObjectType::Primitive(PrimitiveType::Bool),
            role: DataObjectRole::Internal,
        });
        let errors = validate_dto(&dup);
        assert!(
            errors.iter().any(|e| e.rule == "V1"),
            "duplicate DataObject id must trip V1: {errors:?}"
        );
    }
}
