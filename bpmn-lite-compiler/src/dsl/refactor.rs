use crate::dsl::ast::*;

/// Formatting trait to serialize AST nodes back to BPMN-DSL S-expressions.
pub trait ToSexpr {
    fn to_sexpr(&self, indent_level: usize) -> String;
}

impl ToSexpr for WorkflowSource {
    fn to_sexpr(&self, indent_level: usize) -> String {
        let indent = " ".repeat(indent_level);
        let mut out = format!("{}(workflow {}\n", indent, self.name);
        for node in &self.nodes {
            out.push_str(&node.to_sexpr(indent_level + 2));
            out.push('\n');
        }
        out.push_str(&format!("{})", indent));
        out
    }
}

impl ToSexpr for NodeAst {
    fn to_sexpr(&self, indent: usize) -> String {
        match self {
            Self::Start(n) => n.to_sexpr(indent),
            Self::End(n) => n.to_sexpr(indent),
            Self::Task(n) => n.to_sexpr(indent),
            Self::Split(n) => n.to_sexpr(indent),
            Self::Join(n) => n.to_sexpr(indent),
            Self::Loop(n) => n.to_sexpr(indent),
        }
    }
}

impl ToSexpr for StartAst {
    fn to_sexpr(&self, indent: usize) -> String {
        format!(
            "{}(start-event :id {} :next {})",
            " ".repeat(indent),
            self.id,
            self.next
        )
    }
}

impl ToSexpr for EndAst {
    fn to_sexpr(&self, indent: usize) -> String {
        if self.status.is_empty() {
            format!("{}(end-event :id {})", " ".repeat(indent), self.id)
        } else {
            format!(
                "{}(end-event :id {} :status \"{}\")",
                " ".repeat(indent),
                self.id,
                self.status
            )
        }
    }
}

impl ToSexpr for TaskAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let mut args_str = String::new();
        if !self.args.is_empty() {
            args_str.push_str(" :args (");
            let pairs: Vec<String> = self
                .args
                .iter()
                .map(|(k, v)| format!(":{} \"{}\"", k, v))
                .collect();
            args_str.push_str(&pairs.join(" "));
            args_str.push(')');
        }

        let delivery_str = self
            .delivery_mode
            .as_ref()
            .map(|d| format!(" :delivery-mode \"{}\"", d))
            .unwrap_or_default();

        format!(
            "{}(service-task :id {} :verb {}{}{} :next {})",
            pad, self.id, self.plug, args_str, delivery_str, self.next
        )
    }
}

impl ToSexpr for SplitAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let inner_pad = " ".repeat(indent + 2);

        let mode_str = match self.mode {
            SplitModeAst::Xor => "exclusive-gateway",
            SplitModeAst::Or => "inclusive-gateway",
            SplitModeAst::And => "parallel-gateway",
        };

        let plug_str = self
            .plug
            .as_ref()
            .map(|p| format!(" :plug {}", p))
            .unwrap_or_default();

        let mut out = format!(
            "{}({} :id {}{}{} :join {}\n",
            pad, mode_str, self.id, plug_str, "", self.join
        );
        for flow in &self.flows {
            let cond_str = flow
                .condition
                .as_ref()
                .map(|c| match c {
                    ConditionAst::Eq { placeholder, value } => {
                        format!(" :condition (= {} \"{}\")", placeholder, value)
                    }
                })
                .unwrap_or_default();
            out.push_str(&format!(
                "{}(flow{} :next {})\n",
                inner_pad, cond_str, flow.next
            ));
        }
        out.push_str(&format!("{})", pad));
        out
    }
}

impl ToSexpr for JoinAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let mode_str = match self.mode {
            JoinModeAst::Xor => "join-xor",
            JoinModeAst::Or => "join-or",
            JoinModeAst::And => "join-and",
        };
        format!(
            "{}({} :id {} :split {} :next {})",
            " ".repeat(indent),
            mode_str,
            self.id,
            self.split,
            self.next
        )
    }
}

impl ToSexpr for LoopAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let inner_pad = " ".repeat(indent + 2);
        let mut out = format!(
            "{}(loop :id {} :ceiling {} :body (\n",
            pad, self.id, self.ceiling
        );
        for node in &self.body {
            out.push_str(&node.to_sexpr(indent + 4));
            out.push('\n');
        }
        out.push_str(&format!("{})\n", inner_pad));
        out.push_str(&format!("{}:next {})", inner_pad, self.next));
        out
    }
}

pub struct AstMutator<'a> {
    pub workflow: &'a mut WorkflowSource,
}

impl<'a> AstMutator<'a> {
    pub fn new(workflow: &'a mut WorkflowSource) -> Self {
        Self { workflow }
    }

    /// Finds a mutable reference to a node by ID (including nested loop bodies)
    pub fn find_node_mut(&mut self, id: &str) -> Option<&mut NodeAst> {
        Self::find_node_in_slice_mut(&mut self.workflow.nodes, id)
    }

    fn find_node_in_slice_mut<'b>(nodes: &'b mut [NodeAst], id: &str) -> Option<&'b mut NodeAst> {
        for node in nodes {
            if node.id() == id {
                return Some(node);
            }
            if let NodeAst::Loop(lp) = node {
                if let Some(n) = Self::find_node_in_slice_mut(&mut lp.body, id) {
                    return Some(n);
                }
            }
        }
        None
    }

    /// Rewires the execution path exiting node `from_id` to point to `to_id`.
    pub fn rewire_next(&mut self, from_id: &str, to_id: &str) -> Result<(), String> {
        let node = self
            .find_node_mut(from_id)
            .ok_or_else(|| format!("Node '{}' not found", from_id))?;

        match node {
            NodeAst::Start(st) => st.next = to_id.to_string(),
            NodeAst::Task(tk) => tk.next = to_id.to_string(),
            NodeAst::Join(jn) => jn.next = to_id.to_string(),
            NodeAst::Loop(lp) => lp.next = to_id.to_string(),
            NodeAst::End(_) => return Err("Cannot rewire 'next' on an end event".into()),
            NodeAst::Split(_) => {
                return Err("Cannot directly rewire Split next; edit flow paths instead".into());
            }
        }
        Ok(())
    }

    /// Inserts a new node directly after an existing node, rewiring connections automatically.
    pub fn insert_after(
        &mut self,
        predecessor_id: &str,
        mut new_node: NodeAst,
    ) -> Result<(), String> {
        // 1. Get next ID of predecessor
        let orig_next = {
            let pred_node = self
                .find_node_mut(predecessor_id)
                .ok_or_else(|| format!("Predecessor '{}' not found", predecessor_id))?;

            match pred_node {
                NodeAst::Start(st) => st.next.clone(),
                NodeAst::Task(tk) => tk.next.clone(),
                NodeAst::Join(jn) => jn.next.clone(),
                NodeAst::Loop(lp) => lp.next.clone(),
                NodeAst::Split(_) => {
                    return Err("Target is a Split node. Insert into branches instead.".into())
                }
                NodeAst::End(_) => return Err("Cannot insert after an End event.".into()),
            }
        };

        // 2. Set new node's next to the predecessor's original next
        match &mut new_node {
            NodeAst::Start(_) => return Err("Cannot insert a Start event".into()),
            NodeAst::Task(tk) => tk.next = orig_next,
            NodeAst::Join(jn) => jn.next = orig_next,
            NodeAst::Loop(lp) => lp.next = orig_next,
            NodeAst::End(_) => {}
            NodeAst::Split(_) => return Err("Cannot insert a Split node directly via insert_after; use specialized refactoring macros".into()),
        }

        // 3. Rewire predecessor to point to the new node
        self.rewire_next(predecessor_id, new_node.id())?;

        // 4. Inject the new node into the list containing the predecessor
        self.inject_into_same_scope(predecessor_id, new_node)?;
        Ok(())
    }

    fn inject_into_same_scope(
        &mut self,
        sibling_id: &str,
        node_to_insert: NodeAst,
    ) -> Result<(), String> {
        fn inject(nodes: &mut Vec<NodeAst>, sibling_id: &str, to_insert: NodeAst) -> bool {
            let mut pos = None;
            for (idx, node) in nodes.iter_mut().enumerate() {
                if node.id() == sibling_id {
                    pos = Some(idx);
                    break;
                }
                if let NodeAst::Loop(lp) = node {
                    if inject(&mut lp.body, sibling_id, to_insert.clone()) {
                        return true;
                    }
                }
            }
            if let Some(idx) = pos {
                nodes.insert(idx + 1, to_insert);
                true
            } else {
                false
            }
        }

        if inject(&mut self.workflow.nodes, sibling_id, node_to_insert) {
            Ok(())
        } else {
            Err(format!("Could not find sibling scope for '{}'", sibling_id))
        }
    }

    /// Removes a node by ID (including nested loop bodies) and returns it.
    pub fn remove_node(&mut self, id: &str) -> Option<NodeAst> {
        Self::remove_node_from_slice(&mut self.workflow.nodes, id)
    }

    fn remove_node_from_slice(nodes: &mut Vec<NodeAst>, id: &str) -> Option<NodeAst> {
        let mut found_idx = None;
        for (idx, node) in nodes.iter_mut().enumerate() {
            if node.id() == id {
                found_idx = Some(idx);
                break;
            }
            if let NodeAst::Loop(lp) = node {
                if let Some(n) = Self::remove_node_from_slice(&mut lp.body, id) {
                    return Some(n);
                }
            }
        }
        found_idx.map(|idx| nodes.remove(idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::compile;
    use crate::dsl::linter::StubPlaceholderRegistry;
    use crate::dsl::macros::create_bounded_retry_macro;
    use bpmn_lite_types::SourceSpan;

    fn parse_sexpr(source: &str) -> WorkflowSource {
        let (tokens, _) = crate::dsl::lexer::lex(source);
        let mut p = crate::dsl::parser::Parser::new(tokens);
        p.parse_workflow().expect("parse failed")
    }

    #[test]
    fn test_programmatic_insert_and_serialization() {
        let input = r#"(workflow test
  (start-event :id start :next end)
  (end-event :id end :status "completed")
)"#;

        let mut workflow = parse_sexpr(input);
        let mut mutator = AstMutator::new(&mut workflow);

        let new_task = NodeAst::Task(TaskAst {
            id: "new-task".to_string(),
            plug: "cbu.create".to_string(),
            args: vec![],
            next: "".to_string(),
            delivery_mode: None,
            span: SourceSpan::new(0, 0),
        });

        mutator.insert_after("start", new_task).unwrap();
        let output_code = workflow.to_sexpr(0);

        assert!(output_code.contains("(start-event :id start :next new-task)"));
        assert!(output_code.contains("(service-task :id new-task :verb cbu.create :next end)"));
    }

    #[test]
    fn test_retry_loop_macro_sese_validation() {
        let input = r#"(workflow onboarding
  (start-event :id start :next charge-card)
  (service-task :id charge-card :verb billing.charge :next end)
  (end-event :id end :status "completed")
)"#;

        let mut workflow = parse_sexpr(input);
        let mut mutator = AstMutator::new(&mut workflow);

        let target = match mutator.remove_node("charge-card").unwrap() {
            NodeAst::Task(t) => t,
            _ => panic!("Expected task"),
        };
        mutator.rewire_next("start", "end").unwrap();

        let loop_node = NodeAst::Loop(create_bounded_retry_macro(
            target,
            3,
            "charge-retry-loop",
            "end",
        ));

        mutator.insert_after("start", loop_node).unwrap();
        let final_dsl = workflow.to_sexpr(0);
        println!("FINAL DSL:\n{}", final_dsl);

        let mut registry = StubPlaceholderRegistry::new().with_demo_bindings();
        registry.register_verb("billing.charge", crate::dsl::linter::BindingDecl::default());

        let plan = compile(&final_dsl, &registry).expect("Compilation failed");
        assert!(plan.mathematically_proved);
        assert!(plan.unsafe_breeches.is_empty());
    }
}
