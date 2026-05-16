//! Production bytecode stack VM.
//!
//! Executes a [`VerifiedDecision`]'s instruction stream against a
//! [`TypedInputContext`] and produces an [`EvaluationOutput`] with the same
//! contract as `reference::evaluate`.
//!
//! The VM **may short-circuit**: `BrFalse(addr)` jumps when the top of stack
//! is false, skipping subsequent predicate evaluations. This is valid per the
//! bytecode spec §4.2 — the output is identical to the reference evaluator
//! for all well-formed inputs, which Phase 1.5 will prove via differential
//! testing.

use std::cmp::Ordering;

use smallvec::SmallVec;

use dmn_lite_types::{
    EvalError, EvaluationTrace, FieldId, PredicateTrace, RuleId, RuleTrace, TraceOutcome,
    TypedInputContext, TypedOutputContext,
    compiled::{CompiledDecision, RangeEntry, RuleMapEntry, VerifiedDecision},
    ids::SourceSpan,
    instr::Instr,
    ir::{HitPolicy, TypedValue},
    values::compute_schema_hash,
};

use crate::reference::EvaluationOutput;

// ── Public API ────────────────────────────────────────────────────────────────

/// Evaluate a verified decision against a typed input context using the
/// stack VM (production execution path).
///
/// The `source` string is used for predicate `description` fields in the
/// evaluation trace.  Pass `""` if the source is unavailable.
pub fn evaluate(
    decision: &VerifiedDecision,
    input: &TypedInputContext,
    source: &str,
) -> Result<EvaluationOutput, EvalError> {
    let compiled = decision.as_compiled();
    run_vm(compiled, input, source)
}

// ── VM implementation ─────────────────────────────────────────────────────────

fn run_vm(
    compiled: &CompiledDecision,
    input: &TypedInputContext,
    source: &str,
) -> Result<EvaluationOutput, EvalError> {
    // Validate input schema
    let expected_hash = compute_schema_hash(&compiled.input_schema);
    if expected_hash != input.schema_hash {
        return Err(EvalError::SchemaHashMismatch);
    }
    if input.len() != compiled.input_schema.len() {
        return Err(EvalError::InputSchemaMismatch {
            expected: compiled.input_schema.len(),
            actual: input.len(),
        });
    }

    let mut state = VmState::new(compiled, input, source);
    loop {
        if state.halted || state.pc as usize >= compiled.instructions.len() {
            break;
        }
        step(&mut state, compiled, input)?;
    }
    finalize(state, compiled)
}

struct VmState<'a> {
    pc: u32,
    data_stack: SmallVec<[TypedValue; 16]>,
    matched_rules: Vec<RuleId>,
    output: Vec<Option<TypedValue>>,
    halted: bool,

    source: &'a str,
    rule_map: &'a [RuleMapEntry],
    hit_policy: HitPolicy,

    rule_traces: Vec<RuleTrace>,
    current_rule_predicates: Vec<PredicateTrace>,
    current_rule_idx: Option<usize>, // index into rule_map
}

impl<'a> VmState<'a> {
    fn new(compiled: &'a CompiledDecision, _input: &TypedInputContext, source: &'a str) -> Self {
        Self {
            pc: 0,
            data_stack: SmallVec::new(),
            matched_rules: Vec::new(),
            output: vec![None; compiled.output_schema.len()],
            halted: false,
            source,
            rule_map: &compiled.rule_map,
            hit_policy: compiled.hit_policy,
            rule_traces: Vec::new(),
            current_rule_predicates: Vec::new(),
            current_rule_idx: None,
        }
    }

    fn push(&mut self, v: TypedValue) {
        self.data_stack.push(v);
    }

    fn pop(&mut self) -> Option<TypedValue> {
        self.data_stack.pop()
    }

    fn peek(&self) -> Option<&TypedValue> {
        self.data_stack.last()
    }
}

fn step(
    state: &mut VmState,
    compiled: &CompiledDecision,
    input: &TypedInputContext,
) -> Result<(), EvalError> {
    let pc = state.pc as usize;
    let span = compiled.source_spans[pc];
    let instr = &compiled.instructions[pc];

    // Detect rule transitions before executing the instruction.
    detect_rule_transition(state, pc);

    match instr {
        Instr::LoadField(fid) => {
            let idx = fid.0;
            let value = input.get(FieldId(idx)).cloned().unwrap_or(TypedValue::Null);
            state.push(value);
            state.pc += 1;
        }
        Instr::PushConst(cid) => {
            let v = compiled.const_pool[cid.0 as usize].clone();
            state.push(v);
            state.pc += 1;
        }
        Instr::PushConstSet(sid) => {
            // Push a sentinel marker — InSet handles it directly from the pool.
            // We store the set_id as a special "Set" value using a hack-free approach:
            // Push a placeholder; InSet reads the actual set from compiled.const_set_pool.
            // Trick: store the index in a TypedValue::Integer for identification.
            state.push(TypedValue::Integer(sid.0 as i64));
            state.pc += 1;
        }
        Instr::Pop => {
            state.pop();
            state.pc += 1;
        }
        Instr::Dup => {
            if let Some(v) = state.peek().cloned() {
                state.push(v);
            }
            state.pc += 1;
        }
        Instr::Eq => {
            let b = state.pop().unwrap_or(TypedValue::Null);
            let a = state.pop().unwrap_or(TypedValue::Null);
            state.push(TypedValue::Bool(values_equal(&a, &b)));
            state.pc += 1;
        }
        Instr::NotEq => {
            let b = state.pop().unwrap_or(TypedValue::Null);
            let a = state.pop().unwrap_or(TypedValue::Null);
            // Two-valued null semantics (semantics.md §3.2): null on either side → false.
            // NotEq is NOT simply !Eq — both return false when null is involved.
            let result = if matches!(a, TypedValue::Null) || matches!(b, TypedValue::Null) {
                false
            } else {
                !values_equal(&a, &b)
            };
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Lt => {
            let (a, b) = pop2(state);
            let result = compare_numeric(&a, &b).is_some_and(|o| o == Ordering::Less);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Le => {
            let (a, b) = pop2(state);
            let result = compare_numeric(&a, &b).is_some_and(|o| o != Ordering::Greater);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Gt => {
            let (a, b) = pop2(state);
            let result = compare_numeric(&a, &b).is_some_and(|o| o == Ordering::Greater);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Ge => {
            let (a, b) = pop2(state);
            let result = compare_numeric(&a, &b).is_some_and(|o| o != Ordering::Less);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::InSet => {
            // Stack: [value, set_id_as_integer]
            let set_id_val = state.pop().unwrap_or(TypedValue::Null);
            let value = state.pop().unwrap_or(TypedValue::Null);
            let result = if let TypedValue::Integer(sid) = set_id_val {
                let set = &compiled.const_set_pool[sid as usize];
                set.iter().any(|v| values_equal(&value, v))
            } else {
                false
            };
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::RangeCheck(rid) => {
            let value = state.pop().unwrap_or(TypedValue::Null);
            let range = &compiled.range_pool[rid.0 as usize];
            let result = eval_range_check(&value, range);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::IsNull => {
            let v = state.pop().unwrap_or(TypedValue::Null);
            let is_null = matches!(v, TypedValue::Null);
            state.push(TypedValue::Bool(is_null));
            state.pc += 1;
        }
        Instr::IsNotNull => {
            let v = state.pop().unwrap_or(TypedValue::Null);
            let is_not_null = !matches!(v, TypedValue::Null);
            state.push(TypedValue::Bool(is_not_null));
            state.pc += 1;
        }
        Instr::And => {
            let (a, b) = pop2(state);
            let result = as_bool(&a) && as_bool(&b);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Or => {
            let (a, b) = pop2(state);
            let result = as_bool(&a) || as_bool(&b);
            state.push(TypedValue::Bool(result));
            state.pc += 1;
        }
        Instr::Not => {
            let v = state.pop().unwrap_or(TypedValue::Bool(false));
            state.push(TypedValue::Bool(!as_bool(&v)));
            state.pc += 1;
        }
        Instr::Br(addr) => {
            state.pc = *addr;
        }
        Instr::BrFalse(addr) => {
            let v = state.pop().unwrap_or(TypedValue::Bool(false));
            let is_false = !as_bool(&v);
            // Record predicate trace before branching.
            record_predicate_trace(state, as_bool(&v), span);
            if is_false {
                // Rule did not match on this predicate — finalise current rule as non-matched.
                finalise_non_matching_rule(state, compiled);
                state.pc = *addr;
            } else {
                state.pc += 1;
            }
        }
        Instr::BrTrue(addr) => {
            let v = state.pop().unwrap_or(TypedValue::Bool(false));
            if as_bool(&v) {
                state.pc = *addr;
            } else {
                state.pc += 1;
            }
        }
        Instr::RuleMatched(rid) => {
            state.matched_rules.push(*rid);
            // Finalise the current rule as matched.
            finalise_matching_rule(state, *rid, compiled);
            state.pc += 1;
        }
        Instr::StoreOutputTos(fid) => {
            let v = state.pop().unwrap_or(TypedValue::Null);
            state.output[fid.0 as usize] = Some(v);
            state.pc += 1;
        }
        Instr::StoreOutput(fid, cid) => {
            let v = compiled.const_pool[cid.0 as usize].clone();
            state.output[fid.0 as usize] = Some(v);
            state.pc += 1;
        }
        Instr::EndDecision => {
            state.halted = true;
        }
        // Reserved — verifier guarantees these never appear.
        _ => {
            state.pc += 1;
        }
    }
    Ok(())
}

// ── Trace helpers ─────────────────────────────────────────────────────────────

/// Check if we've transitioned to a new rule and set up state accordingly.
fn detect_rule_transition(state: &mut VmState, pc: usize) {
    for (idx, entry) in state.rule_map.iter().enumerate() {
        if entry.entry_addr as usize == pc {
            // Starting a new rule.
            state.current_rule_idx = Some(idx);
            state.current_rule_predicates.clear();
            break;
        }
    }
}

/// Record a predicate trace when a `BrFalse` executes.
fn record_predicate_trace(state: &mut VmState, result: bool, span: SourceSpan) {
    let description = state
        .source
        .get(span.start as usize..span.end as usize)
        .unwrap_or("")
        .to_owned();
    state.current_rule_predicates.push(PredicateTrace {
        result,
        source_span: span,
        description,
    });
}

/// Finalise the trace for a rule that did NOT match (BrFalse jumped away).
fn finalise_non_matching_rule(state: &mut VmState, compiled: &CompiledDecision) {
    let idx = match state.current_rule_idx {
        Some(i) => i,
        None => return,
    };
    let entry = &state.rule_map[idx];
    let predicates = std::mem::take(&mut state.current_rule_predicates);
    state.rule_traces.push(RuleTrace {
        rule_id: entry.rule_id,
        rule_name: entry.rule_name.clone(),
        matched: false,
        predicates,
        source_span: entry.source_span,
    });
    state.current_rule_idx = None;
    let _ = compiled;
}

/// Finalise the trace for a rule that DID match (`RuleMatched` executed).
fn finalise_matching_rule(state: &mut VmState, rule_id: RuleId, compiled: &CompiledDecision) {
    let idx = match state.current_rule_idx {
        Some(i) => i,
        None => return,
    };
    // For a catch-all rule, add a single always-true predicate trace.
    let entry = &state.rule_map[idx];
    let mut predicates = std::mem::take(&mut state.current_rule_predicates);
    if predicates.is_empty() {
        // Catch-all: manufacture a single true predicate trace.
        predicates.push(PredicateTrace {
            result: true,
            source_span: entry.source_span,
            description: "catch-all".into(),
        });
    }
    state.rule_traces.push(RuleTrace {
        rule_id,
        rule_name: entry.rule_name.clone(),
        matched: true,
        predicates,
        source_span: entry.source_span,
    });
    state.current_rule_idx = None;
    let _ = compiled;
}

// ── Finalisation ──────────────────────────────────────────────────────────────

fn finalize(state: VmState, compiled: &CompiledDecision) -> Result<EvaluationOutput, EvalError> {
    // Build rule traces: merge what the VM recorded with any rules that had
    // no trace (e.g., skipped entirely under FIRST after a match). Pad
    // trace.rules to match compiled.rule_map length.
    let mut traces = state.rule_traces;
    let recorded_ids: std::collections::BTreeSet<usize> =
        traces.iter().map(|t| t.rule_id.0).collect();
    for entry in state.rule_map {
        if !recorded_ids.contains(&entry.rule_id.0) {
            // Rule was never entered (FIRST short-circuit). Add a stub trace.
            traces.push(RuleTrace {
                rule_id: entry.rule_id,
                rule_name: entry.rule_name.clone(),
                matched: false,
                predicates: Vec::new(),
                source_span: entry.source_span,
            });
        }
    }
    // Sort by rule_id (source order).
    traces.sort_by_key(|t| t.rule_id.0);

    let outcome = match state.hit_policy {
        HitPolicy::Unique => match state.matched_rules.len() {
            0 => return Err(EvalError::NoMatch),
            1 => TraceOutcome::Match {
                rule_id: state.matched_rules[0],
            },
            _ => {
                return Err(EvalError::MultipleMatches {
                    rules: state.matched_rules.clone(),
                });
            }
        },
        HitPolicy::First => match state.matched_rules.first() {
            None => return Err(EvalError::NoMatch),
            Some(&rule_id) => TraceOutcome::Match { rule_id },
        },
    };

    // Build output context from the output slots.
    let slots: Vec<TypedValue> = state
        .output
        .into_iter()
        .map(|s| s.unwrap_or(TypedValue::Null))
        .collect();
    let output = TypedOutputContext::from_slots(&compiled.output_schema, slots);

    let trace = EvaluationTrace {
        rules: traces,
        outcome,
    };
    Ok(EvaluationOutput { output, trace })
}

// ── Value helpers ─────────────────────────────────────────────────────────────

fn pop2(state: &mut VmState) -> (TypedValue, TypedValue) {
    let b = state.pop().unwrap_or(TypedValue::Null);
    let a = state.pop().unwrap_or(TypedValue::Null);
    (a, b)
}

fn values_equal(a: &TypedValue, b: &TypedValue) -> bool {
    match (a, b) {
        (TypedValue::Null, _) | (_, TypedValue::Null) => false,
        _ => a == b,
    }
}

fn compare_numeric(a: &TypedValue, b: &TypedValue) -> Option<Ordering> {
    match (a, b) {
        (TypedValue::Integer(x), TypedValue::Integer(y)) => x.partial_cmp(y),
        (TypedValue::Integer(x), TypedValue::Decimal(y)) => (*x as f64).partial_cmp(y),
        (TypedValue::Decimal(x), TypedValue::Integer(y)) => x.partial_cmp(&(*y as f64)),
        (TypedValue::Decimal(x), TypedValue::Decimal(y)) => x.partial_cmp(y),
        _ => None,
    }
}

fn as_bool(v: &TypedValue) -> bool {
    matches!(v, TypedValue::Bool(true))
}

fn eval_range_check(value: &TypedValue, range: &RangeEntry) -> bool {
    if matches!(value, TypedValue::Null) {
        return false;
    }
    // Check lower bound.
    if let Some(lb) = &range.lower {
        match compare_numeric(value, lb) {
            None => return false,
            Some(Ordering::Less) => return false,
            Some(Ordering::Equal) if !range.lower_inclusive => return false,
            _ => {}
        }
    }
    // Check upper bound.
    if let Some(ub) = &range.upper {
        match compare_numeric(value, ub) {
            None => return false,
            Some(Ordering::Greater) => return false,
            Some(Ordering::Equal) if !range.upper_inclusive => return false,
            _ => {}
        }
    }
    true
}
