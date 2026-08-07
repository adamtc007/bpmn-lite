//! Private deterministic matcher for governed, non-authoritative motifs.

use std::collections::BTreeMap;

use designer_graph::schema::DesignerDag;
use semantic_pack::{CompiledPack, MotifFactPatternSource, MotifSource};

pub(super) const MAX_GRAPH_FACTS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MotifState {
    Active,
    Completed,
    Abandoned,
    Inactive,
}

pub(super) struct MotifMatch<'a> {
    pub(super) source: &'a MotifSource,
    pub(super) state: MotifState,
}

fn matches(patterns: &[MotifFactPatternSource], facts: &BTreeMap<String, bool>) -> bool {
    patterns.iter().all(|pattern| {
        facts.get(pattern.fact.as_str()).copied().unwrap_or(false) == pattern.expected
    })
}

pub(super) fn graph_facts(dag: &DesignerDag) -> anyhow::Result<BTreeMap<String, bool>> {
    use bpmn_lite_compiler::IRNode;

    let graph = dag.to_ir()?;
    let mut facts = BTreeMap::new();
    facts.insert("graph.has_flow_node".to_string(), graph.node_count() > 0);
    facts.insert(
        "graph.has_guard_host".to_string(),
        graph.node_weights().any(|node| {
            matches!(
                node,
                IRNode::ServiceTask { .. }
                    | IRNode::HumanWait { .. }
                    | IRNode::MessageWait { .. }
                    | IRNode::SendTask { .. }
            )
        }),
    );
    facts.insert(
        "graph.has_boundary_guard".to_string(),
        graph.node_weights().any(|node| {
            matches!(
                node,
                IRNode::BoundaryTimer { .. } | IRNode::BoundaryError { .. }
            )
        }),
    );
    facts.insert(
        "graph.has_human_wait".to_string(),
        graph
            .node_weights()
            .any(|node| matches!(node, IRNode::HumanWait { .. })),
    );
    facts.insert(
        "graph.has_message_wait".to_string(),
        graph
            .node_weights()
            .any(|node| matches!(node, IRNode::MessageWait { .. })),
    );
    facts.insert(
        "graph.has_send_task".to_string(),
        graph
            .node_weights()
            .any(|node| matches!(node, IRNode::SendTask { .. })),
    );
    if facts.len() > MAX_GRAPH_FACTS {
        anyhow::bail!("graph fact projection exceeds {MAX_GRAPH_FACTS} facts");
    }
    Ok(facts)
}

pub(super) fn match_pack<'a>(
    pack: &'a CompiledPack,
    facts: &BTreeMap<String, bool>,
) -> Vec<MotifMatch<'a>> {
    pack.motifs()
        .iter()
        .map(|source| {
            let state = if !source.abandonment_conditions.is_empty()
                && matches(&source.abandonment_conditions, facts)
            {
                MotifState::Abandoned
            } else if !source.completion_conditions.is_empty()
                && matches(&source.completion_conditions, facts)
            {
                MotifState::Completed
            } else if matches(&source.preconditions, facts) {
                MotifState::Active
            } else {
                MotifState::Inactive
            };
            MotifMatch { source, state }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bpmn_pack::compiled_semantic_pack;

    #[test]
    fn governed_motif_lifecycle_is_deterministic() {
        let pack = compiled_semantic_pack();
        let states = |facts: BTreeMap<String, bool>| {
            match_pack(pack, &facts)
                .into_iter()
                .map(|matched| (matched.source.id.as_str().to_string(), matched.state))
                .collect::<BTreeMap<_, _>>()
        };
        let active = states(BTreeMap::from([
            ("graph.has_guard_host".into(), true),
            ("graph.has_boundary_guard".into(), false),
            ("graph.has_flow_node".into(), true),
            ("graph.has_message_wait".into(), false),
            ("graph.has_send_task".into(), false),
        ]));
        assert_eq!(active["motif.reminder_then_escalate"], MotifState::Active);
        assert_eq!(active["motif.request_and_wait"], MotifState::Active);

        let completed = states(BTreeMap::from([
            ("graph.has_guard_host".into(), true),
            ("graph.has_boundary_guard".into(), true),
            ("graph.has_flow_node".into(), true),
            ("graph.has_message_wait".into(), true),
            ("graph.has_send_task".into(), true),
        ]));
        assert_eq!(
            completed["motif.reminder_then_escalate"],
            MotifState::Completed
        );
        assert_eq!(completed["motif.request_and_wait"], MotifState::Completed);

        let abandoned = states(BTreeMap::from([
            ("graph.has_guard_host".into(), false),
            ("graph.has_flow_node".into(), false),
        ]));
        assert_eq!(
            abandoned["motif.reminder_then_escalate"],
            MotifState::Abandoned
        );
        assert_eq!(abandoned["motif.request_and_wait"], MotifState::Abandoned);
    }
}
