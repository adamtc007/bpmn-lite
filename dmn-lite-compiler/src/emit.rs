//! Bytecode emitter: `TypedDecision` → `CompiledDecision`.
//!
//! Implements the canonical lowering patterns from `docs/dmn-lite-bytecode.md`
//! §4–6 verbatim.  The emitter is *total* (no errors); all type and semantic
//! errors are caught by the Phase 1.2 compiler before emission.
//!
//! Reading this file alongside §6 of the bytecode spec should feel like
//! reading the same document twice.

use std::sync::Arc;
use std::time::SystemTime;

use dmn_lite_types::{
    SourceSpan,
    compiled::{CompileContext, CompiledDecision, RangeEntry, RuleMapEntry},
    ids::{ConstId, ConstSetId, FieldId, OutputFieldId, RangeId},
    instr::Instr,
    ir::{
        ComparisonOp, HitPolicy, TypedAssignment, TypedDecision, TypedPredicate, TypedValue,
        TypedWhen,
    },
};

use crate::hash::compute_artifact_hash;

// ── EmitContext ───────────────────────────────────────────────────────────────

/// Accumulates bytecode and pool state during a single decision emission.
struct EmitContext {
    instructions: Vec<Instr>,
    source_spans: Vec<SourceSpan>,

    // Pools: Vec holds the actual entries; BTreeMap maps canonical key → index
    // for deduplication. BTreeMap (not HashMap) for deterministic ordering.
    const_pool: Vec<TypedValue>,
    const_pool_keys: std::collections::BTreeMap<Vec<u8>, u32>,

    const_set_pool: Vec<Vec<TypedValue>>,
    const_set_pool_keys: std::collections::BTreeMap<Vec<Vec<u8>>, u32>,

    range_pool: Vec<RangeEntry>,
    range_pool_keys: std::collections::BTreeMap<Vec<u8>, u32>,

    rule_map: Vec<RuleMapEntry>,

    // Pending branch-target patches: instruction indices of Br(MAX) to patch.
    patches_end: Vec<usize>, // Br(placeholder) that must jump to EndDecision
}

impl EmitContext {
    fn new() -> Self {
        Self {
            instructions: Vec::new(),
            source_spans: Vec::new(),
            const_pool: Vec::new(),
            const_pool_keys: std::collections::BTreeMap::new(),
            const_set_pool: Vec::new(),
            const_set_pool_keys: std::collections::BTreeMap::new(),
            range_pool: Vec::new(),
            range_pool_keys: std::collections::BTreeMap::new(),
            rule_map: Vec::new(),
            patches_end: Vec::new(),
        }
    }

    fn emit(&mut self, instr: Instr, span: SourceSpan) {
        self.instructions.push(instr);
        self.source_spans.push(span);
    }

    fn pc(&self) -> u32 {
        self.instructions.len() as u32
    }

    // ── Pool internment ───────────────────────────────────────────────────────

    fn intern_const(&mut self, value: TypedValue) -> ConstId {
        let key = serialize_typed_value(&value);
        if let Some(&idx) = self.const_pool_keys.get(&key) {
            return ConstId(idx);
        }
        let idx = self.const_pool.len() as u32;
        self.const_pool_keys.insert(key, idx);
        self.const_pool.push(value);
        ConstId(idx)
    }

    fn intern_const_set(&mut self, mut values: Vec<TypedValue>) -> ConstSetId {
        // Canonically sort set members for deterministic deduplication.
        values.sort_by_key(serialize_typed_value);
        values.dedup_by(|a, b| serialize_typed_value(a) == serialize_typed_value(b));
        let keys: Vec<Vec<u8>> = values.iter().map(serialize_typed_value).collect();
        if let Some(&idx) = self.const_set_pool_keys.get(&keys) {
            return ConstSetId(idx);
        }
        let idx = self.const_set_pool.len() as u32;
        self.const_set_pool_keys.insert(keys, idx);
        self.const_set_pool.push(values);
        ConstSetId(idx)
    }

    fn intern_range(
        &mut self,
        lower: Option<TypedValue>,
        upper: Option<TypedValue>,
        lower_inclusive: bool,
        upper_inclusive: bool,
    ) -> RangeId {
        let mut key = Vec::new();
        key.push(lower_inclusive as u8);
        key.push(upper_inclusive as u8);
        match &lower {
            None => key.push(0),
            Some(v) => {
                key.push(1);
                key.extend(serialize_typed_value(v));
            }
        }
        match &upper {
            None => key.push(0),
            Some(v) => {
                key.push(1);
                key.extend(serialize_typed_value(v));
            }
        }
        if let Some(&idx) = self.range_pool_keys.get(&key) {
            return RangeId(idx);
        }
        let idx = self.range_pool.len() as u32;
        self.range_pool_keys.insert(key, idx);
        self.range_pool.push(RangeEntry {
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
        });
        RangeId(idx)
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Emit bytecode from a `TypedDecision`, producing a `CompiledDecision`.
///
/// This function is total: all type and semantic errors must have been caught
/// by the Phase 1.2 compiler before calling `emit`.
pub fn emit(typed: TypedDecision, source_text: &str) -> CompiledDecision {
    let mut ctx = EmitContext::new();

    let n_rules = typed.rules.len();

    // Emit each rule, collecting patch indices as we go.
    for rule in typed.rules.iter() {
        let entry_addr = ctx.pc();
        ctx.rule_map.push(RuleMapEntry {
            rule_id: rule.rule_id,
            rule_name: rule.rule_name.clone(),
            entry_addr,
            source_span: rule.source_span,
        });

        let mut rule_brfalse_patches: Vec<usize> = Vec::new();

        // Emit :when clause
        match &rule.when {
            TypedWhen::CatchAll(_) => {
                // No predicate instructions for catch-all rules.
            }
            TypedWhen::Predicates(preds, _) => {
                for pred in preds {
                    // Emit the predicate (leaves bool on stack), then BrFalse.
                    emit_predicate_nested(&mut ctx, pred, source_text);
                    // BrFalse with placeholder — will be patched to next_rule_start.
                    let br_idx = ctx.instructions.len();
                    ctx.emit(Instr::BrFalse(u32::MAX), pred_span(pred));
                    rule_brfalse_patches.push(br_idx);
                }
            }
        }

        // Emit :then assignments
        emit_assignments(&mut ctx, &rule.then, source_text);

        // RuleMatched
        ctx.emit(Instr::RuleMatched(rule.rule_id), rule.source_span);

        // Under FIRST: Br(end) — with placeholder.
        if typed.hit_policy == HitPolicy::First {
            let br_idx = ctx.instructions.len();
            ctx.emit(Instr::Br(u32::MAX), rule.source_span);
            ctx.patches_end.push(br_idx);
        }

        // Patch all BrFalse instructions in this rule to next_rule_start.
        // next_rule_start = current PC (immediately after this rule's body).
        let next_rule_start = ctx.pc();
        for idx in rule_brfalse_patches {
            if let Instr::BrFalse(ref mut addr) = ctx.instructions[idx] {
                *addr = next_rule_start;
            }
        }
    }

    // EndDecision
    let end_addr = ctx.pc();
    let decision_span = typed.source_span;
    ctx.emit(Instr::EndDecision, decision_span);

    // Patch all Br(end) placeholders.
    for idx in &ctx.patches_end {
        if let Instr::Br(ref mut addr) = ctx.instructions[*idx] {
            *addr = end_addr;
        }
    }

    // Const-set pool: convert Vec<TypedValue> → Arc<[TypedValue]>
    let const_set_pool: Vec<Arc<[TypedValue]>> =
        ctx.const_set_pool.into_iter().map(|v| v.into()).collect();

    // Compute artifact hash.
    let artifact_hash = compute_artifact_hash(
        source_text,
        &typed,
        &ctx.instructions,
        &ctx.const_pool,
        &ctx.range_pool,
    );

    let compile_context = CompileContext {
        sem_os_snapshot_id: typed
            .resolved_entities
            .first()
            .map(|_| dmn_lite_types::SnapshotId(uuid::Uuid::nil()))
            .unwrap_or(dmn_lite_types::SnapshotId(uuid::Uuid::nil())),
        compiled_at: SystemTime::now(),
        compiler_version: env!("CARGO_PKG_VERSION").to_owned(),
    };

    let _ = n_rules;

    CompiledDecision {
        decision_id: typed.decision_id.clone(),
        name: typed.name.clone(),
        hit_policy: typed.hit_policy,
        input_schema: typed.input_schema.clone(),
        output_schema: typed.output_schema.clone(),
        const_pool: ctx.const_pool,
        const_set_pool,
        range_pool: ctx.range_pool,
        instructions: ctx.instructions,
        source_spans: ctx.source_spans,
        rule_map: ctx.rule_map,
        artifact_hash,
        compile_context,
        typed_ir: typed,
    }
}

// ── Assignment emission ───────────────────────────────────────────────────────

fn emit_assignments(ctx: &mut EmitContext, assignments: &[TypedAssignment], _source: &str) {
    for a in assignments {
        let const_id = ctx.intern_const(a.value.clone());
        ctx.emit(Instr::PushConst(const_id), a.source_span);
        ctx.emit(
            Instr::StoreOutputTos(OutputFieldId(a.output_field.0 as u32)),
            a.source_span,
        );
    }
}

// ── Predicate emission (nested — result left on stack) ────────────────────────

/// Emit a predicate's instructions, leaving a boolean result on the stack.
/// Does NOT emit a trailing `BrFalse`; the caller adds that when emitting at
/// rule-:when level.
fn emit_predicate_nested(ctx: &mut EmitContext, pred: &TypedPredicate, _source: &str) {
    match pred {
        TypedPredicate::Comparison {
            field,
            op,
            rhs,
            source_span,
        } => {
            emit_comparison(ctx, *field, *op, rhs, *source_span);
        }
        TypedPredicate::InSet {
            field,
            values,
            source_span,
        } => {
            emit_in_set(ctx, *field, values, *source_span);
        }
        TypedPredicate::Range {
            field,
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            source_span,
        } => {
            emit_range(
                ctx,
                *field,
                lower,
                upper,
                *lower_inclusive,
                *upper_inclusive,
                *source_span,
            );
        }
        TypedPredicate::IsNull { field, source_span } => {
            emit_null_test(ctx, *field, true, *source_span);
        }
        TypedPredicate::IsNotNull { field, source_span } => {
            emit_null_test(ctx, *field, false, *source_span);
        }
        TypedPredicate::Not { inner, source_span } => {
            emit_not(ctx, inner, *source_span, _source);
        }
        TypedPredicate::And { items, source_span } => {
            emit_and(ctx, items, *source_span, _source);
        }
        TypedPredicate::Or { items, source_span } => {
            emit_or(ctx, items, *source_span, _source);
        }
    }
}

// ── Comparison (§6.1) ─────────────────────────────────────────────────────────

fn emit_comparison(
    ctx: &mut EmitContext,
    field: FieldId,
    op: ComparisonOp,
    rhs: &TypedValue,
    span: SourceSpan,
) {
    ctx.emit(Instr::LoadField(field), span);
    let cid = ctx.intern_const(rhs.clone());
    ctx.emit(Instr::PushConst(cid), span);
    let instr = match op {
        ComparisonOp::Eq => Instr::Eq,
        ComparisonOp::NotEq => Instr::NotEq,
        ComparisonOp::Lt => Instr::Lt,
        ComparisonOp::Le => Instr::Le,
        ComparisonOp::Gt => Instr::Gt,
        ComparisonOp::Ge => Instr::Ge,
    };
    ctx.emit(instr, span);
}

// ── InSet (§6.2) ──────────────────────────────────────────────────────────────

fn emit_in_set(ctx: &mut EmitContext, field: FieldId, values: &[TypedValue], span: SourceSpan) {
    ctx.emit(Instr::LoadField(field), span);
    let set_id = ctx.intern_const_set(values.to_vec());
    ctx.emit(Instr::PushConstSet(set_id), span);
    ctx.emit(Instr::InSet, span);
}

// ── Range (§6.3) ──────────────────────────────────────────────────────────────

fn emit_range(
    ctx: &mut EmitContext,
    field: FieldId,
    lower: &Option<TypedValue>,
    upper: &Option<TypedValue>,
    lower_inclusive: bool,
    upper_inclusive: bool,
    span: SourceSpan,
) {
    ctx.emit(Instr::LoadField(field), span);
    let rid = ctx.intern_range(
        lower.clone(),
        upper.clone(),
        lower_inclusive,
        upper_inclusive,
    );
    ctx.emit(Instr::RangeCheck(rid), span);
}

// ── Null tests (§6.4) ─────────────────────────────────────────────────────────

fn emit_null_test(ctx: &mut EmitContext, field: FieldId, is_null: bool, span: SourceSpan) {
    ctx.emit(Instr::LoadField(field), span);
    if is_null {
        ctx.emit(Instr::IsNull, span);
    } else {
        ctx.emit(Instr::IsNotNull, span);
    }
}

// ── Not (§6.5) ────────────────────────────────────────────────────────────────

fn emit_not(ctx: &mut EmitContext, inner: &TypedPredicate, span: SourceSpan, source: &str) {
    emit_predicate_nested(ctx, inner, source);
    ctx.emit(Instr::Not, span);
}

// ── And (§6.6) ────────────────────────────────────────────────────────────────

fn emit_and(ctx: &mut EmitContext, items: &[TypedPredicate], span: SourceSpan, source: &str) {
    // Evaluate all sub-predicates; fold with And (no short-circuit within and).
    assert!(items.len() >= 2, "and requires at least 2 predicates");
    emit_predicate_nested(ctx, &items[0], source);
    emit_predicate_nested(ctx, &items[1], source);
    ctx.emit(Instr::And, span);
    for item in &items[2..] {
        emit_predicate_nested(ctx, item, source);
        ctx.emit(Instr::And, span);
    }
}

// ── Or (§6.7) ─────────────────────────────────────────────────────────────────

fn emit_or(ctx: &mut EmitContext, items: &[TypedPredicate], span: SourceSpan, source: &str) {
    assert!(items.len() >= 2, "or requires at least 2 predicates");
    emit_predicate_nested(ctx, &items[0], source);
    emit_predicate_nested(ctx, &items[1], source);
    ctx.emit(Instr::Or, span);
    for item in &items[2..] {
        emit_predicate_nested(ctx, item, source);
        ctx.emit(Instr::Or, span);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Source span for a predicate (used as the BrFalse span).
fn pred_span(pred: &TypedPredicate) -> SourceSpan {
    match pred {
        TypedPredicate::Comparison { source_span, .. }
        | TypedPredicate::InSet { source_span, .. }
        | TypedPredicate::Range { source_span, .. }
        | TypedPredicate::IsNull { source_span, .. }
        | TypedPredicate::IsNotNull { source_span, .. }
        | TypedPredicate::Not { source_span, .. }
        | TypedPredicate::And { source_span, .. }
        | TypedPredicate::Or { source_span, .. } => *source_span,
    }
}

// ── Canonical serialisation of TypedValue (for pool deduplication) ────────────

/// Deterministic byte serialisation of a `TypedValue` used as deduplication
/// keys in `BTreeMap`. Not a public format; only used inside the emitter.
pub(crate) fn serialize_typed_value(v: &TypedValue) -> Vec<u8> {
    use dmn_lite_types::ir::TypedValue as TV;
    let mut out = Vec::new();
    match v {
        TV::Null => out.push(0x00),
        TV::Bool(b) => {
            out.push(0x01);
            out.push(*b as u8);
        }
        TV::Integer(i) => {
            out.push(0x02);
            out.extend(i.to_le_bytes());
        }
        TV::Decimal(f) => {
            out.push(0x03);
            out.extend(f.to_bits().to_le_bytes());
        }
        TV::Str(s) => {
            out.push(0x04);
            out.extend((s.len() as u32).to_le_bytes());
            out.extend(s.as_bytes());
        }
        TV::Enum {
            domain_id,
            value_id,
        } => {
            out.push(0x05);
            out.extend(domain_id.0.as_bytes());
            out.extend(value_id.0.as_bytes());
        }
    }
    out
}
