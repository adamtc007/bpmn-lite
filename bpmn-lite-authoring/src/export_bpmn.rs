use super::dto::*;
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{:02x}", b);
        acc
    })
}

/// Export a `WorkflowGraphDto` to BPMN 2.0 XML (Zeebe-compatible).
///
/// ## bpmn_id policy
/// - If `ServiceTask.bpmn_id` is `Some(id)`, use it.
/// - Otherwise: `sanitize_ncname(node_id) + "_" + sha256(node_id)[..4].hex()`.
/// - Never embed sensitive data; deterministic across re-runs.
///
/// ## XOR default
/// Set `<bpmn:exclusiveGateway default="...">` ONLY from edges with `is_default=true`.
/// Never infer default from edge ordering.
///
/// ## RaceWait
/// Exported as `<bpmn:eventBasedGateway>` + catch events per arm.
/// The existing `parser.rs` does NOT handle eventBasedGateway import, so
/// RaceWait round-trip (export → re-import) is deferred until parser support is added.
pub fn dto_to_bpmn_xml(dto: &WorkflowGraphDto) -> Result<String> {
    let process_id = sanitize_ncname(&dto.id);
    let bpmn_ids = compute_bpmn_ids(dto);

    // Collect outgoing edges per node
    let mut outgoing: HashMap<&str, Vec<&EdgeDto>> = HashMap::new();
    for edge in &dto.edges {
        outgoing.entry(edge.from.as_str()).or_default().push(edge);
    }

    // Find XOR default edge ids for the `default` attribute
    let mut xor_defaults: HashMap<&str, String> = HashMap::new();
    for edge in &dto.edges {
        if edge.is_default {
            let flow_id = seq_flow_id(&bpmn_ids, &edge.from, &edge.to);
            xor_defaults.insert(edge.from.as_str(), flow_id);
        }
    }

    // ── Topological layout for DI ──
    let topo = topo_layout(dto);

    let mut xml = String::new();

    // ── Header ──
    writeln!(xml, r#"<?xml version="1.0" encoding="UTF-8"?>"#)?;
    writeln!(
        xml,
        r#"<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL""#
    )?;
    writeln!(
        xml,
        r#"                  xmlns:bpmndi="http://www.omg.org/spec/BPMN/20100524/DI""#
    )?;
    writeln!(
        xml,
        r#"                  xmlns:dc="http://www.omg.org/spec/DD/20100524/DC""#
    )?;
    writeln!(
        xml,
        r#"                  xmlns:di="http://www.omg.org/spec/DD/20100524/DI""#
    )?;
    writeln!(
        xml,
        r#"                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0""#
    )?;
    writeln!(
        xml,
        r#"                  id="Definitions_1" targetNamespace="http://bpmn.io/schema/bpmn">"#
    )?;

    writeln!(
        xml,
        r#"  <bpmn:process id="{}" isExecutable="true">"#,
        process_id
    )?;

    // ── Elements ──
    for node in &dto.nodes {
        let bid = &bpmn_ids[node.id()];
        match node {
            NodeDto::Start { .. } => {
                writeln!(xml, r#"    <bpmn:startEvent id="{}" />"#, bid)?;
            }
            NodeDto::End { terminate, .. } => {
                if *terminate {
                    writeln!(xml, r#"    <bpmn:endEvent id="{}">"#, bid)?;
                    writeln!(xml, r#"      <bpmn:terminateEventDefinition />"#)?;
                    writeln!(xml, r#"    </bpmn:endEvent>"#)?;
                } else {
                    writeln!(xml, r#"    <bpmn:endEvent id="{}" />"#, bid)?;
                }
            }
            NodeDto::ServiceTask {
                task_type, bpmn_id, ..
            } => {
                let name_attr = bpmn_id
                    .as_deref()
                    .map(|n| format!(r#" name="{}""#, xml_escape(n)))
                    .unwrap_or_default();
                writeln!(
                    xml,
                    r#"    <bpmn:serviceTask id="{}"{}>
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="{}" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>"#,
                    bid, name_attr, task_type
                )?;
            }
            NodeDto::ExclusiveGateway { id } => {
                let default_attr = xor_defaults
                    .get(id.as_str())
                    .map(|fid| format!(r#" default="{}""#, fid))
                    .unwrap_or_default();
                writeln!(
                    xml,
                    r#"    <bpmn:exclusiveGateway id="{}"{}  />"#,
                    bid, default_attr
                )?;
            }
            NodeDto::ParallelGateway { direction, .. } => {
                let dir = gateway_dir_attr(direction);
                writeln!(xml, r#"    <bpmn:parallelGateway id="{}" {} />"#, bid, dir)?;
            }
            NodeDto::InclusiveGateway { direction, .. } => {
                let dir = gateway_dir_attr(direction);
                writeln!(xml, r#"    <bpmn:inclusiveGateway id="{}" {} />"#, bid, dir)?;
            }
            NodeDto::TimerWait {
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
                ..
            } => {
                writeln!(xml, r#"    <bpmn:intermediateCatchEvent id="{}">"#, bid)?;
                writeln!(xml, r#"      <bpmn:timerEventDefinition>"#)?;
                write_timer_child(&mut xml, *duration_ms, *deadline_ms, *cycle_ms, *cycle_max)?;
                writeln!(xml, r#"      </bpmn:timerEventDefinition>"#)?;
                writeln!(xml, r#"    </bpmn:intermediateCatchEvent>"#)?;
            }
            NodeDto::MessageWait { name, .. } => {
                writeln!(
                    xml,
                    r#"    <bpmn:intermediateCatchEvent id="{}" name="{}">
      <bpmn:messageEventDefinition />
    </bpmn:intermediateCatchEvent>"#,
                    bid,
                    xml_escape(name)
                )?;
            }
            NodeDto::HumanWait { task_kind, .. } => {
                writeln!(
                    xml,
                    r#"    <bpmn:intermediateCatchEvent id="{}" name="human.{}">
      <bpmn:messageEventDefinition />
    </bpmn:intermediateCatchEvent>"#,
                    bid,
                    xml_escape(task_kind)
                )?;
            }
            NodeDto::RaceWait { arms, .. } => {
                // Export as eventBasedGateway + catch events per arm.
                // NOTE: parser.rs does NOT handle eventBasedGateway import.
                writeln!(xml, r#"    <bpmn:eventBasedGateway id="{}" />"#, bid)?;
                for arm in arms {
                    let arm_bid = format!("{}_{}", bid, sanitize_ncname(&arm.arm_id));
                    match &arm.kind {
                        RaceArmKind::Timer { duration_ms, .. } => {
                            writeln!(
                                xml,
                                r#"    <bpmn:intermediateCatchEvent id="{}">
      <bpmn:timerEventDefinition>
        <bpmn:timeDuration>{}</bpmn:timeDuration>
      </bpmn:timerEventDefinition>
    </bpmn:intermediateCatchEvent>"#,
                                arm_bid,
                                ms_to_iso_duration(*duration_ms)
                            )?;
                        }
                        RaceArmKind::Message { name, .. } => {
                            writeln!(
                                xml,
                                r#"    <bpmn:intermediateCatchEvent id="{}" name="{}">
      <bpmn:messageEventDefinition />
    </bpmn:intermediateCatchEvent>"#,
                                arm_bid,
                                xml_escape(name)
                            )?;
                        }
                    }
                }
            }
            NodeDto::BoundaryTimer {
                host,
                duration_ms,
                deadline_ms,
                cycle_ms,
                cycle_max,
                interrupting,
                ..
            } => {
                let host_bid = &bpmn_ids[host.as_str()];
                let cancel = if *interrupting { "true" } else { "false" };
                writeln!(
                    xml,
                    r#"    <bpmn:boundaryEvent id="{}" attachedToRef="{}" cancelActivity="{}">
      <bpmn:timerEventDefinition>"#,
                    bid, host_bid, cancel
                )?;
                write_timer_child(&mut xml, *duration_ms, *deadline_ms, *cycle_ms, *cycle_max)?;
                writeln!(xml, r#"      </bpmn:timerEventDefinition>"#)?;
                writeln!(xml, r#"    </bpmn:boundaryEvent>"#)?;
            }
            NodeDto::BoundaryError {
                host, error_code, ..
            } => {
                let host_bid = &bpmn_ids[host.as_str()];
                let err_attr = error_code
                    .as_deref()
                    .map(|c| format!(r#" errorRef="{}""#, xml_escape(c)))
                    .unwrap_or_default();
                writeln!(
                    xml,
                    r#"    <bpmn:boundaryEvent id="{}" attachedToRef="{}">
      <bpmn:errorEventDefinition{} />
    </bpmn:boundaryEvent>"#,
                    bid, host_bid, err_attr
                )?;
            }
        }
    }

    // ── Sequence flows ──
    for edge in &dto.edges {
        let from_bid = resolve_edge_bpmn_id(&edge.from, &bpmn_ids);
        let to_bid = resolve_edge_bpmn_id(&edge.to, &bpmn_ids);
        let flow_id = seq_flow_id(&bpmn_ids, &edge.from, &edge.to);

        if let Some(cond) = &edge.condition {
            writeln!(
                xml,
                r#"    <bpmn:sequenceFlow id="{}" sourceRef="{}" targetRef="{}">
      <bpmn:conditionExpression>{}</bpmn:conditionExpression>
    </bpmn:sequenceFlow>"#,
                flow_id,
                from_bid,
                to_bid,
                flag_condition_to_feel(cond)
            )?;
        } else if let Some(on_err) = &edge.on_error {
            // Error edges: comment that this is an error route
            writeln!(
                xml,
                r#"    <!-- Error route: {} on {} -->
    <bpmn:sequenceFlow id="{}" sourceRef="{}" targetRef="{}" />"#,
                on_err.error_code, edge.from, flow_id, from_bid, to_bid
            )?;
        } else {
            writeln!(
                xml,
                r#"    <bpmn:sequenceFlow id="{}" sourceRef="{}" targetRef="{}" />"#,
                flow_id, from_bid, to_bid
            )?;
        }
    }

    writeln!(xml, r#"  </bpmn:process>"#)?;

    // ── BPMN DI ──
    writeln!(xml, r#"  <bpmndi:BPMNDiagram id="BPMNDiagram_1">"#)?;
    writeln!(
        xml,
        r#"    <bpmndi:BPMNPlane id="BPMNPlane_1" bpmnElement="{}">"#,
        process_id
    )?;

    // Shapes
    for node in &dto.nodes {
        let bid = &bpmn_ids[node.id()];
        let (x, y) = topo.get(node.id()).copied().unwrap_or((0.0, 0.0));
        let (w, h) = shape_size(node);
        writeln!(
            xml,
            r#"      <bpmndi:BPMNShape id="{}_di" bpmnElement="{}">
        <dc:Bounds x="{:.0}" y="{:.0}" width="{:.0}" height="{:.0}" />
      </bpmndi:BPMNShape>"#,
            bid, bid, x, y, w, h
        )?;
    }

    // Edges (2-waypoint horizontal)
    for edge in &dto.edges {
        let from_bid = resolve_edge_bpmn_id(&edge.from, &bpmn_ids);
        let to_bid = resolve_edge_bpmn_id(&edge.to, &bpmn_ids);
        let flow_id = seq_flow_id(&bpmn_ids, &edge.from, &edge.to);

        let from_base = edge.from.split('.').next().unwrap_or(&edge.from);
        let to_base = edge.to.split('.').next().unwrap_or(&edge.to);
        let (x1, y1) = topo.get(from_base).copied().unwrap_or((0.0, 0.0));
        let (x2, y2) = topo.get(to_base).copied().unwrap_or((200.0, 0.0));

        writeln!(
            xml,
            r#"      <bpmndi:BPMNEdge id="{}_di" bpmnElement="{}">
        <di:waypoint x="{:.0}" y="{:.0}" />
        <di:waypoint x="{:.0}" y="{:.0}" />
      </bpmndi:BPMNEdge>"#,
            flow_id,
            flow_id,
            x1 + 50.0,
            y1 + 20.0,
            x2,
            y2 + 20.0
        )?;
        // Suppress unused variable warnings for source/target bpmn_ids
        let _ = (&from_bid, &to_bid);
    }

    writeln!(xml, r#"    </bpmndi:BPMNPlane>"#)?;
    writeln!(xml, r#"  </bpmndi:BPMNDiagram>"#)?;
    writeln!(xml, r#"</bpmn:definitions>"#)?;

    Ok(xml)
}

/// Convert a `FlagCondition` to a FEEL expression string.
///
/// Format: `= {flag} {op} {value}`
pub fn flag_condition_to_feel(cond: &FlagCondition) -> String {
    let op_str = match cond.op {
        FlagOp::Eq => "==",
        FlagOp::Neq => "!=",
        FlagOp::Lt => "<",
        FlagOp::Gt => ">",
    };
    let val_str = match &cond.value {
        FlagValue::Bool(b) => b.to_string(),
        FlagValue::I64(i) => i.to_string(),
    };
    format!("= {} {} {}", cond.flag, op_str, val_str)
}

/// Convert milliseconds to ISO 8601 duration.
///
/// Examples: 5000 → "PT5S", 90000 → "PT1M30S", 86400000 → "P1D"
pub fn ms_to_iso_duration(ms: u64) -> String {
    if ms == 0 {
        return "PT0S".to_string();
    }

    let total_seconds = ms / 1000;
    let days = total_seconds / 86400;
    let remaining = total_seconds % 86400;
    let hours = remaining / 3600;
    let remaining = remaining % 3600;
    let minutes = remaining / 60;
    let seconds = remaining % 60;

    let mut result = String::from("P");
    if days > 0 {
        write!(result, "{}D", days).unwrap();
    }
    if hours > 0 || minutes > 0 || seconds > 0 {
        result.push('T');
        if hours > 0 {
            write!(result, "{}H", hours).unwrap();
        }
        if minutes > 0 {
            write!(result, "{}M", minutes).unwrap();
        }
        if seconds > 0 {
            write!(result, "{}S", seconds).unwrap();
        }
    }
    result
}

// ── Internal helpers ──

fn compute_bpmn_ids(dto: &WorkflowGraphDto) -> HashMap<String, String> {
    let mut ids = HashMap::new();
    for node in &dto.nodes {
        let bid = match node {
            NodeDto::ServiceTask {
                bpmn_id: Some(name),
                ..
            } => sanitize_ncname(name),
            _ => {
                let hash = short_hash(node.id());
                format!("{}_{}", sanitize_ncname(node.id()), hash)
            }
        };
        ids.insert(node.id().to_string(), bid);
    }
    ids
}

/// Sanitize a string to be a valid XML NCName: start with letter or underscore,
/// then alphanumeric, underscore, hyphen, or period.
fn sanitize_ncname(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for (i, ch) in s.chars().enumerate() {
        if i == 0 {
            if ch.is_ascii_alphabetic() || ch == '_' {
                result.push(ch);
            } else {
                result.push('_');
                if ch.is_ascii_alphanumeric() {
                    result.push(ch);
                }
            }
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    if result.is_empty() {
        result.push_str("_id");
    }
    result
}

/// First 4 bytes (8 hex chars) of SHA-256 hash — deterministic.
fn short_hash(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    hex_encode(&result[..4])
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn gateway_dir_attr(dir: &bpmn_lite_compiler::ir::GatewayDirection) -> &'static str {
    match dir {
        bpmn_lite_compiler::ir::GatewayDirection::Diverging => r#"gatewayDirection="Diverging""#,
        bpmn_lite_compiler::ir::GatewayDirection::Converging => r#"gatewayDirection="Converging""#,
    }
}

fn seq_flow_id(bpmn_ids: &HashMap<String, String>, from: &str, to: &str) -> String {
    let from_bid = resolve_edge_bpmn_id(from, bpmn_ids);
    let to_bid = resolve_edge_bpmn_id(to, bpmn_ids);
    format!("flow_{}_to_{}", from_bid, to_bid)
}

/// Resolve an edge endpoint to its bpmn_id. Handles dot-notation (race.arm → race bpmn_id).
fn resolve_edge_bpmn_id(endpoint: &str, bpmn_ids: &HashMap<String, String>) -> String {
    if let Some(bid) = bpmn_ids.get(endpoint) {
        return bid.clone();
    }
    // Dot-notation: "race.arm_id" → look up "race"
    if let Some((base, _)) = endpoint.split_once('.') {
        if let Some(bid) = bpmn_ids.get(base) {
            return bid.clone();
        }
    }
    sanitize_ncname(endpoint)
}

fn write_timer_child(
    xml: &mut String,
    duration_ms: Option<u64>,
    deadline_ms: Option<u64>,
    cycle_ms: Option<u64>,
    cycle_max: Option<u32>,
) -> Result<()> {
    if let Some(ms) = duration_ms {
        writeln!(
            xml,
            r#"        <bpmn:timeDuration>{}</bpmn:timeDuration>"#,
            ms_to_iso_duration(ms)
        )?;
    } else if let Some(ms) = deadline_ms {
        writeln!(xml, r#"        <bpmn:timeDate>{}</bpmn:timeDate>"#, ms)?;
    } else if let Some(ms) = cycle_ms {
        let max = cycle_max.unwrap_or(1);
        writeln!(
            xml,
            r#"        <bpmn:timeCycle>R{}/{}</bpmn:timeCycle>"#,
            max,
            ms_to_iso_duration(ms)
        )?;
    }
    Ok(())
}

/// Simple topological left-to-right layout.
/// X = topo_rank * 200, Y = rank_index * 100.
fn topo_layout(dto: &WorkflowGraphDto) -> HashMap<String, (f64, f64)> {
    // Build adjacency
    let mut successors: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for node in &dto.nodes {
        in_degree.entry(node.id()).or_insert(0);
    }
    for edge in &dto.edges {
        let from_base = edge.from.split('.').next().unwrap_or(&edge.from);
        let to_base = edge.to.split('.').next().unwrap_or(&edge.to);
        successors.entry(from_base).or_default().push(to_base);
        *in_degree.entry(to_base).or_insert(0) += 1;
    }

    // Kahn's algorithm for topological order
    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(&id, _)| id)
        .collect();
    queue.sort(); // deterministic order

    let mut rank: HashMap<&str, usize> = HashMap::new();
    while let Some(node) = queue.first().copied() {
        queue.remove(0);
        let r = rank.get(node).copied().unwrap_or(0);
        if let Some(succs) = successors.get(node) {
            for &s in succs {
                let new_rank = r + 1;
                let entry = rank.entry(s).or_insert(0);
                if new_rank > *entry {
                    *entry = new_rank;
                }
                if let Some(deg) = in_degree.get_mut(s) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        queue.push(s);
                        queue.sort();
                    }
                }
            }
        }
        rank.entry(node).or_insert(r);
    }

    // Assign coordinates
    let mut rank_counts: HashMap<usize, usize> = HashMap::new();
    let mut positions = HashMap::new();
    // Sort by rank for deterministic Y assignment
    let mut items: Vec<(&str, usize)> = rank.iter().map(|(&id, &r)| (id, r)).collect();
    items.sort_by_key(|(id, r)| (*r, *id));
    for (id, r) in &items {
        let y_idx = rank_counts.entry(*r).or_insert(0);
        let x = (*r as f64) * 200.0;
        let y = (*y_idx as f64) * 100.0;
        positions.insert(id.to_string(), (x, y));
        *rank_counts.get_mut(r).unwrap() += 1;
    }

    positions
}

fn shape_size(node: &NodeDto) -> (f64, f64) {
    match node {
        NodeDto::Start { .. } | NodeDto::End { .. } => (36.0, 36.0),
        NodeDto::ExclusiveGateway { .. }
        | NodeDto::ParallelGateway { .. }
        | NodeDto::InclusiveGateway { .. } => (50.0, 50.0),
        _ => (100.0, 80.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn minimal_dto() -> WorkflowGraphDto {
        WorkflowGraphDto {
            id: "test_wf".to_string(),
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
        }
    }

    /// T-EXP-1: Start→Task→End exports valid BPMN XML
    #[test]
    fn t_exp_1_basic_export() {
        let dto = minimal_dto();
        let xml = dto_to_bpmn_xml(&dto).unwrap();
        assert!(xml.contains("<bpmn:startEvent"));
        assert!(xml.contains("<bpmn:serviceTask"));
        assert!(xml.contains("<bpmn:endEvent"));
        assert!(xml.contains("<bpmn:sequenceFlow"));
        assert!(xml.contains(r#"type="do_work""#));
        assert!(xml.contains("bpmndi:BPMNShape"));
        assert!(xml.contains("bpmndi:BPMNEdge"));
    }

    /// T-EXP-2: XOR `default` attr only from `is_default=true` edge
    #[test]
    fn t_exp_2_xor_default_from_is_default() {
        let dto = WorkflowGraphDto {
            id: "test".to_string(),
            meta: None,
            nodes: vec![
                NodeDto::Start {
                    id: "start".to_string(),
                },
                NodeDto::ExclusiveGateway {
                    id: "xor".to_string(),
                },
                NodeDto::ServiceTask {
                    id: "a".to_string(),
                    task_type: "do_a".to_string(),
                    bpmn_id: None,
                },
                NodeDto::ServiceTask {
                    id: "b".to_string(),
                    task_type: "do_b".to_string(),
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
                    to: "xor".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "a".to_string(),
                    condition: Some(FlagCondition {
                        flag: "x".to_string(),
                        op: FlagOp::Eq,
                        value: FlagValue::Bool(true),
                    }),
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "xor".to_string(),
                    to: "b".to_string(),
                    condition: None,
                    is_default: true,
                    on_error: None,
                },
                EdgeDto {
                    from: "a".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
                EdgeDto {
                    from: "b".to_string(),
                    to: "end".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let xml = dto_to_bpmn_xml(&dto).unwrap();
        // XOR gateway should have default= attribute
        assert!(
            xml.contains(r#"exclusiveGateway"#),
            "Should have exclusiveGateway"
        );
        assert!(
            xml.contains(r#"default="#),
            "Should have default attribute on XOR gateway"
        );
        // The condition edge should have a FEEL expression
        assert!(xml.contains("= x == true"));
    }

    /// T-EXP-3: Timer ms → ISO 8601 (5000 → PT5S)
    #[test]
    fn t_exp_3_timer_iso_duration() {
        assert_eq!(ms_to_iso_duration(5000), "PT5S");
        assert_eq!(ms_to_iso_duration(90000), "PT1M30S");
        assert_eq!(ms_to_iso_duration(86400000), "P1D");
        assert_eq!(ms_to_iso_duration(3600000), "PT1H");
        assert_eq!(ms_to_iso_duration(0), "PT0S");
    }

    /// T-EXP-4: FlagCondition → FEEL expression
    #[test]
    fn t_exp_4_condition_to_feel() {
        let cond = FlagCondition {
            flag: "sanctions_clear".to_string(),
            op: FlagOp::Eq,
            value: FlagValue::Bool(true),
        };
        assert_eq!(flag_condition_to_feel(&cond), "= sanctions_clear == true");

        let cond2 = FlagCondition {
            flag: "count".to_string(),
            op: FlagOp::Gt,
            value: FlagValue::I64(5),
        };
        assert_eq!(flag_condition_to_feel(&cond2), "= count > 5");
    }

    /// T-EXP-5: bpmn_id stable across multiple calls
    #[test]
    fn t_exp_5_bpmn_id_deterministic() {
        let dto = minimal_dto();
        let xml1 = dto_to_bpmn_xml(&dto).unwrap();
        let xml2 = dto_to_bpmn_xml(&dto).unwrap();
        assert_eq!(xml1, xml2, "BPMN XML should be deterministic across calls");
    }

    /// T-EXP-6: BoundaryTimer exports with attachedToRef + cancelActivity
    #[test]
    fn t_exp_6_boundary_timer_export() {
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
                NodeDto::BoundaryTimer {
                    id: "bt".to_string(),
                    host: "task_a".to_string(),
                    duration_ms: Some(30000),
                    deadline_ms: None,
                    cycle_ms: None,
                    cycle_max: None,
                    interrupting: false,
                },
                NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
                NodeDto::End {
                    id: "end_timeout".to_string(),
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
                    from: "bt".to_string(),
                    to: "end_timeout".to_string(),
                    condition: None,
                    is_default: false,
                    on_error: None,
                },
            ],
        };

        let xml = dto_to_bpmn_xml(&dto).unwrap();
        assert!(xml.contains("boundaryEvent"), "Should have boundaryEvent");
        assert!(xml.contains("attachedToRef="), "Should have attachedToRef");
        assert!(
            xml.contains(r#"cancelActivity="false""#),
            "Should have cancelActivity=false for non-interrupting"
        );
        assert!(xml.contains("PT30S"), "30000ms = PT30S");
    }

    /// T-EXP-7: Round-trip: DTO → BPMN → parse → IR → ir_to_dto → compare structure.
    /// Excludes RaceWait workflows (parser.rs doesn't handle eventBasedGateway).
    #[test]
    fn t_exp_7_round_trip() {
        let dto = minimal_dto();
        let xml = dto_to_bpmn_xml(&dto).unwrap();

        // Parse back to IR
        let ir = bpmn_lite_compiler::parser::parse_bpmn(&xml).unwrap();
        // Verify IR is valid
        let errors = bpmn_lite_compiler::verifier::verify(&ir);
        assert!(
            errors.is_empty(),
            "Round-trip IR should verify: {:?}",
            errors
        );

        // IR → DTO via ir_to_dto
        let dto2 = super::super::ir_to_dto::ir_to_dto(&ir, "test_wf").unwrap();

        // Compare structure: same number of nodes and edges
        assert_eq!(
            dto.nodes.len(),
            dto2.nodes.len(),
            "Node count mismatch after round-trip"
        );
        assert_eq!(
            dto.edges.len(),
            dto2.edges.len(),
            "Edge count mismatch after round-trip"
        );

        // Verify start/end/service nodes exist
        let has_start = dto2
            .nodes
            .iter()
            .any(|n| matches!(n, NodeDto::Start { .. }));
        let has_end = dto2.nodes.iter().any(|n| matches!(n, NodeDto::End { .. }));
        let has_service = dto2
            .nodes
            .iter()
            .any(|n| matches!(n, NodeDto::ServiceTask { .. }));
        assert!(has_start, "Round-trip should preserve Start");
        assert!(has_end, "Round-trip should preserve End");
        assert!(has_service, "Round-trip should preserve ServiceTask");
    }
}
