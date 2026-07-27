//! The context projection — the ONE serializer (DIR-002 A1).
//!
//! The text this module emits is simultaneously (a) the training-side
//! context text every corpus example embeds and (b) the inference-side
//! preimage of `context_projection_hash` in the I28 `DecisionRecord`.
//! There is deliberately no second textualisation anywhere: train/serve
//! skew is the failure mode A1 names, and the way to not have it is for
//! this function to be the only one that exists. The hash is DERIVED from
//! the serialized bytes here and is never caller-supplied (same rule as
//! `ConfigRegistry`: a supplied hash is an unverifiable claim).
//!
//! Canonical form: a fixed line grammar, one field per line, list items
//! on their own `- `-indented lines, everything ordered by construction
//! (lists sorted at build time, refused if not). Injectivity holds
//! because the line structure is fixed and NO field value may contain a
//! control character — that is enforced as a typed reject at
//! construction (`FiniteScore` pattern: the invalid value is
//! unrepresentable, not checked downstream).

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// Bump on ANY change to the serialized form; the version is the first
/// line of the preimage, so old and new forms can never hash-collide.
pub const CONTEXT_PROJECTION_SCHEMA_VERSION: u32 = 1;

/// One node, as the projection sees it: a stable kind string and its
/// BPMN id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSummary {
    pub kind: String,
    pub id: String,
}

/// The anchor neighbourhood — present when the Designer session has a
/// cursor/selection; absent on whole-graph boards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorContext {
    pub node: NodeSummary,
    /// Sorted by id at construction (refused otherwise).
    pub predecessors: Vec<NodeSummary>,
    pub successors: Vec<NodeSummary>,
    /// Guards attached to the anchor node.
    pub attached_guards: Vec<NodeSummary>,
}

/// The serializable Designer context an utterance is interpreted in.
/// Construct via [`ContextProjection::new`]; fields are private so no
/// path can bypass the control-character and ordering rejects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProjection {
    schema_version: u32,
    pack_identity: String,
    graph_identity: String,
    anchor: Option<AnchorContext>,
    /// (kind, count), sorted by kind, no duplicates.
    node_kind_counts: Vec<(String, u32)>,
}

fn reject_control(field: &str, value: &str) -> Result<()> {
    if value.chars().any(|c| c.is_control()) {
        return Err(anyhow!(
            "context projection refused: field '{field}' contains a control character \
             (value {value:?}) — the canonical line grammar would lose injectivity"
        ));
    }
    Ok(())
}

fn check_summaries(field: &str, items: &[NodeSummary]) -> Result<()> {
    for s in items {
        reject_control(field, &s.kind)?;
        reject_control(field, &s.id)?;
        if s.kind.contains(' ') {
            return Err(anyhow!(
                "context projection refused: node kind {:?} in '{field}' contains a space — \
                 kinds are single tokens by contract",
                s.kind
            ));
        }
    }
    if !items.windows(2).all(|w| w[0].id <= w[1].id) {
        return Err(anyhow!(
            "context projection refused: '{field}' is not sorted by id — canonical order \
             is the constructor's obligation, not the serializer's repair job"
        ));
    }
    Ok(())
}

impl ContextProjection {
    pub fn new(
        pack_identity: impl Into<String>,
        graph_identity: impl Into<String>,
        anchor: Option<AnchorContext>,
        node_kind_counts: Vec<(String, u32)>,
    ) -> Result<Self> {
        let pack_identity = pack_identity.into();
        let graph_identity = graph_identity.into();
        reject_control("pack_identity", &pack_identity)?;
        reject_control("graph_identity", &graph_identity)?;
        if let Some(a) = &anchor {
            reject_control("anchor.node", &a.node.kind)?;
            reject_control("anchor.node", &a.node.id)?;
            check_summaries("anchor.predecessors", &a.predecessors)?;
            check_summaries("anchor.successors", &a.successors)?;
            check_summaries("anchor.attached_guards", &a.attached_guards)?;
        }
        for (kind, _) in &node_kind_counts {
            reject_control("node_kind_counts", kind)?;
            if kind.contains(' ') {
                return Err(anyhow!(
                    "context projection refused: kind {kind:?} contains a space"
                ));
            }
        }
        if !node_kind_counts.windows(2).all(|w| w[0].0 < w[1].0) {
            return Err(anyhow!(
                "context projection refused: node_kind_counts must be strictly sorted by \
                 kind with no duplicates"
            ));
        }
        Ok(ContextProjection {
            schema_version: CONTEXT_PROJECTION_SCHEMA_VERSION,
            pack_identity,
            graph_identity,
            anchor,
            node_kind_counts,
        })
    }

    /// THE canonical text — training context and hash preimage alike.
    pub fn serialize_canonical(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("ctxproj.v{}\n", self.schema_version));
        out.push_str(&format!("pack: {}\n", self.pack_identity));
        out.push_str(&format!("graph: {}\n", self.graph_identity));
        match &self.anchor {
            None => out.push_str("anchor: none\n"),
            Some(a) => {
                out.push_str(&format!("anchor: {} {}\n", a.node.kind, a.node.id));
                for (label, list) in [
                    ("predecessors", &a.predecessors),
                    ("successors", &a.successors),
                    ("attached_guards", &a.attached_guards),
                ] {
                    out.push_str(&format!("{label}:\n"));
                    for s in list {
                        out.push_str(&format!("- {} {}\n", s.kind, s.id));
                    }
                }
            }
        }
        out.push_str("nodes:\n");
        for (kind, count) in &self.node_kind_counts {
            out.push_str(&format!("- {kind} x{count}\n"));
        }
        out
    }

    /// blake3 over the canonical bytes — derived here, supplied nowhere.
    pub fn hash(&self) -> String {
        blake3::hash(self.serialize_canonical().as_bytes())
            .to_hex()
            .to_string()
    }
}

/// A minimal whole-graph projection for tests and single-board harnesses:
/// no anchor, empty counts. Real sessions build the full projection.
pub fn minimal(pack_identity: &str, graph_identity: &str) -> ContextProjection {
    ContextProjection::new(pack_identity, graph_identity, None, vec![])
        .expect("minimal projection has no refusable content")
}

/// THE canonical node-kind vocabulary of the projection (blind-review
/// finding 2 remediation): one mapping, here, used by every constructor.
/// The compiler vocabulary IS the node kind (WS-A.1 schema ruling).
pub fn ir_kind_str(node: &bpmn_lite_compiler::ir::IRNode) -> &'static str {
    use bpmn_lite_compiler::ir::IRNode as N;
    match node {
        N::Start { .. } => "start",
        N::End { .. } => "end",
        N::ServiceTask { .. } => "service_task",
        N::GatewayXor { .. } => "gateway_xor",
        N::GatewayAnd { .. } => "gateway_and",
        N::TimerWait { .. } => "timer_wait",
        N::MessageWait { .. } => "message_wait",
        N::HumanWait { .. } => "human_wait",
        N::BoundaryTimer { .. } => "boundary_timer",
        N::BoundaryError { .. } => "boundary_error",
        N::GatewayInclusive { .. } => "gateway_inclusive",
        N::DataObject { .. } => "data_object",
        N::FfiServiceTask { .. } => "ffi_service_task",
        N::SendTask { .. } => "send_task",
        N::MultiInstance { .. } => "multi_instance",
    }
}

/// THE shared DAG→projection constructor (blind-review finding 2, the A1
/// HALT condition one layer up): both the corpus generator and the
/// serving path construct projections HERE, from the projected IR graph
/// (`DesignerDag::to_ir()` on the designer side). Anchor is a BPMN id;
/// unknown anchor is a typed reject, never a silent whole-graph
/// downgrade. Attached guards are read from the IR's own
/// `attached_to` fields.
///
/// INTERIM LIMITATION (recorded, not hidden): the shadow session
/// endpoint compiles DSL source to an execution plan, which has no IR
/// graph — its projections are census-only via a plan-kind mapping and
/// are NOT training-grade. Convergence point: WS-B's DesignerDag-backed
/// sessions (substrate ask filed in the plan).
pub fn project_ir(
    graph: &petgraph::Graph<bpmn_lite_compiler::ir::IRNode, bpmn_lite_compiler::ir::IREdge>,
    anchor_id: Option<&str>,
    pack_identity: &str,
    graph_identity: &str,
) -> Result<ContextProjection> {
    use petgraph::Direction;

    let mut counts = std::collections::BTreeMap::<String, u32>::new();
    for n in graph.node_weights() {
        *counts.entry(ir_kind_str(n).to_owned()).or_insert(0) += 1;
    }

    let anchor = match anchor_id {
        None => None,
        Some(id) => {
            let idx = graph
                .node_indices()
                .find(|i| graph[*i].id() == id)
                .ok_or_else(|| {
                    anyhow!("projection refused: anchor id {id:?} not present in the graph")
                })?;
            let summarize = |dir: Direction| -> Vec<NodeSummary> {
                let mut v: Vec<NodeSummary> = graph
                    .neighbors_directed(idx, dir)
                    .map(|n| NodeSummary {
                        kind: ir_kind_str(&graph[n]).to_owned(),
                        id: graph[n].id().to_owned(),
                    })
                    .collect();
                v.sort_by(|a, b| a.id.cmp(&b.id));
                v
            };
            let predecessors = summarize(Direction::Incoming);
            let successors = summarize(Direction::Outgoing);
            let mut attached_guards: Vec<NodeSummary> = graph
                .node_weights()
                .filter(|n| {
                    matches!(n,
                        bpmn_lite_compiler::ir::IRNode::BoundaryTimer { attached_to, .. }
                        | bpmn_lite_compiler::ir::IRNode::BoundaryError { attached_to, .. }
                            if attached_to == id)
                })
                .map(|n| NodeSummary { kind: ir_kind_str(n).to_owned(), id: n.id().to_owned() })
                .collect();
            attached_guards.sort_by(|a, b| a.id.cmp(&b.id));
            Some(AnchorContext {
                node: NodeSummary {
                    kind: ir_kind_str(&graph[idx]).to_owned(),
                    id: id.to_owned(),
                },
                predecessors,
                successors,
                attached_guards,
            })
        }
    };

    ContextProjection::new(pack_identity, graph_identity, anchor, counts.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(kind: &str, id: &str) -> NodeSummary {
        NodeSummary { kind: kind.into(), id: id.into() }
    }

    fn fixture() -> ContextProjection {
        ContextProjection::new(
            "pack.demo@0",
            "g-abc123",
            Some(AnchorContext {
                node: ns("service_task", "send_request"),
                predecessors: vec![ns("service_task", "resolve_route")],
                successors: vec![ns("message_wait", "wait_document")],
                attached_guards: vec![ns("boundary_timer", "g_reminder")],
            }),
            vec![
                ("end".into(), 1),
                ("message_wait".into(), 1),
                ("service_task".into(), 3),
                ("start".into(), 1),
            ],
        )
        .unwrap()
    }

    /// Cement: the canonical bytes and their hash are pinned. Any change
    /// to the serialized form MUST bump CONTEXT_PROJECTION_SCHEMA_VERSION
    /// and re-pin (a silent drift here is train/serve skew, A1).
    #[test]
    fn golden_canonical_form_and_hash_are_pinned() {
        let text = fixture().serialize_canonical();
        assert_eq!(
            text,
            "ctxproj.v1\n\
             pack: pack.demo@0\n\
             graph: g-abc123\n\
             anchor: service_task send_request\n\
             predecessors:\n\
             - service_task resolve_route\n\
             successors:\n\
             - message_wait wait_document\n\
             attached_guards:\n\
             - boundary_timer g_reminder\n\
             nodes:\n\
             - end x1\n\
             - message_wait x1\n\
             - service_task x3\n\
             - start x1\n"
        );
        assert_eq!(
            fixture().hash(),
            "07290be2994d396d10dd27a3f47aa1a15eba395b45f8528bdf1ba3c3b871f804",
            "GOLDEN ctxproj.v1 hash moved — that is a schema change; bump the version"
        );
    }

    /// RED: injectivity is enforced at construction, not hoped for.
    #[test]
    fn control_characters_and_disorder_are_refused() {
        assert!(ContextProjection::new("p\nx", "g", None, vec![])
            .unwrap_err()
            .to_string()
            .contains("control character"));
        assert!(ContextProjection::new(
            "p",
            "g",
            None,
            vec![("b".into(), 1), ("a".into(), 1)]
        )
        .unwrap_err()
        .to_string()
        .contains("sorted"));
        assert!(ContextProjection::new(
            "p",
            "g",
            None,
            vec![("a".into(), 1), ("a".into(), 2)]
        )
        .unwrap_err()
        .to_string()
        .contains("sorted"),);
        let unsorted = ContextProjection::new(
            "p",
            "g",
            Some(AnchorContext {
                node: ns("service_task", "t"),
                predecessors: vec![ns("service_task", "b"), ns("service_task", "a")],
                successors: vec![],
                attached_guards: vec![],
            }),
            vec![],
        );
        assert!(unsorted.unwrap_err().to_string().contains("sorted"));
    }

    /// Blind-review finding 2 remediation cement: the shared constructor,
    /// driven cross-crate exactly the way the corpus generator will drive
    /// it — seed(Start) → ops → to_ir → project_ir — produces pinned
    /// canonical bytes. Anchored on the guarded task: neighbourhood and
    /// attached guard come from the IR itself.
    #[test]
    fn project_ir_golden_from_designer_ops() {
        use designer_graph::ops::{apply, GuardTrigger, Operation};
        use designer_graph::schema::{DesignerDag, NodeKey, Provenance};
        use bpmn_lite_compiler::ir::{IRNode, TimerSpec};

        let mut dag = DesignerDag::new("proj-fx");
        let start = dag
            .seed(
                NodeKey(uuid::Uuid::new_v4()),
                IRNode::Start { id: "start".into() },
                Provenance::default(),
            )
            .unwrap();
        // seed() is fail-closed: flow nodes must come through ops.
        assert!(dag
            .seed(
                NodeKey(uuid::Uuid::new_v4()),
                IRNode::ServiceTask { id: "smuggled".into(), name: "s".into(), task_type: "t".into() },
                Provenance::default(),
            )
            .unwrap_err()
            .to_string()
            .contains("seed refused"));

        let t1 = NodeKey(uuid::Uuid::new_v4());
        let guard = NodeKey(uuid::Uuid::new_v4());
        let mut graph = dag;
        for op in [
            Operation::AppendNode {
                anchor: start,
                key: t1,
                node: IRNode::ServiceTask {
                    id: "send_request".into(),
                    name: "send_request".into(),
                    task_type: "noop".into(),
                },
                edge_id: "f1".into(),
            },
            Operation::AppendNode {
                anchor: t1,
                key: NodeKey(uuid::Uuid::new_v4()),
                node: IRNode::End { id: "end".into(), terminate: false },
                edge_id: "f2".into(),
            },
            Operation::AttachRearmingGuard {
                host: t1,
                key: guard,
                guard_id: "g_reminder".into(),
                trigger: GuardTrigger::Timer(TimerSpec::Cycle {
                    interval_ms: 1000,
                    max_fires: 2,
                }),
            },
        ] {
            graph = apply(&graph, op, Provenance::default()).unwrap().candidate;
        }
        let ir = graph.to_ir().unwrap();

        let proj = project_ir(&ir, Some("send_request"), "pack.none", "g-fixture").unwrap();
        assert_eq!(
            proj.serialize_canonical(),
            "ctxproj.v1\n\
             pack: pack.none\n\
             graph: g-fixture\n\
             anchor: service_task send_request\n\
             predecessors:\n\
             - start start\n\
             successors:\n\
             - end end\n\
             attached_guards:\n\
             - boundary_timer g_reminder\n\
             nodes:\n\
             - boundary_timer x1\n\
             - end x1\n\
             - service_task x1\n\
             - start x1\n"
        );
        // Unknown anchor is a typed reject, never a whole-graph downgrade.
        assert!(project_ir(&ir, Some("ghost"), "pack.none", "g-fixture")
            .unwrap_err()
            .to_string()
            .contains("ghost"));
        // Whole-graph projection carries the census with no anchor block.
        let whole = project_ir(&ir, None, "pack.none", "g-fixture").unwrap();
        assert!(whole.serialize_canonical().contains("anchor: none\n"));
    }

    /// Distinct projections hash distinctly (spot the obvious collisions:
    /// anchor none vs empty lists; count moved between kinds).
    #[test]
    fn distinct_contexts_hash_distinctly() {
        let base = fixture();
        let no_anchor =
            ContextProjection::new("pack.demo@0", "g-abc123", None, vec![("start".into(), 1)])
                .unwrap();
        let anchored_empty = ContextProjection::new(
            "pack.demo@0",
            "g-abc123",
            Some(AnchorContext {
                node: ns("start", "start"),
                predecessors: vec![],
                successors: vec![],
                attached_guards: vec![],
            }),
            vec![("start".into(), 1)],
        )
        .unwrap();
        assert_ne!(base.hash(), no_anchor.hash());
        assert_ne!(no_anchor.hash(), anchored_empty.hash());
    }
}
