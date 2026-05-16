use super::contracts::ContractRegistry;
use super::dto::*;
use std::collections::{HashMap, HashSet, VecDeque};

/// Severity level for a lint diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    Error,
    Warning,
    Info,
}

/// A single lint diagnostic emitted by the contract linter.
#[derive(Debug, Clone)]
pub struct LintDiagnostic {
    /// Rule identifier: "L1", "L2", etc.
    pub rule: String,
    pub level: LintLevel,
    pub message: String,
    /// Node id where the issue was detected (if applicable).
    pub node_id: Option<String>,
}

impl std::fmt::Display for LintDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            LintLevel::Error => "ERROR",
            LintLevel::Warning => "WARN",
            LintLevel::Info => "INFO",
        };
        if let Some(ref nid) = self.node_id {
            write!(
                f,
                "[{}:{}] {} (node: {})",
                self.rule, level, self.message, nid
            )
        } else {
            write!(f, "[{}:{}] {}", self.rule, level, self.message)
        }
    }
}

/// Lint a `WorkflowGraphDto` against a `ContractRegistry`.
///
/// Returns all diagnostics found. Callers should check for any `LintLevel::Error`
/// entries to decide whether to block compilation/publishing.
///
/// ## Rules
///
/// - **L1 (Flag provenance):** Every flag referenced in a `FlagCondition` must be
///   written by an upstream `ServiceTask`. Backward BFS from the gateway collects
///   `writes_flags`. If the flag is in `known_workflow_inputs`, emit Warning not Error.
///
/// - **L2 (Error code validity):** `on_error.error_code` must be in the source task's
///   `may_raise_errors`. Catch-all `"*"` always satisfies.
///
/// - **L3 (Correlation provenance):** `MessageWait`/`HumanWait` `corr_key_source`
///   should match an upstream `produces_correlation.key_source`. Skip if
///   `corr_key_source == "instance_id"` (built-in). Warning only.
///
/// - **L4 (Missing contract):** `ServiceTask` without a registered contract. Warning.
///
/// - **L5 (Unused writes):** Verb declares `writes_flags` but no edge condition in
///   the entire workflow references that flag. Warning.
pub fn lint_contracts(dto: &WorkflowGraphDto, registry: &ContractRegistry) -> Vec<LintDiagnostic> {
    let mut diags = Vec::new();

    // Build index structures
    let node_map: HashMap<&str, &NodeDto> = dto.nodes.iter().map(|n| (n.id(), n)).collect();
    let mut incoming_edges: HashMap<&str, Vec<&EdgeDto>> = HashMap::new();
    for edge in &dto.edges {
        incoming_edges
            .entry(edge.to.as_str())
            .or_default()
            .push(edge);
    }

    // Collect all condition flags referenced in the entire workflow (for L5)
    let all_condition_flags: HashSet<&str> = dto
        .edges
        .iter()
        .filter_map(|e| e.condition.as_ref().map(|c| c.flag.as_str()))
        .collect();

    // ── L1: Flag provenance ──
    lint_l1_flag_provenance(dto, registry, &node_map, &incoming_edges, &mut diags);

    // ── L2: Error code validity ──
    lint_l2_error_codes(dto, registry, &node_map, &mut diags);

    // ── L3: Correlation provenance ──
    lint_l3_correlation(dto, registry, &node_map, &incoming_edges, &mut diags);

    // ── L4: Missing contract ──
    lint_l4_missing_contract(dto, registry, &mut diags);

    // ── L5: Unused writes ──
    lint_l5_unused_writes(registry, &all_condition_flags, &mut diags);

    diags
}

/// L1: Flag provenance — every flag in a FlagCondition must be written by an upstream ServiceTask.
fn lint_l1_flag_provenance(
    dto: &WorkflowGraphDto,
    registry: &ContractRegistry,
    node_map: &HashMap<&str, &NodeDto>,
    incoming_edges: &HashMap<&str, Vec<&EdgeDto>>,
    diags: &mut Vec<LintDiagnostic>,
) {
    // For every edge with a condition, check that the flag is written upstream
    for edge in &dto.edges {
        if let Some(cond) = &edge.condition {
            // The condition is on an edge FROM a gateway — check upstream of the gateway
            let gateway_id = &edge.from;
            let upstream = upstream_flags(gateway_id, dto, registry, node_map, incoming_edges);

            if !upstream.contains(cond.flag.as_str()) {
                let level = if registry.is_known_input(&cond.flag) {
                    LintLevel::Warning
                } else {
                    LintLevel::Error
                };
                let qualifier = if level == LintLevel::Warning {
                    " (known workflow input)"
                } else {
                    ""
                };
                diags.push(LintDiagnostic {
                    rule: "L1".to_string(),
                    level,
                    message: format!(
                        "Flag '{}' in condition on edge {}→{} is not written by any upstream task{}",
                        cond.flag, edge.from, edge.to, qualifier
                    ),
                    node_id: Some(gateway_id.clone()),
                });
            }
        }
    }
}

/// Backward BFS from `target_node_id` collecting all `writes_flags` from upstream ServiceTasks.
///
/// Race arm edge handling: if `edge.from` contains `.`, split on first `.` and use the left
/// part as the node_id (e.g., `"race.arm1"` → traverse from `"race"`).
fn upstream_flags<'a>(
    target_node_id: &str,
    _dto: &'a WorkflowGraphDto,
    registry: &ContractRegistry,
    node_map: &HashMap<&str, &'a NodeDto>,
    incoming_edges: &HashMap<&str, Vec<&EdgeDto>>,
) -> HashSet<String> {
    let mut flags = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(target_node_id.to_string());
    visited.insert(target_node_id.to_string());

    while let Some(current) = queue.pop_front() {
        // Find all edges pointing TO current
        if let Some(edges) = incoming_edges.get(current.as_str()) {
            for edge in edges {
                // Resolve base node id (handle dot-notation for race arms)
                let from_base = if edge.from.contains('.') {
                    edge.from.split('.').next().unwrap_or(&edge.from)
                } else {
                    &edge.from
                };

                if visited.contains(from_base) {
                    continue;
                }
                visited.insert(from_base.to_string());

                // If from_base is a ServiceTask with a contract, collect its writes_flags
                if let Some(NodeDto::ServiceTask { task_type, .. }) = node_map.get(from_base) {
                    if let Some(contract) = registry.get(task_type) {
                        flags.extend(contract.writes_flags.iter().cloned());
                    }
                }

                queue.push_back(from_base.to_string());
            }
        }
    }

    flags
}

/// L2: Error code validity — on_error.error_code must be in the source task's may_raise_errors.
fn lint_l2_error_codes(
    dto: &WorkflowGraphDto,
    registry: &ContractRegistry,
    node_map: &HashMap<&str, &NodeDto>,
    diags: &mut Vec<LintDiagnostic>,
) {
    for edge in &dto.edges {
        if let Some(on_error) = &edge.on_error {
            // Resolve base node (strip dot-notation)
            let from_base = if edge.from.contains('.') {
                edge.from.split('.').next().unwrap_or(&edge.from)
            } else {
                &edge.from
            };

            if let Some(NodeDto::ServiceTask { task_type, .. }) = node_map.get(from_base) {
                if let Some(contract) = registry.get(task_type) {
                    // Catch-all "*" satisfies any error code
                    if !contract.may_raise_errors.contains("*")
                        && !contract.may_raise_errors.contains(&on_error.error_code)
                    {
                        diags.push(LintDiagnostic {
                            rule: "L2".to_string(),
                            level: LintLevel::Error,
                            message: format!(
                                "Error code '{}' on edge {}→{} is not in {}'s may_raise_errors",
                                on_error.error_code, edge.from, edge.to, task_type
                            ),
                            node_id: Some(from_base.to_string()),
                        });
                    }
                }
                // If no contract, L4 will catch it — don't double-report
            }
        }
    }
}

/// L3: Correlation provenance — MessageWait/HumanWait corr_key_source should match upstream.
fn lint_l3_correlation(
    dto: &WorkflowGraphDto,
    registry: &ContractRegistry,
    node_map: &HashMap<&str, &NodeDto>,
    incoming_edges: &HashMap<&str, Vec<&EdgeDto>>,
    diags: &mut Vec<LintDiagnostic>,
) {
    for node in &dto.nodes {
        let (node_id, corr_key) = match node {
            NodeDto::MessageWait {
                id,
                corr_key_source,
                ..
            } => (id, corr_key_source),
            NodeDto::HumanWait {
                id,
                corr_key_source,
                ..
            } => (id, corr_key_source),
            _ => continue,
        };

        // Built-in "instance_id" is always valid
        if corr_key == "instance_id" {
            continue;
        }

        // Collect upstream produced correlations
        let upstream_corr = upstream_correlations(node_id, dto, registry, node_map, incoming_edges);

        if !upstream_corr.contains(corr_key.as_str()) {
            diags.push(LintDiagnostic {
                rule: "L3".to_string(),
                level: LintLevel::Warning,
                message: format!(
                    "corr_key_source '{}' on {} is not produced by any upstream task",
                    corr_key, node_id
                ),
                node_id: Some(node_id.clone()),
            });
        }
    }
}

/// Backward BFS collecting all `produces_correlation.key_source` from upstream ServiceTasks.
fn upstream_correlations<'a>(
    target_node_id: &str,
    _dto: &'a WorkflowGraphDto,
    registry: &ContractRegistry,
    node_map: &HashMap<&str, &'a NodeDto>,
    incoming_edges: &HashMap<&str, Vec<&EdgeDto>>,
) -> HashSet<String> {
    let mut keys = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(target_node_id.to_string());
    visited.insert(target_node_id.to_string());

    while let Some(current) = queue.pop_front() {
        if let Some(edges) = incoming_edges.get(current.as_str()) {
            for edge in edges {
                let from_base = if edge.from.contains('.') {
                    edge.from.split('.').next().unwrap_or(&edge.from)
                } else {
                    &edge.from
                };

                if visited.contains(from_base) {
                    continue;
                }
                visited.insert(from_base.to_string());

                if let Some(NodeDto::ServiceTask { task_type, .. }) = node_map.get(from_base) {
                    if let Some(contract) = registry.get(task_type) {
                        for corr in &contract.produces_correlation {
                            keys.insert(corr.key_source.clone());
                        }
                    }
                }

                queue.push_back(from_base.to_string());
            }
        }
    }

    keys
}

/// L4: Missing contract — ServiceTask without a registered contract.
fn lint_l4_missing_contract(
    dto: &WorkflowGraphDto,
    registry: &ContractRegistry,
    diags: &mut Vec<LintDiagnostic>,
) {
    for node in &dto.nodes {
        if let NodeDto::ServiceTask { id, task_type, .. } = node {
            if !registry.has(task_type) {
                diags.push(LintDiagnostic {
                    rule: "L4".to_string(),
                    level: LintLevel::Warning,
                    message: format!(
                        "ServiceTask '{}' (task_type='{}') has no registered contract",
                        id, task_type
                    ),
                    node_id: Some(id.clone()),
                });
            }
        }
    }
}

/// L5: Unused writes — verb declares writes_flags but no edge condition references the flag.
fn lint_l5_unused_writes(
    registry: &ContractRegistry,
    all_condition_flags: &HashSet<&str>,
    diags: &mut Vec<LintDiagnostic>,
) {
    for (task_type, contract) in registry.iter() {
        for flag in &contract.writes_flags {
            if !all_condition_flags.contains(flag.as_str()) {
                diags.push(LintDiagnostic {
                    rule: "L5".to_string(),
                    level: LintLevel::Warning,
                    message: format!(
                        "Flag '{}' written by '{}' is never referenced in any edge condition",
                        flag, task_type
                    ),
                    node_id: None,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::VerbContract;

    // ── Helpers ──

    fn make_registry_with(contracts: Vec<VerbContract>) -> ContractRegistry {
        let mut reg = ContractRegistry::new();
        for c in contracts {
            reg.register(c);
        }
        reg
    }

    fn xor_workflow_with_condition(flag: &str) -> WorkflowGraphDto {
        WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "branch_yes".to_string(),
                    task_type: "handle_yes".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "branch_no".to_string(),
                    task_type: "handle_no".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "branch_yes".to_string(),
                    condition: Some(FlagCondition {
                        flag: flag.to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "branch_no".to_string(),
                    condition: None,
                    is_default: true,
                    on_error: None,
                },
                EdgeDto {
                    from: "branch_yes".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "branch_no".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        }
    }

    // ── T-LINT-1: L1 Error — condition flag not written upstream ──

    #[test]
    fn t_lint_1_l1_error_flag_not_written() {
        let dto = xor_workflow_with_condition("unknown_flag");
        // Register do_work but it does NOT write "unknown_flag"
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: ["other_flag".to_string()].into(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l1_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L1" && d.level == LintLevel::Error)
            .collect();
        assert!(
            !l1_errors.is_empty(),
            "Expected L1 Error for unknown_flag, got: {:?}",
            diags
        );
    }

    // ── T-LINT-2: L1 — No error when upstream task writes the flag ──

    #[test]
    fn t_lint_2_l1_pass_when_written() {
        let dto = xor_workflow_with_condition("sanctions_clear");
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: ["sanctions_clear".to_string()].into(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l1_issues: Vec<_> = diags.iter().filter(|d| d.rule == "L1").collect();
        assert!(
            l1_issues.is_empty(),
            "Expected no L1 issues, got: {:?}",
            l1_issues
        );
    }

    // ── T-LINT-3: L1 Warning — flag is in known_workflow_inputs ──

    #[test]
    fn t_lint_3_l1_warning_known_input() {
        let dto = xor_workflow_with_condition("orch_high_risk");
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: HashSet::new(), // Does NOT write orch_high_risk
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        }])
        .with_known_inputs(["orch_high_risk"]);

        let diags = lint_contracts(&dto, &reg);
        let l1_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L1" && d.level == LintLevel::Warning)
            .collect();
        assert!(
            !l1_warnings.is_empty(),
            "Expected L1 Warning for known input, got: {:?}",
            diags
        );
        // Must NOT be an error
        let l1_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L1" && d.level == LintLevel::Error)
            .collect();
        assert!(
            l1_errors.is_empty(),
            "Expected no L1 Errors for known input, got: {:?}",
            l1_errors
        );
    }

    // ── T-LINT-4: L2 Error — error code not in may_raise_errors ──

    #[test]
    fn t_lint_4_l2_invalid_error_code() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "escalation".to_string(),
                    task_type: "escalate".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                // Error edge with code NOT in contract
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "escalation".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: Some(ErrorEdge {
                        error_code: "UNKNOWN_CODE".to_string(),
                        retries: 0,
                    }),
                },
                EdgeDto {
                    from: "escalation".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: HashSet::new(),
            may_raise_errors: ["SANCTIONS_HIT".to_string()].into(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l2_errors: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L2" && d.level == LintLevel::Error)
            .collect();
        assert!(
            !l2_errors.is_empty(),
            "Expected L2 Error for UNKNOWN_CODE, got: {:?}",
            diags
        );
    }

    // ── T-LINT-5: L2 — No error with catch-all "*" ──

    #[test]
    fn t_lint_5_l2_pass_with_catchall() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "escalation".to_string(),
                    task_type: "escalate".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "escalation".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: Some(ErrorEdge {
                        error_code: "ANY_CODE_AT_ALL".to_string(),
                        retries: 0,
                    }),
                },
                EdgeDto {
                    from: "escalation".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        // Contract has catch-all "*"
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: HashSet::new(),
            may_raise_errors: ["*".to_string()].into(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l2_errors: Vec<_> = diags.iter().filter(|d| d.rule == "L2").collect();
        assert!(
            l2_errors.is_empty(),
            "Expected no L2 issues with catch-all *, got: {:?}",
            l2_errors
        );
    }

    // ── T-LINT-6: L3 Warning — corr_key_source not produced upstream ──

    #[test]
    fn t_lint_6_l3_corr_not_produced() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::MessageWait {
                    id: "wait_msg".to_string(),
                    name: "doc_ready".to_string(),
                    corr_key_source: "missing_corr_key".to_string(),
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "wait_msg".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "wait_msg".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: HashSet::new(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![], // Does NOT produce missing_corr_key
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l3_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L3" && d.level == LintLevel::Warning)
            .collect();
        assert!(
            !l3_warnings.is_empty(),
            "Expected L3 Warning, got: {:?}",
            diags
        );
    }

    // ── T-LINT-7: L4 Warning — ServiceTask without registered contract ──

    #[test]
    fn t_lint_7_l4_missing_contract() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "unregistered_task".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        // Empty registry — no contracts registered
        let reg = ContractRegistry::new();

        let diags = lint_contracts(&dto, &reg);
        let l4_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L4" && d.level == LintLevel::Warning)
            .collect();
        assert!(
            !l4_warnings.is_empty(),
            "Expected L4 Warning, got: {:?}",
            diags
        );
    }

    // ── T-LINT-8: L5 Warning — writes_flags never referenced ──

    #[test]
    fn t_lint_8_l5_unused_writes() {
        // Simple linear workflow — no conditions at all
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };
        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: ["never_used_flag".to_string()].into(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l5_warnings: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "L5" && d.level == LintLevel::Warning)
            .collect();
        assert!(
            !l5_warnings.is_empty(),
            "Expected L5 Warning for unused flag, got: {:?}",
            diags
        );
    }

    // ── T-LINT-9: L1 backward BFS traverses race arm edges (dot-notation) ──

    #[test]
    fn t_lint_9_l1_race_arm_dot_notation() {
        // Workflow: start → task_a → race → (race.timer_arm → xor → ..., race.msg_arm → ...)
        // The XOR condition references a flag written by task_a.
        // BFS must traverse through "race.timer_arm" → "race" → "task_a".
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "task_a".to_string(),
                    task_type: "do_work".to_string(),
                    bpmn_id: None,
                },
                NodeDto::RaceWait {
                    id: "race".to_string(),
                    arms: vec![
                        RaceArm {
                            arm_id: "timer_arm".to_string(),
                            kind: RaceArmKind::Timer {
                                duration_ms: 30000,
                                interrupting: true,
                            },
                        },
                        RaceArm {
                            arm_id: "msg_arm".to_string(),
                            kind: RaceArmKind::Message {
                                name: "doc_ready".to_string(),
                                corr_key_source: "instance_id".to_string(),
                            },
                        },
                    ],
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "branch_a".to_string(),
                    task_type: "handle_a".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "branch_b".to_string(),
                    task_type: "handle_b".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "task_a".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "task_a".to_string(),
                    to: "race".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                // Race arm edges use dot-notation
                EdgeDto {
                    from: "race.timer_arm".to_string(),
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "race.msg_arm".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                // XOR branches — condition references flag written by task_a
                EdgeDto {
                    from: "xor".to_string(),
                    to: "branch_a".to_string(),
                    condition: Some(FlagCondition {
                        flag: "work_done".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "branch_b".to_string(),
                    condition: None,
                    is_default: true,
                    on_error: None,
                },
                EdgeDto {
                    from: "branch_a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "branch_b".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let reg = make_registry_with(vec![VerbContract {
            task_type: "do_work".to_string(),
            reads_flags: HashSet::new(),
            writes_flags: ["work_done".to_string()].into(),
            may_raise_errors: HashSet::new(),
            produces_correlation: vec![],
        }]);

        let diags = lint_contracts(&dto, &reg);
        let l1_issues: Vec<_> = diags.iter().filter(|d| d.rule == "L1").collect();
        assert!(
            l1_issues.is_empty(),
            "Expected no L1 issues (BFS should traverse race arms), got: {:?}",
            l1_issues
        );
    }

    // ── T-LINT-10: Clean workflow — all rules pass with complete contracts ──

    #[test]
    fn t_lint_10_clean_workflow() {
        let dto = WorkflowGraphDto {
            id: "kyc_flow".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "open_case".to_string(),
                    task_type: "open_case".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "check_sanctions".to_string(),
                    task_type: "check_sanctions".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "approve".to_string(),
                    task_type: "approve_case".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "reject".to_string(),
                    task_type: "reject_case".to_string(),
                    bpmn_id: None,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![
                EdgeDto {
                    from: "start".to_string(),
                    to: "open_case".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "open_case".to_string(),
                    to: "check_sanctions".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "check_sanctions".to_string(),
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "approve".to_string(),
                    condition: Some(FlagCondition {
                        flag: "sanctions_clear".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "reject".to_string(),
                    condition: None,
                    is_default: true,
                    on_error: None,
                },
                EdgeDto {
                    from: "approve".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "reject".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let reg = make_registry_with(vec![
            VerbContract {
                task_type: "open_case".to_string(),
                reads_flags: HashSet::new(),
                writes_flags: ["case_created".to_string()].into(),
                may_raise_errors: HashSet::new(),
                produces_correlation: vec![],
            },
            VerbContract {
                task_type: "check_sanctions".to_string(),
                reads_flags: ["case_created".to_string()].into(),
                writes_flags: ["sanctions_clear".to_string()].into(),
                may_raise_errors: ["SANCTIONS_HIT".to_string()].into(),
                produces_correlation: vec![],
            },
            VerbContract {
                task_type: "approve_case".to_string(),
                reads_flags: ["sanctions_clear".to_string()].into(),
                writes_flags: HashSet::new(),
                may_raise_errors: HashSet::new(),
                produces_correlation: vec![],
            },
            VerbContract {
                task_type: "reject_case".to_string(),
                reads_flags: HashSet::new(),
                writes_flags: HashSet::new(),
                may_raise_errors: HashSet::new(),
                produces_correlation: vec![],
            },
        ]);

        let diags = lint_contracts(&dto, &reg);
        // Allow only L5 warnings (unused "case_created" flag is only read, not in conditions)
        let non_l5: Vec<_> = diags.iter().filter(|d| d.rule != "L5").collect();
        assert!(
            non_l5.is_empty(),
            "Expected clean workflow (only L5 warnings allowed), got: {:?}",
            non_l5
        );
    }
}
