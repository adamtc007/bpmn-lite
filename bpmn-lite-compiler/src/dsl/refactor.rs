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
            Self::MessageWait(n) => n.to_sexpr(indent),
            Self::TimerWait(n) => n.to_sexpr(indent),
            Self::Split(n) => n.to_sexpr(indent),
            Self::Join(n) => n.to_sexpr(indent),
            Self::Loop(n) => n.to_sexpr(indent),
            Self::BoundaryTimer(n) => n.to_sexpr(indent),
            Self::BoundaryError(n) => n.to_sexpr(indent),
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

impl ToSexpr for MessageWaitAst {
    fn to_sexpr(&self, indent: usize) -> String {
        format!(
            "{}(message-wait :id {} :name {} :correlation-source {} :next {})",
            " ".repeat(indent),
            self.id,
            self.name,
            self.correlation_source,
            self.next
        )
    }
}

impl ToSexpr for SplitAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        let inner_pad = " ".repeat(indent + 2);

        // Head keywords must be ones `parser.rs`'s node-kind match actually
        // accepts for this attribute shape (`parse_split`: :id [:plug] :join
        // then (flow ...) children). The gateway-style names this printer
        // used before ("exclusive-gateway"/"inclusive-gateway"/
        // "parallel-gateway") either aren't parseable heads at all (And/Or)
        // or route to the legacy parse fn with a different attribute shape
        // (Xor: no :join, join id synthesized) — so printed splits never
        // re-parsed. Found and fixed under EOP-PLAN-GRAPH-DSL-BRIDGE-001 B0;
        // round-trip cement tests below. Note the grammar still cannot
        // express a plug-less Xor/Or split or a conditioned And flow — an
        // AST in one of those states prints to source the parser rejects;
        // that is a parser/AST asymmetry owned by the DSL-parity programme,
        // not paperable here.
        let mode_str = match self.mode {
            SplitModeAst::Xor => "split-xor",
            SplitModeAst::Or => "split-or",
            SplitModeAst::And => "split-and",
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

impl ToSexpr for BoundaryTimerAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let spec_str = match &self.spec {
            crate::ir::TimerSpec::Duration { ms } => format!(":duration-ms {ms}"),
            crate::ir::TimerSpec::Date { deadline_ms } => format!(":deadline-ms {deadline_ms}"),
            crate::ir::TimerSpec::Cycle {
                interval_ms,
                max_fires,
            } => format!(":cycle-ms {interval_ms} :max-fires {max_fires}"),
        };
        let budget_str = self
            .budget
            .map(|b| format!(" :budget {b}"))
            .unwrap_or_default();
        format!(
            "{}(boundary-timer :id {} :host {} {} :interrupting {}{} :next {})",
            " ".repeat(indent),
            self.id,
            self.host,
            spec_str,
            self.interrupting,
            budget_str,
            self.next
        )
    }
}

impl ToSexpr for BoundaryErrorAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let code_str = self
            .error_code
            .as_ref()
            .map(|c| format!(" :error-code \"{c}\""))
            .unwrap_or_default();
        let budget_str = self
            .budget
            .map(|b| format!(" :budget {b}"))
            .unwrap_or_default();
        format!(
            "{}(boundary-error :id {} :host {}{}{} :next {})",
            " ".repeat(indent),
            self.id,
            self.host,
            code_str,
            budget_str,
            self.next
        )
    }
}

impl ToSexpr for TimerWaitAst {
    fn to_sexpr(&self, indent: usize) -> String {
        let spec_str = match &self.spec {
            crate::ir::TimerSpec::Duration { ms } => format!(":duration-ms {ms}"),
            crate::ir::TimerSpec::Date { deadline_ms } => format!(":deadline-ms {deadline_ms}"),
            crate::ir::TimerSpec::Cycle {
                interval_ms,
                max_fires,
            } => format!(":cycle-ms {interval_ms} :max-fires {max_fires}"),
        };
        format!(
            "{}(timer-wait :id {} {} :next {})",
            " ".repeat(indent),
            self.id,
            spec_str,
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
            NodeAst::MessageWait(wait) => wait.next = to_id.to_string(),
            NodeAst::TimerWait(wait) => wait.next = to_id.to_string(),
            NodeAst::Join(jn) => jn.next = to_id.to_string(),
            NodeAst::Loop(lp) => lp.next = to_id.to_string(),
            NodeAst::BoundaryTimer(g) => g.next = to_id.to_string(),
            NodeAst::BoundaryError(g) => g.next = to_id.to_string(),
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
                NodeAst::MessageWait(wait) => wait.next.clone(),
                NodeAst::TimerWait(wait) => wait.next.clone(),
                NodeAst::Join(jn) => jn.next.clone(),
                NodeAst::Loop(lp) => lp.next.clone(),
                NodeAst::BoundaryTimer(g) => g.next.clone(),
                NodeAst::BoundaryError(g) => g.next.clone(),
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
            NodeAst::MessageWait(wait) => wait.next = orig_next,
            NodeAst::TimerWait(wait) => wait.next = orig_next,
            NodeAst::Join(jn) => jn.next = orig_next,
            NodeAst::Loop(lp) => lp.next = orig_next,
            NodeAst::BoundaryTimer(_) | NodeAst::BoundaryError(_) => {
                return Err(
                    "Cannot insert a boundary guard via insert_after: guards attach to a :host, not to sequence flow".into(),
                )
            }
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
        // Fail on ANY lex/parse error, not just a missing workflow: the
        // parser's recovery loop silently drops a failed node, so an
        // error-swallowing helper "proves" round-trips on a workflow
        // missing the very node under test (B0 blind-review finding — a
        // quoted-vs-symbol mismatch in a message-wait fixture hid exactly
        // that way).
        let (tokens, lex_errs) = crate::dsl::lexer::lex(source);
        assert!(lex_errs.is_empty(), "lex errors in fixture: {lex_errs:?}");
        let mut p = crate::dsl::parser::Parser::new(tokens);
        let wf = p.parse_workflow();
        let errs = p.into_errors();
        assert!(errs.is_empty(), "parse errors in fixture: {errs:?}");
        wf.expect("parse failed")
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
            loop_origin: None,
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

    /// Print → re-parse → print must be a fixpoint, and the printed source
    /// must re-parse without a single parser error. Cement for the split
    /// head-keyword desync found under EOP-PLAN-GRAPH-DSL-BRIDGE-001 B0:
    /// the printer used to emit "parallel-gateway"/"inclusive-gateway"
    /// heads no parser arm accepts (and "exclusive-gateway", whose legacy
    /// parse fn takes a different attribute shape), so printed splits never
    /// re-parsed — breaking AstMutator's regenerate-and-recompile path for
    /// any workflow containing a split.
    fn assert_print_reparse_fixpoint(source: &str) -> String {
        let wf1 = parse_sexpr(source);
        let p1 = wf1.to_sexpr(0);
        let (tokens, lex_errs) = crate::dsl::lexer::lex(&p1);
        assert!(lex_errs.is_empty(), "lex errors on printed source:\n{p1}");
        let mut parser = crate::dsl::parser::Parser::new(tokens);
        let wf2 = parser.parse_workflow();
        let errs = parser.into_errors();
        assert!(
            errs.is_empty(),
            "printed source does not re-parse: {errs:?}\nprinted:\n{p1}"
        );
        let p2 = wf2.expect("printed source parsed to no workflow").to_sexpr(0);
        assert_eq!(p1, p2, "print→parse→print is not a fixpoint");
        p1
    }

    #[test]
    fn split_and_print_reparse_roundtrip_and_recompiles() {
        let source = r#"(workflow test-and-roundtrip
  (start-event :id start :next split-gateway)
  (split-and :id split-gateway :join join-gateway
    (flow :next prod-1)
    (flow :next prod-2))
  (service-task :id prod-1 :verb cbu.produce-part1 :next join-gateway)
  (service-task :id prod-2 :verb cbu.produce-part2 :next join-gateway)
  (join-and :id join-gateway :split split-gateway :next end)
  (end-event :id end :status "done"))"#;
        let printed = assert_print_reparse_fixpoint(source);

        let mut registry = StubPlaceholderRegistry::new();
        registry.register_verb(
            "cbu.produce-part1",
            crate::dsl::linter::BindingDecl::default(),
        );
        registry.register_verb(
            "cbu.produce-part2",
            crate::dsl::linter::BindingDecl::default(),
        );
        compile(&printed, &registry).expect("printed split-and source must recompile");
    }

    #[test]
    fn split_xor_print_reparse_roundtrip() {
        // :plug and per-flow :condition are grammatically REQUIRED for
        // non-And splits (parse_split/parse_split_flow), so the fixture
        // carries both — a plug-less Xor/Or AST is unprintable-as-parseable
        // today (parser/AST asymmetry owned by the DSL-parity programme).
        let source = r#"(workflow test-xor-roundtrip
  (start-event :id start :next type-gateway)
  (split-xor :id type-gateway :plug cbu_type_routing :join type-gateway-join
    (flow :condition (= @cbu-type "fund") :next type-gateway-join)
    (flow :condition (= @cbu-type "corporate") :next type-gateway-join))
  (join-xor :id type-gateway-join :split type-gateway :next end)
  (end-event :id end :status "done"))"#;
        assert_print_reparse_fixpoint(source);
    }

    #[test]
    fn split_or_print_reparse_roundtrip() {
        let source = r#"(workflow test-or-roundtrip
  (start-event :id start :next type-gateway)
  (split-or :id type-gateway :plug cbu_type_routing :join type-gateway-join
    (flow :condition (= @cbu-type "fund") :next type-gateway-join)
    (flow :condition (= @cbu-type "corporate") :next type-gateway-join))
  (join-or :id type-gateway-join :split type-gateway :next end)
  (end-event :id end :status "done"))"#;
        assert_print_reparse_fixpoint(source);
    }

    /// D1 fixpoint cement: every guard form and timer shape prints to
    /// source the parser accepts, byte-stably, and recompiles with the
    /// guards lowered onto the right hosts in guard-id order. (One timer
    /// per host — R32 — so the two timer shapes live on two hosts.)
    #[test]
    fn guard_forms_print_reparse_roundtrip_and_recompile() {
        let source = r#"(workflow test-guard-roundtrip
  (start-event :id start :next t1)
  (service-task :id t1 :verb cbu.host :next t2)
  (service-task :id t2 :verb cbu.host2 :next end)
  (boundary-timer :id g-dur :host t1 :duration-ms 60000 :interrupting true :next esc1)
  (boundary-timer :id g-cyc :host t2 :cycle-ms 30000 :max-fires 5 :interrupting false :budget 3 :next esc2)
  (boundary-error :id g-err :host t1 :error-code "E42" :budget 2 :next esc3)
  (boundary-error :id g-bare :host t2 :next esc4)
  (end-event :id esc1 :status "done")
  (end-event :id esc2 :status "done")
  (end-event :id esc3 :status "done")
  (end-event :id esc4 :status "done")
  (end-event :id end :status "completed"))"#;
        let printed = assert_print_reparse_fixpoint(source);
        let mut registry = StubPlaceholderRegistry::new();
        registry.register_verb("cbu.host", crate::dsl::linter::BindingDecl::default());
        registry.register_verb("cbu.host2", crate::dsl::linter::BindingDecl::default());
        let plan = compile(&printed, &registry).expect("guard source must compile");
        match plan.nodes().get("t1") {
            Some(crate::dsl::ExecutionNode::Task(t)) => {
                let ids: Vec<&str> = t.guards.iter().map(|g| g.guard_id.as_str()).collect();
                assert_eq!(ids, ["g-dur", "g-err"], "guard-id order on t1");
            }
            other => panic!("t1 must be a Task, got {other:?}"),
        }
        match plan.nodes().get("t2") {
            Some(crate::dsl::ExecutionNode::Task(t)) => {
                let ids: Vec<&str> = t.guards.iter().map(|g| g.guard_id.as_str()).collect();
                assert_eq!(ids, ["g-bare", "g-cyc"], "guard-id order on t2");
            }
            other => panic!("t2 must be a Task, got {other:?}"),
        }
    }

    /// D1 parse/lint reds R29-R35 — each names its exact refusal.
    #[test]
    fn guard_red_axes_refuse_at_parse_or_lint() {
        let reg = || {
            let mut r = StubPlaceholderRegistry::new();
            r.register_verb("cbu.host", crate::dsl::linter::BindingDecl::default());
            r
        };
        let wrap = |guards: &str| {
            format!(
                "(workflow red\n  (start-event :id start :next t1)\n  (service-task :id t1 :verb cbu.host :next end)\n{guards}\n  (end-event :id esc :status \"done\")\n  (end-event :id end :status \"completed\"))"
            )
        };
        let expect_err = |src: &str, needle: &str| {
            let err = compile(src, &reg()).expect_err("must refuse");
            let msg = err.to_string();
            assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
        };
        // R31 budget 0 (lint)
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 100 :interrupting true :budget 0 :next esc)"),
            ":budget 0",
        );
        // R32 second timer on one host (lint)
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 100 :interrupting true :next esc)\n  (boundary-timer :id g2 :host t1 :deadline-ms 999 :interrupting true :next esc)"),
            "already carries timer guard",
        );
        // R33 interrupting cycle (lint)
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :cycle-ms 100 :max-fires 2 :interrupting true :next esc)"),
            "interrupting cycle timer",
        );
        // R34 malformed integer (parse, named — never silent-zero)
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 10x :interrupting true :next esc)"),
            "not a valid non-negative integer",
        );
        // R35 non-boolean :interrupting (parse, named)
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 100 :interrupting maybe :next esc)"),
            "not a boolean",
        );
        // R30 missing :interrupting / double timer shape (parse).
        // Discriminating needle: the exact expected-keyword parse error,
        // not any message that happens to mention "interrupting".
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 100 :next esc)"),
            "expected ':interrupting'",
        );
        expect_err(
            &wrap("  (boundary-timer :id g1 :host t1 :duration-ms 100 :deadline-ms 5 :interrupting true :next esc)"),
            "more than one timer shape",
        );
        // R29 duplicate guard id vs node id (lint pass 2)
        expect_err(
            &wrap("  (boundary-error :id t1 :host t1 :next esc)"),
            "duplicate node id",
        );
        // R29 duplicate guard id vs OTHER guard id (lint pass 2 — the
        // second half of the freeze's collision rule, cemented per the
        // D1 blind review)
        expect_err(
            &wrap("  (boundary-error :id g1 :host t1 :next esc)\n  (boundary-error :id g1 :host t1 :next esc)"),
            "duplicate node id",
        );
        // Guard as a `:next` target — guards are in the AST id set but
        // never plan nodes, so lint must refuse or the escape/flow edge
        // dangles (validate_dag has no dangling-target check; found by
        // the D1 blind review, which compiled both shapes GREEN before
        // this fix). Escape-into-guard:
        expect_err(
            &wrap("  (boundary-error :id g1 :host t1 :next g2)\n  (boundary-error :id g2 :host t1 :next esc)"),
            "targets a boundary guard",
        );
        // ...and ordinary sequence flow into a guard (mirror of emit's
        // FlowIntoGuard on the DSL path):
        expect_err(
            "(workflow red\n  (start-event :id start :next t1)\n  (service-task :id t1 :verb cbu.host :next g1)\n  (boundary-error :id g1 :host t1 :next esc)\n  (end-event :id esc :status \"done\")\n  (end-event :id end :status \"completed\"))",
            "targets a boundary guard",
        );
        // unknown host / non-task host (lint)
        expect_err(
            &wrap("  (boundary-error :id g1 :host nope :next esc)"),
            "unknown node",
        );
        expect_err(
            &wrap("  (boundary-error :id g1 :host start :next esc)"),
            "not a service task",
        );
    }

    /// D2: all three timer-wait shapes print→parse→print fixpoint and
    /// recompile.
    #[test]
    fn timer_wait_forms_print_reparse_roundtrip_and_recompile() {
        let source = r#"(workflow test-timer-roundtrip
  (start-event :id start :next w-dur)
  (timer-wait :id w-dur :duration-ms 1000 :next w-date)
  (timer-wait :id w-date :deadline-ms 999 :next w-cyc)
  (timer-wait :id w-cyc :cycle-ms 60000 :max-fires 3 :next end)
  (end-event :id end :status "completed"))"#;
        assert_print_reparse_fixpoint(source);
        let reg = StubPlaceholderRegistry::new();
        compile(source, &reg).expect("timer-wait chain must compile");
    }

    /// D2 red axes: R-D2.1 double shape, R-D2.2 malformed int, R-D2.3
    /// missing shape — all named parse errors under the timer-wait head;
    /// discriminating needles per the D1-review convention.
    #[test]
    fn timer_wait_red_axes_refuse_at_parse() {
        let wrap = |node: &str| {
            format!(
                "(workflow red\n  (start-event :id start :next w1)\n{node}\n  (end-event :id end :status \"completed\"))"
            )
        };
        let expect_err = |src: &str, needle: &str| {
            let reg = StubPlaceholderRegistry::new();
            let err = compile(src, &reg).expect_err("must refuse");
            let msg = err.to_string();
            assert!(msg.contains(needle), "expected {needle:?} in: {msg}");
        };
        expect_err(
            &wrap("  (timer-wait :id w1 :duration-ms 100 :deadline-ms 5 :next end)"),
            "timer-wait carries more than one timer shape",
        );
        expect_err(
            &wrap("  (timer-wait :id w1 :duration-ms 10x :next end)"),
            "not a valid non-negative integer",
        );
        expect_err(
            &wrap("  (timer-wait :id w1 :cycle-ms 100 :max-fires 2x :next end)"),
            "not a valid u32 integer",
        );
        expect_err(
            &wrap("  (timer-wait :id w1 :next end)"),
            "timer-wait requires exactly one timer shape",
        );
    }

    #[test]
    fn linear_core_shapes_print_reparse_roundtrip() {
        // Start / Task / MessageWait / End — the linear members of the
        // graph→DSL bridge's core emission set, fixpoint-proven per
        // variant (the pre-existing tests only asserted substring
        // presence for task shapes).
        let source = r#"(workflow test-linear-roundtrip
  (start-event :id start :next fetch)
  (service-task :id fetch :verb cbu.create :next wait-reply)
  (message-wait :id wait-reply :name reply-received :correlation-source case-id :next end)
  (end-event :id end :status "terminated"))"#;
        assert_print_reparse_fixpoint(source);
    }
}
