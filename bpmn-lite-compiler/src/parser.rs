use crate::ir::*;
use anyhow::{anyhow, Context, Result};
use bpmn_lite_types::ffi_bindings::{DataObjectRole, DataObjectType, PrimitiveType};
use petgraph::graph::NodeIndex;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use std::collections::HashMap;
use uuid::Uuid;

const MAX_TIMER_MS: u64 = 366 * 24 * 60 * 60 * 1_000;
const MAX_TIMER_CYCLE_FIRES: u32 = 1_000;

/// Parse BPMN 2.0 XML into an IRGraph.
///
/// Accepts both prefixed (`bpmn:startEvent`) and default-namespace (`startEvent`) forms.
/// Only elements in the canonical mapping table are accepted; all others produce a compile error.
pub fn parse_bpmn(xml: &str) -> Result<IRGraph> {
    let mut reader = Reader::from_str(xml);

    let mut graph = IRGraph::new();
    let mut node_map: HashMap<String, NodeIndex> = HashMap::new();

    // Sequence flows collected to add as edges after all nodes are inserted
    let mut flows: Vec<SequenceFlowRaw> = Vec::new();

    // Parser state
    let mut in_process = false;
    let mut current_element: Option<ElementContext> = None;
    let mut extension_task_type: Option<String> = None;
    let mut extension_corr_key: Option<String> = None;
    let mut condition_text: Option<String> = None;
    let mut in_extension_elements = false;
    let mut in_condition_expression = false;
    let mut sub_event_type: Option<SubEventType> = None;
    let mut timer_text: Option<String> = None;
    let mut timer_kind: Option<TimerKind> = None;
    let mut in_timer_child = false;

    // Error definitions: <bpmn:error id="X" errorCode="Y"/> → id → errorCode
    let mut error_defs: HashMap<String, String> = HashMap::new();

    // FFI service task state — accumulated while parsing the serviceTask's
    // <bpmn:taskDefinition implementation="..."> block.
    let mut ffi_template_id: Option<[u8; 32]> = None;
    let mut ffi_inputs: Vec<FfiInputBinding> = Vec::new();
    let mut ffi_outputs: Vec<FfiOutputBinding> = Vec::new();
    let mut in_ffi_task_definition: bool = false;

    // Data object type declaration — accumulated while parsing
    // <bpmn:dataObject>...<bpmn:dataType .../>...</bpmn:dataObject>.
    let mut data_object_type_decl: Option<(DataObjectType, DataObjectRole)> = None;

    let mut buf = Vec::new();

    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(ref e)) => {
                handle_open_tag(
                    e,
                    false,
                    &mut graph,
                    &mut node_map,
                    &mut flows,
                    &mut in_process,
                    &mut current_element,
                    &mut extension_task_type,
                    &mut extension_corr_key,
                    &mut condition_text,
                    &mut in_extension_elements,
                    &mut in_condition_expression,
                    &mut sub_event_type,
                    &mut timer_text,
                    &mut timer_kind,
                    &mut in_timer_child,
                    &mut error_defs,
                    &mut ffi_template_id,
                    &mut ffi_inputs,
                    &mut ffi_outputs,
                    &mut in_ffi_task_definition,
                    &mut data_object_type_decl,
                )?;
            }
            Ok(Event::Empty(ref e)) => {
                handle_open_tag(
                    e,
                    true,
                    &mut graph,
                    &mut node_map,
                    &mut flows,
                    &mut in_process,
                    &mut current_element,
                    &mut extension_task_type,
                    &mut extension_corr_key,
                    &mut condition_text,
                    &mut in_extension_elements,
                    &mut in_condition_expression,
                    &mut sub_event_type,
                    &mut timer_text,
                    &mut timer_kind,
                    &mut in_timer_child,
                    &mut error_defs,
                    &mut ffi_template_id,
                    &mut ffi_inputs,
                    &mut ffi_outputs,
                    &mut in_ffi_task_definition,
                    &mut data_object_type_decl,
                )?;
            }
            Ok(Event::End(ref e)) => {
                let local = local_name(e.name().as_ref());
                handle_close_tag(
                    &local,
                    &mut graph,
                    &mut node_map,
                    &mut flows,
                    &mut in_process,
                    &mut current_element,
                    &mut extension_task_type,
                    &mut extension_corr_key,
                    &mut condition_text,
                    &mut in_extension_elements,
                    &mut in_condition_expression,
                    &mut sub_event_type,
                    &mut timer_text,
                    &mut timer_kind,
                    &mut in_timer_child,
                    &error_defs,
                    &mut ffi_template_id,
                    &mut ffi_inputs,
                    &mut ffi_outputs,
                    &mut in_ffi_task_definition,
                    &mut data_object_type_decl,
                )?;
            }
            Ok(Event::Text(ref e)) => {
                if let Ok(text) = e.unescape() {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        if in_condition_expression {
                            condition_text = Some(text.clone());
                        }
                        if in_timer_child {
                            timer_text = Some(text);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    // Add edges from collected sequence flows
    for flow in flows {
        let from = node_map.get(&flow.source).ok_or_else(|| {
            anyhow!(
                "sequenceFlow '{}' references unknown sourceRef '{}'",
                flow.id,
                flow.source
            )
        })?;
        let to = node_map.get(&flow.target).ok_or_else(|| {
            anyhow!(
                "sequenceFlow '{}' references unknown targetRef '{}'",
                flow.id,
                flow.target
            )
        })?;
        graph.add_edge(
            *from,
            *to,
            IREdge {
                id: flow.id,
                condition: flow.condition,
            },
        );
    }

    Ok(graph)
}

// ─── Tag handlers ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn handle_open_tag(
    e: &BytesStart,
    is_empty: bool,
    graph: &mut IRGraph,
    node_map: &mut HashMap<String, NodeIndex>,
    flows: &mut Vec<SequenceFlowRaw>,
    in_process: &mut bool,
    current_element: &mut Option<ElementContext>,
    extension_task_type: &mut Option<String>,
    extension_corr_key: &mut Option<String>,
    condition_text: &mut Option<String>,
    in_extension_elements: &mut bool,
    in_condition_expression: &mut bool,
    sub_event_type: &mut Option<SubEventType>,
    timer_text: &mut Option<String>,
    timer_kind: &mut Option<TimerKind>,
    in_timer_child: &mut bool,
    error_defs: &mut HashMap<String, String>,
    ffi_template_id: &mut Option<[u8; 32]>,
    ffi_inputs: &mut Vec<FfiInputBinding>,
    ffi_outputs: &mut Vec<FfiOutputBinding>,
    in_ffi_task_definition: &mut bool,
    data_object_type_decl: &mut Option<(DataObjectType, DataObjectRole)>,
) -> Result<()> {
    let local = local_name(e.name().as_ref());

    match local.as_str() {
        // Error definitions at definitions level: <bpmn:error id="X" errorCode="Y"/>
        "error" if !*in_process => {
            if let (Ok(id), Some(code)) = (get_attr(e, "id"), get_attr_opt(e, "errorCode")) {
                error_defs.insert(id, code);
            }
        }
        "process" => {
            *in_process = true;
        }
        "startEvent" if *in_process => {
            let id = get_attr(e, "id")?;
            let idx = graph.add_node(IRNode::Start { id: id.clone() });
            node_map.insert(id, idx);
        }
        "endEvent" if *in_process => {
            let id = get_attr(e, "id")?;
            if is_empty {
                // Self-closing endEvent — no children, so no terminateEventDefinition
                let idx = graph.add_node(IRNode::End {
                    id: id.clone(),
                    terminate: false,
                });
                node_map.insert(id, idx);
            } else {
                *current_element = Some(ElementContext::EndEvent { id });
                *sub_event_type = None;
            }
        }
        "serviceTask" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            if is_empty {
                // Self-closing: no extensions possible, use name fallback
                let task_type = name_to_snake(&name);
                let idx = graph.add_node(IRNode::ServiceTask {
                    id: id.clone(),
                    name,
                    task_type,
                });
                node_map.insert(id, idx);
            } else {
                *current_element = Some(ElementContext::ServiceTask { id, name });
                *extension_task_type = None;
            }
        }
        "dataObject" if *in_process && !is_empty => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_else(|| id.clone());
            *current_element = Some(ElementContext::DataObject { id, name });
            *data_object_type_decl = None;
            // Self-closing <bpmn:dataObject/> with no type declaration is silently ignored.
        }
        "userTask" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            if is_empty {
                let idx = graph.add_node(IRNode::HumanWait {
                    id: id.clone(),
                    name: name.clone(),
                    task_kind: name,
                    corr_key_source: "0".to_string(),
                });
                node_map.insert(id, idx);
            } else {
                *current_element = Some(ElementContext::UserTask { id, name });
                *extension_task_type = None;
                *extension_corr_key = None;
            }
        }
        "exclusiveGateway" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            let idx = graph.add_node(IRNode::GatewayXor {
                id: id.clone(),
                name,
            });
            node_map.insert(id, idx);
        }
        "parallelGateway" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            let dir_str = get_attr_opt(e, "gatewayDirection").unwrap_or_default();
            let direction = match dir_str.as_str() {
                "Converging" => GatewayDirection::Converging,
                _ => GatewayDirection::Diverging,
            };
            let idx = graph.add_node(IRNode::GatewayAnd {
                id: id.clone(),
                name,
                direction,
            });
            node_map.insert(id, idx);
        }
        "inclusiveGateway" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            let dir_str = get_attr_opt(e, "gatewayDirection").unwrap_or_default();
            let direction = match dir_str.as_str() {
                "Converging" => GatewayDirection::Converging,
                _ => GatewayDirection::Diverging,
            };
            let idx = graph.add_node(IRNode::GatewayInclusive {
                id: id.clone(),
                name,
                direction,
            });
            node_map.insert(id, idx);
        }
        "intermediateCatchEvent" if *in_process => {
            let id = get_attr(e, "id")?;
            let name = get_attr_opt(e, "name").unwrap_or_default();
            if !is_empty {
                *current_element = Some(ElementContext::IntermediateCatch { id, name });
                *sub_event_type = None;
                *extension_corr_key = None;
                *timer_text = None;
                *timer_kind = None;
            }
        }
        "timerEventDefinition" => {
            *sub_event_type = Some(SubEventType::Timer);
        }
        "messageEventDefinition" => {
            *sub_event_type = Some(SubEventType::Message);
        }
        "terminateEventDefinition" => {
            *sub_event_type = Some(SubEventType::Terminate);
        }
        "errorEventDefinition" => {
            let error_ref = get_attr_opt(e, "errorRef");
            *sub_event_type = Some(SubEventType::Error { error_ref });
        }
        "timeDuration" => {
            *timer_kind = Some(TimerKind::Duration);
            *in_timer_child = true;
        }
        "timeDate" => {
            *timer_kind = Some(TimerKind::Date);
            *in_timer_child = true;
        }
        "timeCycle" => {
            *timer_kind = Some(TimerKind::Cycle);
            *in_timer_child = true;
        }
        "sequenceFlow" if *in_process => {
            let id = get_attr(e, "id")?;
            let source = get_attr(e, "sourceRef")?;
            let target = get_attr(e, "targetRef")?;
            if is_empty {
                // Self-closing sequence flow (no condition)
                flows.push(SequenceFlowRaw {
                    id,
                    source,
                    target,
                    condition: None,
                });
            } else {
                *current_element = Some(ElementContext::SequenceFlow { id, source, target });
                *condition_text = None;
            }
        }
        "conditionExpression" => {
            *in_condition_expression = true;
        }
        "extensionElements" => {
            *in_extension_elements = true;
        }
        "taskDefinition" if *in_extension_elements => {
            if let Ok(impl_hex) = get_attr(e, "implementation") {
                // FFI service task: <bpmn:taskDefinition implementation="<64hex>">
                *ffi_template_id = Some(decode_template_id(&impl_hex)?);
                *in_ffi_task_definition = true;
            } else if let Ok(tt) = get_attr(e, "type") {
                // External-job task: <zeebe:taskDefinition type="..."/>
                *extension_task_type = Some(tt);
            }
        }
        "input" if *in_ffi_task_definition && *in_extension_elements => {
            // <bpmn:input target="<field>" expression="<expr>"/>
            let target_field =
                get_attr(e, "target").context("<bpmn:input> missing target= attribute")?;
            let expr_str =
                get_attr(e, "expression").context("<bpmn:input> missing expression= attribute")?;
            let expression = parse_expression(&expr_str)?;
            ffi_inputs.push(FfiInputBinding {
                target_field,
                expression,
            });
        }
        "output" if *in_ffi_task_definition && *in_extension_elements => {
            // <bpmn:output source="<field>" target="<variable>"/>
            let source_field =
                get_attr(e, "source").context("<bpmn:output> missing source= attribute")?;
            let target_variable =
                get_attr(e, "target").context("<bpmn:output> missing target= attribute")?;
            ffi_outputs.push(FfiOutputBinding {
                source_field,
                target_variable,
            });
        }
        "dataType" if *in_extension_elements => {
            // <bpmn:dataType primitive="..." role="..."/> or
            // <bpmn:dataType domain="<uuid>" version-hash="<32hex>" role="..."/>
            *data_object_type_decl = Some(parse_data_object_type(e)?);
        }
        "subscription" if *in_extension_elements => {
            if let Ok(ck) = get_attr(e, "correlationKey") {
                let stripped = ck.strip_prefix('=').unwrap_or(&ck).trim().to_string();
                *extension_corr_key = Some(stripped);
            }
        }
        "boundaryEvent" if *in_process => {
            let id = get_attr(e, "id")?;
            let attached_to = get_attr(e, "attachedToRef")?;
            let cancel_str =
                get_attr_opt(e, "cancelActivity").unwrap_or_else(|| "true".to_string());
            let cancel_activity = cancel_str != "false";
            if !is_empty {
                *current_element = Some(ElementContext::BoundaryEvent {
                    id,
                    attached_to,
                    cancel_activity,
                });
                *sub_event_type = None;
                *timer_text = None;
                *timer_kind = None;
            }
        }
        // Unsupported elements inside process
        "scriptTask" | "businessRuleTask" | "sendTask" | "receiveTask" | "manualTask"
        | "subProcess" | "callActivity" | "eventBasedGateway" | "complexGateway"
            if *in_process =>
        {
            let id = get_attr_opt(e, "id").unwrap_or_else(|| local.clone());
            return Err(anyhow!("Unsupported BPMN element: <{}> (id={})", local, id));
        }
        _ => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_close_tag(
    local: &str,
    graph: &mut IRGraph,
    node_map: &mut HashMap<String, NodeIndex>,
    flows: &mut Vec<SequenceFlowRaw>,
    in_process: &mut bool,
    current_element: &mut Option<ElementContext>,
    extension_task_type: &mut Option<String>,
    extension_corr_key: &mut Option<String>,
    condition_text: &mut Option<String>,
    in_extension_elements: &mut bool,
    in_condition_expression: &mut bool,
    sub_event_type: &mut Option<SubEventType>,
    timer_text: &mut Option<String>,
    timer_kind: &mut Option<TimerKind>,
    in_timer_child: &mut bool,
    error_defs: &HashMap<String, String>,
    ffi_template_id: &mut Option<[u8; 32]>,
    ffi_inputs: &mut Vec<FfiInputBinding>,
    ffi_outputs: &mut Vec<FfiOutputBinding>,
    in_ffi_task_definition: &mut bool,
    data_object_type_decl: &mut Option<(DataObjectType, DataObjectRole)>,
) -> Result<()> {
    match local {
        "process" => {
            *in_process = false;
        }
        "extensionElements" => {
            *in_extension_elements = false;
        }
        "conditionExpression" => {
            *in_condition_expression = false;
        }
        "timeDuration" | "timeDate" | "timeCycle" => {
            *in_timer_child = false;
        }
        "taskDefinition" if *in_ffi_task_definition => {
            *in_ffi_task_definition = false;
        }
        "dataObject" => {
            if let Some(ElementContext::DataObject { id, name }) = current_element.take() {
                let (type_decl, role) = data_object_type_decl.take().unwrap_or((
                    DataObjectType::Primitive(PrimitiveType::String),
                    DataObjectRole::Internal,
                ));
                let idx = graph.add_node(IRNode::DataObject {
                    id: id.clone(),
                    name,
                    type_decl,
                    role,
                });
                node_map.insert(id, idx);
            }
        }
        "serviceTask" => {
            if let Some(ElementContext::ServiceTask { id, name }) = current_element.take() {
                if let Some(tid) = ffi_template_id.take() {
                    // FFI service task — build FfiServiceTask node.
                    let inputs = std::mem::take(ffi_inputs);
                    let outputs = std::mem::take(ffi_outputs);
                    let idx = graph.add_node(IRNode::FfiServiceTask {
                        id: id.clone(),
                        name,
                        template_id: tid,
                        inputs,
                        outputs,
                    });
                    node_map.insert(id, idx);
                } else {
                    // External-job service task — existing path.
                    let task_type = extension_task_type
                        .take()
                        .unwrap_or_else(|| name_to_snake(&name));
                    let idx = graph.add_node(IRNode::ServiceTask {
                        id: id.clone(),
                        name,
                        task_type,
                    });
                    node_map.insert(id, idx);
                }
            }
        }
        "userTask" => {
            if let Some(ElementContext::UserTask { id, name }) = current_element.take() {
                let task_kind = extension_task_type.take().unwrap_or_else(|| name.clone());
                let corr_key_source = extension_corr_key.take().unwrap_or_else(|| "0".to_string());
                let idx = graph.add_node(IRNode::HumanWait {
                    id: id.clone(),
                    name,
                    task_kind,
                    corr_key_source,
                });
                node_map.insert(id, idx);
            }
        }
        "intermediateCatchEvent" => {
            if let Some(ElementContext::IntermediateCatch { id, name }) = current_element.take() {
                match sub_event_type.take() {
                    Some(SubEventType::Timer) => {
                        let spec = parse_timer_spec(timer_kind.take(), timer_text.take())?;
                        let idx = graph.add_node(IRNode::TimerWait {
                            id: id.clone(),
                            spec,
                        });
                        node_map.insert(id, idx);
                    }
                    Some(SubEventType::Message) => {
                        let corr_key_source =
                            extension_corr_key.take().unwrap_or_else(|| "0".to_string());
                        let idx = graph.add_node(IRNode::MessageWait {
                            id: id.clone(),
                            name,
                            corr_key_source,
                        });
                        node_map.insert(id, idx);
                    }
                    Some(SubEventType::Terminate) => {
                        return Err(anyhow!(
                            "intermediateCatchEvent '{}': terminateEventDefinition is only valid on endEvent",
                            id
                        ));
                    }
                    Some(SubEventType::Error { .. }) => {
                        return Err(anyhow!(
                            "intermediateCatchEvent '{}': errorEventDefinition is only valid on boundaryEvent",
                            id
                        ));
                    }
                    None => {
                        return Err(anyhow!(
                            "intermediateCatchEvent '{}' has no timer or message definition",
                            id
                        ));
                    }
                }
            }
        }
        "endEvent" => {
            if let Some(ElementContext::EndEvent { id }) = current_element.take() {
                let terminate = matches!(sub_event_type.take(), Some(SubEventType::Terminate));
                let idx = graph.add_node(IRNode::End {
                    id: id.clone(),
                    terminate,
                });
                node_map.insert(id, idx);
            }
        }
        "sequenceFlow" => {
            if let Some(ElementContext::SequenceFlow { id, source, target }) =
                current_element.take()
            {
                let condition = condition_text
                    .take()
                    .map(|text| parse_condition(&text))
                    .transpose()
                    .with_context(|| {
                        format!("Invalid conditionExpression on sequenceFlow '{}'", id)
                    })?;
                flows.push(SequenceFlowRaw {
                    id,
                    source,
                    target,
                    condition,
                });
            }
        }
        "boundaryEvent" => {
            if let Some(ElementContext::BoundaryEvent {
                id,
                attached_to,
                cancel_activity,
            }) = current_element.take()
            {
                match sub_event_type.take() {
                    Some(SubEventType::Timer) => {
                        let spec = parse_timer_spec(timer_kind.take(), timer_text.take())?;
                        let idx = graph.add_node(IRNode::BoundaryTimer {
                            id: id.clone(),
                            attached_to,
                            spec,
                            interrupting: cancel_activity,
                        });
                        node_map.insert(id, idx);
                    }
                    Some(SubEventType::Error { error_ref }) => {
                        // Resolve errorRef → errorCode via error_defs lookup
                        let error_code = error_ref
                            .as_deref()
                            .and_then(|r| error_defs.get(r))
                            .cloned();
                        let idx = graph.add_node(IRNode::BoundaryError {
                            id: id.clone(),
                            attached_to,
                            error_code,
                        });
                        node_map.insert(id, idx);
                    }
                    Some(SubEventType::Message) => {
                        return Err(anyhow!(
                            "boundaryEvent '{}': message boundary events not yet supported (Phase 3+)",
                            id
                        ));
                    }
                    Some(SubEventType::Terminate) => {
                        return Err(anyhow!(
                            "boundaryEvent '{}': terminateEventDefinition is only valid on endEvent",
                            id
                        ));
                    }
                    None => {
                        return Err(anyhow!(
                            "boundaryEvent '{}' has no event definition (timer/message/error)",
                            id
                        ));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// ─── Internal types ───────────────────────────────────────────

#[derive(Debug)]
enum ElementContext {
    ServiceTask {
        id: String,
        name: String,
    },
    UserTask {
        id: String,
        name: String,
    },
    IntermediateCatch {
        id: String,
        name: String,
    },
    SequenceFlow {
        id: String,
        source: String,
        target: String,
    },
    BoundaryEvent {
        id: String,
        attached_to: String,
        cancel_activity: bool,
    },
    EndEvent {
        id: String,
    },
    DataObject {
        id: String,
        name: String,
    },
}

#[derive(Debug)]
enum SubEventType {
    Timer,
    Message,
    Terminate,
    Error { error_ref: Option<String> },
}

#[derive(Debug)]
enum TimerKind {
    Duration,
    Date,
    Cycle,
}

struct SequenceFlowRaw {
    id: String,
    source: String,
    target: String,
    condition: Option<ConditionExpr>,
}

// ─── Helpers ──────────────────────────────────────────────────

/// Strip namespace prefix from XML element/attribute name.
/// e.g., "bpmn:startEvent" → "startEvent", "zeebe:taskDefinition" → "taskDefinition"
fn local_name(name: &[u8]) -> String {
    let s = std::str::from_utf8(name).unwrap_or("");
    if let Some(pos) = s.rfind(':') {
        s[pos + 1..].to_string()
    } else {
        s.to_string()
    }
}

/// Get a required attribute value.
fn get_attr(e: &BytesStart, name: &str) -> Result<String> {
    for attr in e.attributes().flatten() {
        let key = local_name(attr.key.as_ref());
        if key == name {
            return Ok(attr.unescape_value()?.to_string());
        }
    }
    Err(anyhow!("Missing required attribute '{}'", name))
}

/// Get an optional attribute value.
fn get_attr_opt(e: &BytesStart, name: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = local_name(attr.key.as_ref());
        if key == name {
            return attr.unescape_value().ok().map(|v| v.to_string());
        }
    }
    None
}

/// Convert a name like "Create Case Record" to snake_case: "create_case_record".
fn name_to_snake(name: &str) -> String {
    name.split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Parse condition expression text.
/// Format: `= flag_name == true`
/// Strips leading `=`, parses `flag_name OP literal`.
fn parse_condition(text: &str) -> Result<ConditionExpr> {
    let text = text.trim();
    let text = text.strip_prefix('=').unwrap_or(text).trim();

    if text.contains('<') || text.contains('>') {
        return Err(anyhow!(
            "numeric comparisons are not supported in BPMN-Lite conditions; use an external decision worker and route on boolean flags"
        ));
    }

    // Switch conditions only support boolean equality/inequality.
    let (flag, op, literal) = if let Some((lhs, rhs)) = text.split_once("==") {
        (lhs.trim(), ConditionOp::Eq, rhs.trim())
    } else if let Some((lhs, rhs)) = text.split_once("!=") {
        (lhs.trim(), ConditionOp::Neq, rhs.trim())
    } else {
        return Err(anyhow!("missing supported operator (==, !=)"));
    };
    if flag.is_empty() {
        return Err(anyhow!("condition flag name is empty"));
    }

    let literal = match literal {
        "true" => ConditionLiteral::Bool(true),
        "false" => ConditionLiteral::Bool(false),
        other => return Err(anyhow!("unsupported condition literal '{}'", other)),
    };

    Ok(ConditionExpr {
        flag_name: flag.to_string(),
        op,
        literal,
    })
}

/// Parse timer spec from duration/date/cycle text.
fn parse_timer_spec(kind: Option<TimerKind>, text: Option<String>) -> Result<TimerSpec> {
    let text = text.ok_or_else(|| anyhow!("Timer element has no duration/date/cycle text"))?;
    let text = text.trim();

    match kind {
        Some(TimerKind::Duration) => {
            let ms = parse_iso_duration(text)?;
            Ok(TimerSpec::Duration { ms })
        }
        Some(TimerKind::Date) => {
            let deadline_ms = text
                .parse::<u64>()
                .map_err(|_| anyhow!("Cannot parse timer date: {}", text))?;
            Ok(TimerSpec::Date { deadline_ms })
        }
        Some(TimerKind::Cycle) => {
            let (interval_ms, max_fires) = parse_iso_cycle(text)?;
            Ok(TimerSpec::Cycle {
                interval_ms,
                max_fires,
            })
        }
        None => Err(anyhow!("Timer spec has no kind (duration, date, or cycle)")),
    }
}

/// Parse a simple ISO 8601 duration to milliseconds.
/// Supports: PT{n}S, PT{n}M, PT{n}H, P{n}D and combinations like PT1H30M.
fn parse_iso_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    if !s.starts_with('P') {
        let ms = s
            .parse::<u64>()
            .map_err(|_| anyhow!("Cannot parse duration: {}", s))?;
        return bounded_duration(ms, s);
    }

    let mut total_ms: u64 = 0;
    let s = &s[1..]; // strip leading P

    let (date_part, time_part) = if let Some(pos) = s.find('T') {
        (&s[..pos], &s[pos + 1..])
    } else {
        (s, "")
    };

    if !date_part.is_empty() {
        let mut num_buf = String::new();
        for ch in date_part.chars() {
            if ch.is_ascii_digit() {
                num_buf.push(ch);
            } else if ch == 'D' {
                let n: u64 = num_buf
                    .parse()
                    .map_err(|_| anyhow!("Bad duration number: {}", num_buf))?;
                add_duration_ms(&mut total_ms, n, 86_400_000, s)?;
                num_buf.clear();
            }
        }
    }

    if !time_part.is_empty() {
        let mut num_buf = String::new();
        for ch in time_part.chars() {
            if ch.is_ascii_digit() {
                num_buf.push(ch);
            } else {
                let n: u64 = num_buf
                    .parse()
                    .map_err(|_| anyhow!("Bad duration number: {}", num_buf))?;
                match ch {
                    'H' => add_duration_ms(&mut total_ms, n, 3_600_000, s)?,
                    'M' => add_duration_ms(&mut total_ms, n, 60_000, s)?,
                    'S' => add_duration_ms(&mut total_ms, n, 1_000, s)?,
                    _ => return Err(anyhow!("Unknown duration unit: {}", ch)),
                }
                num_buf.clear();
            }
        }
    }

    if total_ms == 0 {
        Err(anyhow!("Duration parsed to 0ms: {}", s))
    } else {
        bounded_duration(total_ms, s)
    }
}

fn add_duration_ms(total_ms: &mut u64, n: u64, unit_ms: u64, input: &str) -> Result<()> {
    let component = n
        .checked_mul(unit_ms)
        .ok_or_else(|| anyhow!("Duration overflows milliseconds: {}", input))?;
    *total_ms = total_ms
        .checked_add(component)
        .ok_or_else(|| anyhow!("Duration overflows milliseconds: {}", input))?;
    Ok(())
}

fn bounded_duration(ms: u64, input: &str) -> Result<u64> {
    if ms == 0 {
        return Err(anyhow!("Duration parsed to 0ms: {}", input));
    }
    if ms > MAX_TIMER_MS {
        return Err(anyhow!(
            "Duration exceeds max timer bound: {}ms > {}ms",
            ms,
            MAX_TIMER_MS
        ));
    }
    Ok(ms)
}

/// Parse an ISO 8601 repeating interval to (interval_ms, max_fires).
/// Format: `R<n>/PT<duration>` where n is the repetition count.
/// Examples: `R3/PT1H` → (3_600_000, 3), `R5/PT30M` → (1_800_000, 5)
fn parse_iso_cycle(s: &str) -> Result<(u64, u32)> {
    let s = s.trim();
    if !s.starts_with('R') {
        return Err(anyhow!("Cycle must start with 'R<count>/' — got: {}", s));
    }
    let rest = &s[1..]; // strip 'R'
    let slash_pos = rest
        .find('/')
        .ok_or_else(|| anyhow!("Cycle missing '/' separator: {}", s))?;

    let count_str = &rest[..slash_pos];
    let duration_str = &rest[slash_pos + 1..];

    let max_fires: u32 = count_str
        .parse()
        .map_err(|_| anyhow!("Cannot parse cycle count '{}' in: {}", count_str, s))?;

    if max_fires == 0 {
        return Err(anyhow!("Cycle count must be >= 1: {}", s));
    }
    if max_fires > MAX_TIMER_CYCLE_FIRES {
        return Err(anyhow!(
            "Cycle count exceeds max timer bound: {} > {}",
            max_fires,
            MAX_TIMER_CYCLE_FIRES
        ));
    }

    let interval_ms = parse_iso_duration(duration_str)?;
    Ok((interval_ms, max_fires))
}

// ── FFI / data-object parser helpers ──────────────────────────────────────────

/// Decode a 64-character lowercase hex string to a 32-byte BLAKE3 template id.
fn decode_template_id(hex: &str) -> Result<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        anyhow::bail!(
            "FFI template_id must be exactly 64 hex characters (got {}): '{}'",
            hex.len(),
            &hex[..hex.len().min(16)]
        );
    }
    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk)
            .map_err(|_| anyhow!("non-UTF-8 in template_id hex at byte {}", i * 2))?;
        bytes[i] = u8::from_str_radix(s, 16)
            .map_err(|_| anyhow!("invalid hex in template_id at byte {}: '{}'", i * 2, s))?;
    }
    Ok(bytes)
}

/// Parse a C-minimal expression string (A2 §5 / V&S v1.1 §10.6).
///
/// Grammar:
/// ```text
/// expression   ::= literal | variable-ref
/// variable-ref ::= "${" identifier ("." identifier)* "}"
/// literal      ::= boolean | integer | float | string | symbol
/// ```
///
/// Rules:
/// - `${identifier}` or `${a.b.c}` → VarRef
/// - `true` / `false` (case-insensitive) → Bool literal
/// - Numeric strings that parse as i64 → I64 literal
/// - Numeric strings that parse as f64 → F64 literal
/// - Everything else → String literal (bare symbol or quoted string)
fn parse_expression(s: &str) -> Result<Expression> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix("${").and_then(|t| t.strip_suffix('}')) {
        let path: Vec<String> = inner.split('.').map(|seg| seg.trim().to_string()).collect();
        if path.is_empty() || path.iter().any(|p| p.is_empty()) {
            anyhow::bail!("invalid variable reference '{}': empty path segment", s);
        }
        return Ok(Expression::VarRef(path));
    }
    if s.eq_ignore_ascii_case("true") {
        return Ok(Expression::Literal(IrLiteral::Bool(true)));
    }
    if s.eq_ignore_ascii_case("false") {
        return Ok(Expression::Literal(IrLiteral::Bool(false)));
    }
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Expression::Literal(IrLiteral::I64(n)));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(Expression::Literal(IrLiteral::F64(f)));
    }
    // Quoted string: strip outer quotes
    let stripped = s
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')));
    Ok(Expression::Literal(IrLiteral::String(
        stripped.unwrap_or(s).to_string(),
    )))
}

/// Parse a `<bpmn:dataType .../>` element into a type + role declaration.
fn parse_data_object_type(e: &BytesStart) -> Result<(DataObjectType, DataObjectRole)> {
    let role = match get_attr_opt(e, "role").as_deref() {
        Some("input") => DataObjectRole::Input,
        Some("output") => DataObjectRole::Output,
        _ => DataObjectRole::Internal,
    };
    if let Some(prim_str) = get_attr_opt(e, "primitive") {
        let prim = match prim_str.to_lowercase().as_str() {
            "bool" | "boolean" => PrimitiveType::Bool,
            "integer" | "int" | "i64" => PrimitiveType::I64,
            "float" | "f64" | "decimal" | "number" => PrimitiveType::F64,
            "string" | "str" | "text" => PrimitiveType::String,
            other => anyhow::bail!(
                "<bpmn:dataType> unknown primitive='{}': expected bool/integer/float/string",
                other
            ),
        };
        return Ok((DataObjectType::Primitive(prim), role));
    }
    if let Some(domain_str) = get_attr_opt(e, "domain") {
        let domain_id: Uuid = domain_str.parse().map_err(|_| {
            anyhow!(
                "<bpmn:dataType> invalid UUID in domain='{}': must be a UUIDv7 hex string",
                domain_str
            )
        })?;
        let version_hash_str = get_attr_opt(e, "version-hash").unwrap_or_default();
        let version_hash = if version_hash_str.is_empty() {
            [0u8; 32]
        } else {
            decode_template_id(&version_hash_str).with_context(|| {
                format!(
                    "<bpmn:dataType> invalid version-hash: '{}'",
                    version_hash_str
                )
            })?
        };
        return Ok((
            DataObjectType::SemOsDomain {
                domain_id,
                version_hash,
            },
            role,
        ));
    }
    anyhow::bail!("<bpmn:dataType> requires either primitive= or domain= attribute")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lowering, verifier};

    /// A5.T1: Minimal process (Start → ServiceTask → End) parses correctly
    #[test]
    fn test_minimal_process_parses() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start1" />
            <bpmn:serviceTask id="task1" name="Create Case Record" />
            <bpmn:endEvent id="end1" />
            <bpmn:sequenceFlow id="f1" sourceRef="start1" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end1" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);

        // Verify task_type falls back to name
        let has_service_task = graph.node_weights().any(|n| {
            matches!(n, IRNode::ServiceTask { task_type, .. } if task_type == "create_case_record")
        });
        assert!(has_service_task);
    }

    /// A5.T2: ServiceTask task_type extracted from zeebe extension
    #[test]
    fn test_task_type_from_zeebe_extension() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start1" />
            <bpmn:serviceTask id="task1" name="Create Case">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="create_case_record" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:endEvent id="end1" />
            <bpmn:sequenceFlow id="f1" sourceRef="start1" targetRef="task1" />
            <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end1" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();
        let has_correct_type = graph.node_weights().any(|n| {
            matches!(n, IRNode::ServiceTask { task_type, .. } if task_type == "create_case_record")
        });
        assert!(has_correct_type);
    }

    /// A5.T3: ServiceTask task_type falls back to name attribute
    #[test]
    fn test_task_type_name_fallback() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <definitions xmlns="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <process id="proc1" isExecutable="true">
            <startEvent id="start1" />
            <serviceTask id="task1" name="Request Documents" />
            <endEvent id="end1" />
            <sequenceFlow id="f1" sourceRef="start1" targetRef="task1" />
            <sequenceFlow id="f2" sourceRef="task1" targetRef="end1" />
          </process>
        </definitions>"#;

        let graph = parse_bpmn(xml).unwrap();
        let has_correct_type = graph.node_weights().any(|n| {
            matches!(n, IRNode::ServiceTask { task_type, .. } if task_type == "request_documents")
        });
        assert!(has_correct_type);
    }

    /// A5.T4: MessageWait correlation key extracted
    #[test]
    fn test_message_wait_correlation_key() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start1" />
            <bpmn:intermediateCatchEvent id="msg1" name="docs_received">
              <bpmn:messageEventDefinition messageRef="msg_def_1" />
              <bpmn:extensionElements>
                <zeebe:subscription correlationKey="=case_id" />
              </bpmn:extensionElements>
            </bpmn:intermediateCatchEvent>
            <bpmn:endEvent id="end1" />
            <bpmn:sequenceFlow id="f1" sourceRef="start1" targetRef="msg1" />
            <bpmn:sequenceFlow id="f2" sourceRef="msg1" targetRef="end1" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();
        let has_correct_corr = graph.node_weights().any(|n| {
            matches!(n, IRNode::MessageWait { corr_key_source, .. } if corr_key_source == "case_id")
        });
        assert!(has_correct_corr);
    }

    /// A5.T5: XOR gateway condition expressions parsed
    #[test]
    fn test_xor_condition_expressions() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start1" />
            <bpmn:exclusiveGateway id="gw1" name="Decision" />
            <bpmn:serviceTask id="task_a" name="Approve" />
            <bpmn:serviceTask id="task_b" name="Reject" />
            <bpmn:endEvent id="end1" />
            <bpmn:sequenceFlow id="f1" sourceRef="start1" targetRef="gw1" />
            <bpmn:sequenceFlow id="f2" sourceRef="gw1" targetRef="task_a">
              <bpmn:conditionExpression>= approved == true</bpmn:conditionExpression>
            </bpmn:sequenceFlow>
            <bpmn:sequenceFlow id="f3" sourceRef="gw1" targetRef="task_b" />
            <bpmn:sequenceFlow id="f4" sourceRef="task_a" targetRef="end1" />
            <bpmn:sequenceFlow id="f5" sourceRef="task_b" targetRef="end1" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();

        // Find the conditional edge
        let conditional_edges: Vec<_> = graph
            .edge_weights()
            .filter(|e| e.condition.is_some())
            .collect();
        assert_eq!(conditional_edges.len(), 1);

        let cond = conditional_edges[0].condition.as_ref().unwrap();
        assert_eq!(cond.flag_name, "approved");
        assert_eq!(cond.op, ConditionOp::Eq);
        assert_eq!(cond.literal, ConditionLiteral::Bool(true));
    }

    /// A5.T6: Unsupported element produces compile error
    #[test]
    fn test_unsupported_element_error() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start1" />
            <bpmn:scriptTask id="script1" name="Bad" />
            <bpmn:endEvent id="end1" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let result = parse_bpmn(xml);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unsupported BPMN element"));
        assert!(err.contains("scriptTask"));
    }

    /// A5.T7: Full kyc-open-case BPMN → IR → verify → lower → bytecode valid
    #[test]
    fn test_full_pipeline_kyc_open_case() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="kyc_open_case" isExecutable="true">
            <bpmn:startEvent id="start" />

            <bpmn:serviceTask id="create_case" name="Create Case Record">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="create_case_record" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>

            <bpmn:serviceTask id="request_docs" name="Request Documents">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="request_documents" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>

            <bpmn:intermediateCatchEvent id="wait_docs" name="docs_received">
              <bpmn:messageEventDefinition messageRef="msg_docs" />
              <bpmn:extensionElements>
                <zeebe:subscription correlationKey="=case_id" />
              </bpmn:extensionElements>
            </bpmn:intermediateCatchEvent>

            <bpmn:userTask id="reviewer_decision" name="Reviewer Decision">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="reviewer_decision" />
                <zeebe:subscription correlationKey="=case_id" />
              </bpmn:extensionElements>
            </bpmn:userTask>

            <bpmn:serviceTask id="record_decision" name="Record Decision">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="record_decision" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>

            <bpmn:endEvent id="end" />

            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="create_case" />
            <bpmn:sequenceFlow id="f2" sourceRef="create_case" targetRef="request_docs" />
            <bpmn:sequenceFlow id="f3" sourceRef="request_docs" targetRef="wait_docs" />
            <bpmn:sequenceFlow id="f4" sourceRef="wait_docs" targetRef="reviewer_decision" />
            <bpmn:sequenceFlow id="f5" sourceRef="reviewer_decision" targetRef="record_decision" />
            <bpmn:sequenceFlow id="f6" sourceRef="record_decision" targetRef="end" />
          </bpmn:process>
        </bpmn:definitions>"#;

        // Parse
        let graph = parse_bpmn(xml).unwrap();
        assert_eq!(graph.node_count(), 7); // start, 3 service tasks, msg wait, human wait, end

        // Verify
        verifier::verify_or_err(&graph).unwrap();

        // Lower
        let program = lowering::lower(&graph).unwrap();

        // Bytecode should have ExecNative instructions for 3 service tasks
        let exec_native_count = program
            .program
            .iter()
            .filter(|i| matches!(i, bpmn_lite_types::Instr::ExecNative { .. }))
            .count();
        assert_eq!(exec_native_count, 3);

        // Should have WaitMsg for the message wait and human wait
        let wait_msg_count = program
            .program
            .iter()
            .filter(|i| matches!(i, bpmn_lite_types::Instr::WaitMsg { .. }))
            .count();
        assert_eq!(wait_msg_count, 2);

        // Task manifest should include the task types
        assert!(program
            .task_manifest
            .contains(&"create_case_record".to_string()));
        assert!(program
            .task_manifest
            .contains(&"request_documents".to_string()));
        assert!(program
            .task_manifest
            .contains(&"record_decision".to_string()));

        // Bytecode version should be non-zero
        assert_ne!(program.bytecode_version, [0u8; 32]);
    }

    #[test]
    fn test_parse_iso_duration() {
        assert_eq!(parse_iso_duration("PT5S").unwrap(), 5_000);
        assert_eq!(parse_iso_duration("PT30M").unwrap(), 1_800_000);
        assert_eq!(parse_iso_duration("PT1H").unwrap(), 3_600_000);
        assert_eq!(parse_iso_duration("P1D").unwrap(), 86_400_000);
        assert_eq!(parse_iso_duration("PT1H30M").unwrap(), 5_400_000);
    }

    #[test]
    fn test_parse_iso_cycle() {
        let (ms, n) = parse_iso_cycle("R3/PT1H").unwrap();
        assert_eq!(ms, 3_600_000);
        assert_eq!(n, 3);

        let (ms, n) = parse_iso_cycle("R5/PT30M").unwrap();
        assert_eq!(ms, 1_800_000);
        assert_eq!(n, 5);

        let (ms, n) = parse_iso_cycle("R1/P1D").unwrap();
        assert_eq!(ms, 86_400_000);
        assert_eq!(n, 1);

        // Error cases
        assert!(parse_iso_cycle("PT1H").is_err(), "Missing R prefix");
        assert!(parse_iso_cycle("R0/PT1H").is_err(), "Zero count");
        assert!(parse_iso_cycle("R3PT1H").is_err(), "Missing slash");
    }

    #[test]
    fn test_parse_condition() {
        let c = parse_condition("= approved == true").unwrap();
        assert_eq!(c.flag_name, "approved");
        assert_eq!(c.op, ConditionOp::Eq);
        assert_eq!(c.literal, ConditionLiteral::Bool(true));

        let c = parse_condition("approved != false").unwrap();
        assert_eq!(c.flag_name, "approved");
        assert_eq!(c.op, ConditionOp::Neq);
        assert_eq!(c.literal, ConditionLiteral::Bool(false));

        assert!(parse_condition("count > 5").is_err());
        assert!(parse_condition("count < 5").is_err());
        assert!(parse_condition("count == 5").is_err());
        assert!(parse_condition("garbage").is_err());
    }

    // ═══════════════════════════════════════════════════════════
    //  Phase 2: Boundary timer parser tests
    // ═══════════════════════════════════════════════════════════

    /// T-BTIMER-1: boundaryEvent with timer parses to BoundaryTimer IR node
    #[test]
    fn t_btimer_1_parse_boundary_timer() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="verify_docs" name="Verify Documents">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="verify_docs" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:boundaryEvent id="timeout" attachedToRef="verify_docs" cancelActivity="true">
              <bpmn:timerEventDefinition>
                <bpmn:timeDuration>P3D</bpmn:timeDuration>
              </bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:serviceTask id="escalate" name="Escalate">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="escalate_case" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:endEvent id="end_normal" />
            <bpmn:endEvent id="end_escalated" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="verify_docs" />
            <bpmn:sequenceFlow id="f2" sourceRef="verify_docs" targetRef="end_normal" />
            <bpmn:sequenceFlow id="f3" sourceRef="timeout" targetRef="escalate" />
            <bpmn:sequenceFlow id="f4" sourceRef="escalate" targetRef="end_escalated" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();

        let has_boundary = graph.node_weights().any(|n| {
            matches!(n, IRNode::BoundaryTimer {
                attached_to, interrupting: true, ..
            } if attached_to == "verify_docs")
        });
        assert!(
            has_boundary,
            "Should parse boundaryEvent as BoundaryTimer IR node"
        );
        assert_eq!(
            graph.node_count(),
            6,
            "Expected 6 nodes, got {}",
            graph.node_count()
        );
        assert_eq!(graph.edge_count(), 4, "Expected 4 edges");
    }

    /// T-BTIMER-2: Full pipeline: parse → verify → lower → boundary_map + race_plan populated
    #[test]
    fn t_btimer_2_lower_boundary_timer() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                          xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
          <bpmn:process id="proc1" isExecutable="true">
            <bpmn:startEvent id="start" />
            <bpmn:serviceTask id="verify_docs" name="Verify Documents">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="verify_docs" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:boundaryEvent id="timeout" attachedToRef="verify_docs" cancelActivity="true">
              <bpmn:timerEventDefinition>
                <bpmn:timeDuration>P3D</bpmn:timeDuration>
              </bpmn:timerEventDefinition>
            </bpmn:boundaryEvent>
            <bpmn:serviceTask id="escalate" name="Escalate">
              <bpmn:extensionElements>
                <zeebe:taskDefinition type="escalate_case" />
              </bpmn:extensionElements>
            </bpmn:serviceTask>
            <bpmn:endEvent id="end_normal" />
            <bpmn:endEvent id="end_escalated" />
            <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="verify_docs" />
            <bpmn:sequenceFlow id="f2" sourceRef="verify_docs" targetRef="end_normal" />
            <bpmn:sequenceFlow id="f3" sourceRef="timeout" targetRef="escalate" />
            <bpmn:sequenceFlow id="f4" sourceRef="escalate" targetRef="end_escalated" />
          </bpmn:process>
        </bpmn:definitions>"#;

        let graph = parse_bpmn(xml).unwrap();
        verifier::verify_or_err(&graph).unwrap();
        let program = lowering::lower(&graph).unwrap();

        assert_eq!(
            program.boundary_map.len(),
            1,
            "Expected 1 boundary_map entry"
        );
        assert_eq!(program.race_plan.len(), 1, "Expected 1 race_plan entry");

        let (_, race_entry) = program.race_plan.iter().next().unwrap();
        assert_eq!(
            race_entry.arms.len(),
            2,
            "Expected 2 arms (Internal + Timer)"
        );
        assert!(
            matches!(
                race_entry.arms[0],
                bpmn_lite_types::WaitArm::Internal { .. }
            ),
            "Arm 0 = Internal"
        );

        match &race_entry.arms[1] {
            bpmn_lite_types::WaitArm::Timer {
                duration_ms,
                resume_at,
                interrupting,
                cycle,
            } => {
                assert!(*interrupting, "Default boundary should be interrupting");
                assert!(cycle.is_none(), "No cycle spec for simple duration timer");
                assert_eq!(*duration_ms, 259_200_000, "3 days = 259200000ms");
                assert!(*resume_at > 0, "resume_at should be valid");
                // resume_at should point to escalation task code, not boundary node
                let instr = &program.program[*resume_at as usize];
                assert!(
                    matches!(
                        instr,
                        bpmn_lite_types::Instr::ExecNative { .. }
                            | bpmn_lite_types::Instr::Jump { .. }
                    ),
                    "Timer resume_at should point to escalation code, got {:?}",
                    instr
                );
            }
            other => panic!("Arm 1 should be Timer, got {:?}", other),
        }
    }
}
