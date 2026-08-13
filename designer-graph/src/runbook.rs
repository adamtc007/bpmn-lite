//! G6.2 — render the operation tape as readable Designer-DSL. The tape
//! (`Vec<Operation>`, folded by `apply_production`) is already canonical
//! and already the authoritative state (schema.rs's own module doc: "the
//! durable surface is the edit log... the DAG is its replay product");
//! this is purely a rendering job, one S-expression-shaped line per
//! operation, mirroring `bpmn-lite-compiler`'s `ToSexpr` syntax (`(kind
//! :field value ...)`) without depending on it — that trait operates on
//! the parsed DSL-source AST, a different data model than the Designer
//! operation tape.
//!
//! No round-trip is claimed or attempted: this is read-only session
//! review text, not a re-parseable program.

use crate::ops::{GuardTrigger, Operation, RegionBranch};
use bpmn_lite_compiler::IRNode;

fn ir_node_sexpr(node: &IRNode) -> String {
    match node {
        IRNode::Start { id } => format!("(start :id {id:?})"),
        IRNode::End { id, terminate } => format!("(end :id {id:?} :terminate {terminate})"),
        IRNode::ServiceTask { id, name, task_type } => {
            format!("(service-task :id {id:?} :name {name:?} :task-type {task_type:?})")
        }
        IRNode::GatewayXor { id, name } => format!("(gateway-xor :id {id:?} :name {name:?})"),
        IRNode::GatewayAnd { id, name, direction } => {
            format!("(gateway-and :id {id:?} :name {name:?} :direction {direction:?})")
        }
        IRNode::GatewayInclusive { id, name, direction } => {
            format!("(gateway-inclusive :id {id:?} :name {name:?} :direction {direction:?})")
        }
        IRNode::TimerWait { id, spec } => format!("(timer-wait :id {id:?} :spec {spec:?})"),
        IRNode::MessageWait { id, name, corr_key_source } => format!(
            "(message-wait :id {id:?} :name {name:?} :correlation {corr_key_source:?})"
        ),
        IRNode::HumanWait { id, name, task_kind, corr_key_source } => format!(
            "(human-wait :id {id:?} :name {name:?} :task-kind {task_kind:?} :correlation {corr_key_source:?})"
        ),
        IRNode::BoundaryTimer { id, attached_to, spec, interrupting, failure_budget } => format!(
            "(boundary-timer :id {id:?} :attached-to {attached_to:?} :spec {spec:?} :interrupting {interrupting} :failure-budget {failure_budget:?})"
        ),
        IRNode::BoundaryError { id, attached_to, error_code, failure_budget } => format!(
            "(boundary-error :id {id:?} :attached-to {attached_to:?} :error-code {error_code:?} :failure-budget {failure_budget:?})"
        ),
        other => format!("({:?})", other),
    }
}

fn branch_sexpr(branch: &RegionBranch) -> String {
    format!(
        "(branch :in {:?} :out {:?} :condition {:?} :node {})",
        branch.in_edge_id,
        branch.out_edge_id,
        branch.condition,
        ir_node_sexpr(&branch.node)
    )
}

fn guard_trigger_sexpr(trigger: &GuardTrigger) -> String {
    match trigger {
        GuardTrigger::Timer(spec) => format!("(timer {spec:?})"),
        GuardTrigger::Error { error_code } => format!("(error :code {error_code:?})"),
    }
}

/// Render a single `Operation` as one readable, S-expression-shaped line.
/// Field order matches the operation's own doc comment in `ops.rs`.
pub fn render_operation(op: &Operation) -> String {
    match op {
        Operation::InsertAfter { anchor, key, node, edge_id } => format!(
            "(insert-after :anchor {:?} :key {:?} :edge {edge_id:?} :as {})",
            anchor.0, key.0, ir_node_sexpr(node)
        ),
        Operation::InsertBefore { anchor, key, node, edge_id } => format!(
            "(insert-before :anchor {:?} :key {:?} :edge {edge_id:?} :as {})",
            anchor.0, key.0, ir_node_sexpr(node)
        ),
        Operation::AppendNode { anchor, key, node, edge_id } => format!(
            "(append-node :anchor {:?} :key {:?} :edge {edge_id:?} :as {})",
            anchor.0, key.0, ir_node_sexpr(node)
        ),
        Operation::ReplaceNode { target, key, node } => format!(
            "(replace-node :target {:?} :key {:?} :as {})",
            target.0, key.0, ir_node_sexpr(node)
        ),
        Operation::Connect { from, to, edge_id, condition } => format!(
            "(connect :from {:?} :to {:?} :edge {edge_id:?} :condition {:?})",
            from.0, to.0, condition
        ),
        Operation::DeleteNode { target } => format!("(delete-node :target {:?})", target.0),
        Operation::AttachGuard { host, key, guard_id, trigger } => format!(
            "(attach-guard :host {:?} :key {:?} :guard {guard_id:?} :trigger {})",
            host.0, key.0, guard_trigger_sexpr(trigger)
        ),
        Operation::AttachRearmingGuard { host, key, guard_id, trigger } => format!(
            "(attach-rearming-guard :host {:?} :key {:?} :guard {guard_id:?} :trigger {})",
            host.0, key.0, guard_trigger_sexpr(trigger)
        ),
        Operation::SetGuardTrigger { guard, trigger } => format!(
            "(set-guard-trigger :guard {:?} :trigger {})",
            guard.0, guard_trigger_sexpr(trigger)
        ),
        Operation::SetGuardBudget { guard, failure_budget } => format!(
            "(set-guard-budget :guard {:?} :failure-budget {failure_budget:?})",
            guard.0
        ),
        Operation::SetDefaultGuardBudget { failure_budget } => {
            format!("(set-default-guard-budget :failure-budget {failure_budget:?})")
        }
        Operation::SetDefaultRetryPolicy { policy } => {
            format!("(set-default-retry-policy :policy {policy:?})")
        }
        Operation::SetCorrelationSource { node, corr_key_source } => format!(
            "(set-correlation-source :node {:?} :correlation {corr_key_source:?})",
            node.0
        ),
        Operation::CreateDataObject { key, id, name, type_decl, role } => format!(
            "(create-data-object :key {:?} :id {id:?} :name {name:?} :type {type_decl:?} :role {role:?})",
            key.0
        ),
        Operation::CreateParallelRegion {
            anchor, fork_key, fork_node_id, join_key, join_node_id, entry_edge_id, branches,
        } => format!(
            "(create-parallel-region :anchor {:?} :fork-key {:?} :fork {fork_node_id:?} :join-key {:?} :join {join_node_id:?} :entry-edge {entry_edge_id:?} :branches ({}))",
            anchor.0,
            fork_key.0,
            join_key.0,
            branches.iter().map(branch_sexpr).collect::<Vec<_>>().join(" "),
        ),
        Operation::CreateInclusiveRegion {
            anchor, fork_key, fork_node_id, join_key, join_node_id, entry_edge_id, branches,
        } => format!(
            "(create-inclusive-region :anchor {:?} :fork-key {:?} :fork {fork_node_id:?} :join-key {:?} :join {join_node_id:?} :entry-edge {entry_edge_id:?} :branches ({}))",
            anchor.0,
            fork_key.0,
            join_key.0,
            branches.iter().map(branch_sexpr).collect::<Vec<_>>().join(" "),
        ),
        Operation::CreateMultiInstanceRegion { anchor, key, node, edge_id } => format!(
            "(create-multi-instance-region :anchor {:?} :key {:?} :edge {edge_id:?} :as {})",
            anchor.0, key.0, ir_node_sexpr(node)
        ),
        Operation::CreateBranch { gateway, target, edge_id, condition } => format!(
            "(create-branch :gateway {:?} :target {:?} :edge {edge_id:?} :condition {:?})",
            gateway.0, target.0, condition
        ),
    }
}

/// Render a full operation tape (one `GraphEdit` payload's `Vec<Operation>`,
/// or a whole session's folded tape) as readable Designer-DSL text, one
/// operation per line.
pub fn render_runbook<'a>(ops: impl IntoIterator<Item = &'a Operation>) -> String {
    ops.into_iter()
        .map(render_operation)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::NodeKey;
    use uuid::Uuid;

    fn key(byte: u8) -> NodeKey {
        NodeKey(Uuid::from_bytes([byte; 16]))
    }

    #[test]
    fn append_node_renders_a_readable_sexpr_line() {
        let op = Operation::AppendNode {
            anchor: key(1),
            key: key(2),
            node: IRNode::ServiceTask {
                id: "t1".into(),
                name: "Do it".into(),
                task_type: "verb:pack.do".into(),
            },
            edge_id: "e1".into(),
        };
        let rendered = render_operation(&op);
        assert!(rendered.starts_with("(append-node"));
        assert!(rendered.contains("\"t1\""));
        assert!(rendered.contains("service-task"));
        assert!(rendered.contains("verb:pack.do"));
    }

    #[test]
    fn render_runbook_joins_one_line_per_operation() {
        let ops = vec![
            Operation::DeleteNode { target: key(3) },
            Operation::SetDefaultGuardBudget { failure_budget: Some(4) },
        ];
        let rendered = render_runbook(&ops);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("(delete-node"));
        assert!(lines[1].starts_with("(set-default-guard-budget"));
    }
}
