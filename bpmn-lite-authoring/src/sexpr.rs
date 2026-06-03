use crate::dto::{WorkflowGraphDto, NodeDto, EdgeDto, FlagValue};
use anyhow::{anyhow, Result};
use std::collections::HashMap;

/// Convert a parsed WorkflowGraphDto into a BPMN-DSL S-Expression workbook.
/// Appends warnings to `diagnostics` if unsafe permissive compilation is used.
pub fn dto_to_sexpr(dto: &WorkflowGraphDto, permissive: bool, diagnostics: &mut Vec<String>) -> Result<String> {
    let mut out = String::new();
    let wf_id = dto.id.replace("_", "-");
    out.push_str(&format!("(workflow {}\n", wf_id));

    // Group edges by their source node for easy lookup
    let mut outgoing: HashMap<String, Vec<&EdgeDto>> = HashMap::new();
    for edge in &dto.edges {
        outgoing.entry(edge.from.clone()).or_default().push(edge);
    }

    for node in &dto.nodes {
        out.push_str("  ");
        match node {
            NodeDto::Start { id } => {
                let next = outgoing.get(id)
                    .and_then(|e| e.first())
                    .map(|e| e.to.as_str())
                    .unwrap_or("end");
                out.push_str(&format!("(start-event :id {} :next {})\n", id, next));
            }
            NodeDto::End { id, terminate } => {
                let status = if *terminate { "\"terminated\"" } else { "\"completed\"" };
                out.push_str(&format!("(end-event :id {} :status {})\n", id, status));
            }
            NodeDto::ServiceTask { id, task_type, .. } => {
                let next = outgoing.get(id)
                    .and_then(|e| e.first())
                    .map(|e| e.to.as_str())
                    .unwrap_or("end");
                
                if task_type.starts_with("dmn-lite:") {
                    let decision = task_type.trim_start_matches("dmn-lite:");
                    out.push_str(&format!("(business-rule-task :id {} :decision {} :next {})\n", id, decision, next));
                } else {
                    out.push_str(&format!("(service-task :id {} :verb {} :next {})\n", id, task_type, next));
                }
            }
            NodeDto::TimerWait { id, duration_ms, .. } => {
                let next = outgoing.get(id)
                    .and_then(|e| e.first())
                    .map(|e| e.to.as_str())
                    .unwrap_or("end");
                let duration_str = duration_ms.map(|ms| format!("PT{}M", ms / 60000)).unwrap_or_else(|| "PT15M".to_string());
                out.push_str(&format!("(task :id {} :plug bpmn:timer-wait :args (:duration \"{}\") :next {})\n", id, duration_str, next));
            }
            NodeDto::MessageWait { id, name, .. } => {
                let next = outgoing.get(id)
                    .and_then(|e| e.first())
                    .map(|e| e.to.as_str())
                    .unwrap_or("end");
                out.push_str(&format!("(task :id {} :plug bpmn:message-wait :args (:message_name \"{}\") :next {})\n", id, name, next));
            }
            NodeDto::ExclusiveGateway { id } => {
                out.push_str(&format!("(exclusive-gateway :id {}\n", id));
                if let Some(edges) = outgoing.get(id) {
                    for edge in edges {
                        if edge.is_default {
                            out.push_str(&format!("      (flow :default :next {})\n", edge.to));
                        } else {
                            let cond_str = if let Some(cond) = &edge.condition {
                                let val_str = match &cond.value {
                                    FlagValue::Bool(b) => b.to_string(),
                                    FlagValue::I64(i) => i.to_string(),
                                };
                                format!("(= @{} \"{}\")", cond.flag, val_str)
                            } else {
                                if permissive {
                                    diagnostics.push(format!("Unparsed FEEL/missing condition at Edge from {} to {}, using fallback", edge.from, edge.to));
                                    "(= @feel_eval_warning \"unparsed_expression\")".to_string()
                                } else {
                                    return Err(anyhow!("Missing condition at Edge from {} to {}", edge.from, edge.to));
                                }
                            };
                            out.push_str(&format!("      (flow :condition {} :next {})\n", cond_str, edge.to));
                        }
                    }
                }
                out.push_str("    )\n");
            }
            NodeDto::ParallelGateway { id, direction, join } => {
                if *direction == bpmn_lite_compiler::ir::GatewayDirection::Diverging {
                    let join_id = match join.as_deref() {
                        Some(j) => j.to_string(),
                        None => {
                            if permissive {
                                diagnostics.push(format!("Parallel split Gateway '{}' is missing join linkage, pairing with '{}-join' as fallback", id, id));
                                format!("{}-join", id)
                            } else {
                                return Err(anyhow!("Parallel split Gateway '{}' is missing join linkage", id));
                            }
                        }
                    };
                    out.push_str(&format!("(split-and :id {} :join {}\n", id, join_id));
                    if let Some(edges) = outgoing.get(id) {
                        for edge in edges {
                            out.push_str(&format!("      (flow :next {})\n", edge.to));
                        }
                    }
                    out.push_str("    )\n");
                } else {
                    let split_id = match join.as_deref() {
                        Some(s) => s.to_string(),
                        None => {
                            if permissive {
                                diagnostics.push(format!("Parallel join Gateway '{}' is missing split linkage, pairing with '{}-split' as fallback", id, id));
                                format!("{}-split", id)
                            } else {
                                return Err(anyhow!("Parallel join Gateway '{}' is missing split linkage", id));
                            }
                        }
                    };
                    let next = outgoing.get(id)
                        .and_then(|e| e.first())
                        .map(|e| e.to.as_str())
                        .unwrap_or("end");
                    out.push_str(&format!("(join-and :id {} :split {} :next {})\n", id, split_id, next));
                }
            }
            NodeDto::InclusiveGateway { id, direction, join } => {
                if *direction == bpmn_lite_compiler::ir::GatewayDirection::Diverging {
                    let join_id = match join.as_deref() {
                        Some(j) => j.to_string(),
                        None => {
                            if permissive {
                                diagnostics.push(format!("Inclusive split Gateway '{}' is missing join linkage, pairing with '{}-join' as fallback", id, id));
                                format!("{}-join", id)
                            } else {
                                return Err(anyhow!("Inclusive split Gateway '{}' is missing join linkage", id));
                            }
                        }
                    };
                    out.push_str(&format!("(split-or :id {} :join {}\n", id, join_id));
                    if let Some(edges) = outgoing.get(id) {
                        for edge in edges {
                            out.push_str(&format!("      (flow :next {})\n", edge.to));
                        }
                    }
                    out.push_str("    )\n");
                } else {
                    let split_id = match join.as_deref() {
                        Some(s) => s.to_string(),
                        None => {
                            if permissive {
                                diagnostics.push(format!("Inclusive join Gateway '{}' is missing split linkage, pairing with '{}-split' as fallback", id, id));
                                format!("{}-split", id)
                            } else {
                                return Err(anyhow!("Inclusive join Gateway '{}' is missing split linkage", id));
                            }
                        }
                    };
                    let next = outgoing.get(id)
                        .and_then(|e| e.first())
                        .map(|e| e.to.as_str())
                        .unwrap_or("end");
                    out.push_str(&format!("(join-or :id {} :split {} :next {})\n", id, split_id, next));
                }
            }
            NodeDto::BoundaryTimer { id, .. } => {
                return Err(anyhow!("Unsupported variant in S-Expression serialization: BoundaryTimer {}", id));
            }
            NodeDto::BoundaryError { id, .. } => {
                return Err(anyhow!("Unsupported variant in S-Expression serialization: BoundaryError {}", id));
            }
            other => {
                if permissive {
                    let kind_str = "UnsupportedNode";
                    diagnostics.push(format!("Bypassing unsupported node kind {} as unsafe placeholder (BPMN_BOUNDARY_EVENT_BYPASS)", kind_str));
                    let next = outgoing.get(other.id())
                        .and_then(|e| e.first())
                        .map(|e| e.to.as_str())
                        .unwrap_or("end");
                    out.push_str(&format!("(task :id {} :plug bpmn:unsafe-placeholder :args (:original_kind \"{:?}\") :next {})\n", other.id(), other, next));
                } else {
                    return Err(anyhow!("Unsupported variant in S-Expression serialization: {:?}", other));
                }
            }
        }
    }

    out.push_str(")\n");
    Ok(out)
}
