//! WS-A.0 cement (C2-residual, EOP-PLAN-BPMN-DESIGN-003): the R8 pairing
//! oracle MUST stay consumable from a sibling crate with only public types.
//! This test IS the crate-boundary guarantee — a visibility regression on
//! `compute_post_dominators` / `compute_region_map` / `gateway_pairs` or on
//! the IR construction surface fails this build, not the future
//! `designer-graph` crate's. Do not weaken; do not move inside the compiler
//! crate (an internal test would not lock the boundary).

use bpmn_lite_compiler::{GatewayDirection, IREdge, IRGraph, IRNode};
use bpmn_lite_compiler::{compute_post_dominators, compute_region_map, gateway_pairs};

fn edge(id: &str) -> IREdge {
    IREdge {
        id: id.to_owned(),
        condition: None,
    }
}

/// start → fork(AND) → {t1, t2} → join(AND) → end, built entirely from
/// public types by an external crate, run through all three exported
/// oracle entry points.
#[test]
fn pairing_oracle_is_consumable_across_the_crate_boundary() {
    let mut graph: IRGraph = IRGraph::new();
    let start = graph.add_node(IRNode::Start { id: "start".into() });
    let fork = graph.add_node(IRNode::GatewayAnd {
        id: "fork".into(),
        name: "fork".into(),
        direction: GatewayDirection::Diverging,
    });
    let t1 = graph.add_node(IRNode::ServiceTask {
        id: "t1".into(),
        name: "t1".into(),
        task_type: "noop".into(),
        loop_origin: None,
    });
    let t2 = graph.add_node(IRNode::ServiceTask {
        id: "t2".into(),
        name: "t2".into(),
        task_type: "noop".into(),
        loop_origin: None,
    });
    let join = graph.add_node(IRNode::GatewayAnd {
        id: "join".into(),
        name: "join".into(),
        direction: GatewayDirection::Converging,
    });
    let end = graph.add_node(IRNode::End {
        id: "end".into(),
        terminate: false,
    });
    graph.add_edge(start, fork, edge("e1"));
    graph.add_edge(fork, t1, edge("e2"));
    graph.add_edge(fork, t2, edge("e3"));
    graph.add_edge(t1, join, edge("e4"));
    graph.add_edge(t2, join, edge("e5"));
    graph.add_edge(join, end, edge("e6"));

    let post_doms = compute_post_dominators(&graph);
    assert_eq!(
        post_doms.get(&fork),
        Some(&join),
        "the fork's immediate post-dominator must be its join"
    );

    // region_map's contract: diverging gateway -> its region-closing partner.
    let regions = compute_region_map(&graph, &post_doms);
    assert_eq!(
        regions.get(&fork),
        Some(&join),
        "region map must close the fork's region at its join"
    );

    let pairs = gateway_pairs(&graph);
    assert_eq!(
        pairs.get(&fork),
        Some(&join),
        "gateway_pairs must pair the diverging gateway with its converging partner"
    );
}
