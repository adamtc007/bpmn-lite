//! Bytecode verifier tests — Phase 1.4 §3.6.
//!
//! One test per `VerifierError` variant, plus valid-decision sanity checks.
//! Tests construct minimal `CompiledDecision` objects manually so that each
//! error can be injected cleanly without interference from other constraints.

use std::time::SystemTime;

use dmn_lite_compiler::{
    CompiledDecision, compile_and_verify, load_catalogue_from_str,
    verify::{VerifierError, verify},
};
use dmn_lite_parser::parse;
use dmn_lite_types::{
    ArtifactHash, BkmId, CompileContext, ConstId, ConstSetId, DecisionId, FieldId, Instr,
    OutputFieldId, RangeId, RuleId, RuleMapEntry, SnapshotId, SourceSpan,
    ir::{FieldSchema, HitPolicy, ResolvedType, TypedDecision, TypedValue},
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn zero_span() -> SourceSpan {
    SourceSpan { start: 0, end: 0 }
}

fn int_field(name: &str, idx: usize) -> FieldSchema {
    FieldSchema {
        field_id: FieldId(idx),
        name: name.into(),
        field_type: ResolvedType::Integer,
        source_span: zero_span(),
    }
}

fn stub_typed_ir() -> TypedDecision {
    TypedDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        rules: vec![],
        resolved_entities: vec![],
        source_span: zero_span(),
    }
}

fn stub_compile_context() -> CompileContext {
    CompileContext {
        sem_os_snapshot_id: SnapshotId(uuid::Uuid::nil()),
        compiled_at: SystemTime::now(),
        compiler_version: "test".into(),
    }
}

/// Build a minimal valid FIRST compiled decision:
/// ```text
///  0: LoadField(0)
///  1: PushConst(0)        ; 1
///  2: Eq
///  3: BrFalse(8)          ; → EndDecision
///  4: PushConst(0)        ; 1 (output — deduped)
///  5: StoreOutputTos(0)
///  6: RuleMatched(0)
///  7: Br(8)               ; → EndDecision
///  8: EndDecision
/// ```
fn minimal_valid() -> CompiledDecision {
    let instrs = vec![
        Instr::LoadField(FieldId(0)),
        Instr::PushConst(ConstId(0)),
        Instr::Eq,
        Instr::BrFalse(8),
        Instr::PushConst(ConstId(0)),
        Instr::StoreOutputTos(OutputFieldId(0)),
        Instr::RuleMatched(RuleId(0)),
        Instr::Br(8),
        Instr::EndDecision,
    ];
    let n = instrs.len();
    CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![TypedValue::Integer(1)],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![RuleMapEntry {
            rule_id: RuleId(0),
            rule_name: "r001".into(),
            entry_addr: 0,
            source_span: zero_span(),
        }],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    }
}

const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"
[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

// ── Valid decisions ───────────────────────────────────────────────────────────

/// A freshly emitted decision passes verification.
#[test]
fn valid_compiled_decision_passes_verify() {
    let cd = minimal_valid();
    assert!(verify(cd).is_ok());
}

/// A decision compiled from real source passes verification.
#[test]
fn real_compiled_decision_passes_verify() {
    let cat = load_catalogue_from_str(INT_CAT).unwrap();
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 42)) :then ((y = 42)))))"#;
    let result = compile_and_verify(parse(src).unwrap(), &cat, src);
    assert!(result.is_ok(), "expected Ok, got {:?}", result.err());
}

// ── VerifierError::InstructionSpanLengthMismatch ──────────────────────────────

#[test]
fn span_length_mismatch() {
    let mut cd = minimal_valid();
    cd.source_spans.pop(); // one fewer span than instructions
    assert!(matches!(
        verify(cd),
        Err(VerifierError::InstructionSpanLengthMismatch { .. })
    ));
}

// ── VerifierError::BranchTargetOutOfBounds ────────────────────────────────────

#[test]
fn branch_target_out_of_bounds() {
    let mut cd = minimal_valid();
    // Patch BrFalse at index 3 to point beyond the instruction stream.
    if let Instr::BrFalse(ref mut t) = cd.instructions[3] {
        *t = 9999;
    }
    assert!(matches!(
        verify(cd),
        Err(VerifierError::BranchTargetOutOfBounds { target: 9999, .. })
    ));
}

// ── VerifierError::UnknownLoadFieldId ─────────────────────────────────────────

#[test]
fn unknown_load_field_id() {
    let mut cd = minimal_valid();
    // Replace LoadField(0) with LoadField(100) — field 100 doesn't exist.
    cd.instructions[0] = Instr::LoadField(FieldId(100));
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownLoadFieldId { field: 100, .. })
    ));
}

// ── VerifierError::UnknownStoreOutputId ──────────────────────────────────────

#[test]
fn unknown_store_output_id() {
    let mut cd = minimal_valid();
    // Replace StoreOutputTos(0) with StoreOutputTos(100).
    cd.instructions[5] = Instr::StoreOutputTos(OutputFieldId(100));
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownStoreOutputId { field: 100, .. })
    ));
}

// ── VerifierError::UnknownConstId ─────────────────────────────────────────────

#[test]
fn unknown_const_id() {
    let mut cd = minimal_valid();
    // Replace PushConst(0) with PushConst(99) — const 99 doesn't exist.
    cd.instructions[1] = Instr::PushConst(ConstId(99));
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownConstId { id: 99, .. })
    ));
}

// ── VerifierError::UnknownConstSetId ─────────────────────────────────────────

#[test]
fn unknown_const_set_id() {
    let mut cd = minimal_valid();
    // Inject PushConstSet(0) — but const_set_pool is empty.
    cd.instructions
        .insert(1, Instr::PushConstSet(ConstSetId(0)));
    cd.source_spans.insert(1, zero_span());
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownConstSetId { id: 0, .. })
    ));
}

// ── VerifierError::UnknownRangeId ─────────────────────────────────────────────

#[test]
fn unknown_range_id() {
    let mut cd = minimal_valid();
    // Inject RangeCheck(0) — but range_pool is empty.
    cd.instructions.insert(1, Instr::RangeCheck(RangeId(0)));
    cd.source_spans.insert(1, zero_span());
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownRangeId { id: 0, .. })
    ));
}

// ── VerifierError::UnknownRuleId ─────────────────────────────────────────────

#[test]
fn unknown_rule_id() {
    let mut cd = minimal_valid();
    // Replace RuleMatched(0) with RuleMatched(99) — not in rule_map.
    cd.instructions[6] = Instr::RuleMatched(RuleId(99));
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnknownRuleId { .. })
    ));
}

// ── VerifierError::MissingEndDecision ─────────────────────────────────────────

#[test]
fn missing_end_decision_empty_program() {
    let mut cd = minimal_valid();
    cd.instructions.clear();
    cd.source_spans.clear();
    assert!(matches!(verify(cd), Err(VerifierError::MissingEndDecision)));
}

#[test]
fn missing_end_decision_replaced_with_nop() {
    let mut cd = minimal_valid();
    // Replace the EndDecision with a Pop — no EndDecision remains.
    let last = cd.instructions.len() - 1;
    cd.instructions[last] = Instr::Pop;
    assert!(matches!(verify(cd), Err(VerifierError::MissingEndDecision)));
}

// ── VerifierError::MultipleEndDecisions ──────────────────────────────────────

#[test]
fn multiple_end_decisions() {
    let mut cd = minimal_valid();
    // Append a second EndDecision + its span.
    cd.instructions.push(Instr::EndDecision);
    cd.source_spans.push(zero_span());
    assert!(matches!(
        verify(cd),
        Err(VerifierError::MultipleEndDecisions)
    ));
}

// ── VerifierError::UnreachableEndDecision ────────────────────────────────────

#[test]
fn unreachable_end_decision() {
    // Instruction 0 loops forever; EndDecision at 1 is unreachable.
    // No pool references, no rule_map (no RuleMatched).
    let instrs = vec![Instr::Br(0), Instr::EndDecision];
    let n = instrs.len();
    let cd = CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    };
    assert!(matches!(
        verify(cd),
        Err(VerifierError::UnreachableEndDecision)
    ));
}

// ── VerifierError::DanglingInstruction ───────────────────────────────────────

#[test]
fn dangling_instruction() {
    // BrFalse(4): fall-through→3=EndDecision, branch→4=LoadField.
    // Position 4 is reachable via branch, not EndDecision, not Br,
    // and idx+1=5 >= n=5 → DanglingInstruction.
    let instrs = vec![
        Instr::LoadField(FieldId(0)), // 0: h=0→1
        Instr::IsNotNull,             // 1: h=1→1 (bool)
        Instr::BrFalse(4),            // 2: pop bool (h=1→0); fall→3 (h=0), branch→4 (h=0)
        Instr::EndDecision,           // 3: h=0 ✓ reachable via fall-through
        Instr::LoadField(FieldId(0)), // 4: reachable via branch; idx=4=n-1 → DanglingInstruction
    ];
    let n = instrs.len();
    let cd = CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    };
    assert!(matches!(
        verify(cd),
        Err(VerifierError::DanglingInstruction { .. })
    ));
}

// ── VerifierError::StackUnderflow ─────────────────────────────────────────────

#[test]
fn stack_underflow() {
    // Eq at position 0 needs 2 operands but stack is empty.
    let instrs = vec![Instr::Eq, Instr::EndDecision];
    let n = instrs.len();
    let cd = CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    };
    assert!(matches!(
        verify(cd),
        Err(VerifierError::StackUnderflow { .. })
    ));
}

// ── VerifierError::StackJoinMismatch ─────────────────────────────────────────

#[test]
fn stack_join_mismatch() {
    // Two paths reach instruction 5 with different stack heights (2 vs 1).
    // Path A: 0→1→2→3→4→5  (h=2 at 5)
    // Path B: 0→1 (branch to 4) →4→5  (h=0 at 4, then h=1 after PushConst at 4)
    // Wait, let me trace:
    // 0: PushConst(0)     h=0→1
    // 1: BrFalse(4)       h=1→0; fall→2 (h=0), branch→4 (h=0)
    // 2: PushConst(0)     h=0→1
    // 3: PushConst(0)     h=1→2
    //    Br(5)            h=2 → 5 (h=2)
    // 4: (nothing—empty)
    //    Br(5)            h=0 → 5 (h=0) — conflict with h=2 at 5 → StackJoinMismatch
    // 5: EndDecision
    let instrs = vec![
        Instr::PushConst(ConstId(0)), // 0
        Instr::BrFalse(4),            // 1: fall→2, branch→4
        Instr::PushConst(ConstId(0)), // 2
        Instr::Br(5),                 // 3: jump to 5 with h=2
        Instr::Br(5),                 // 4: jump to 5 with h=0
        Instr::EndDecision,           // 5: receives h=2 AND h=0 → mismatch
    ];
    let n = instrs.len();
    let cd = CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![TypedValue::Integer(1)],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    };
    assert!(matches!(
        verify(cd),
        Err(VerifierError::StackJoinMismatch { .. })
    ));
}

// ── VerifierError::NonZeroStackAtEnd ─────────────────────────────────────────

#[test]
fn non_zero_stack_at_end() {
    // PushConst pushes one value, then EndDecision with h=1 → NonZeroStackAtEnd.
    let instrs = vec![Instr::PushConst(ConstId(0)), Instr::EndDecision];
    let n = instrs.len();
    let cd = CompiledDecision {
        decision_id: DecisionId("t".into()),
        name: "t".into(),
        hit_policy: HitPolicy::First,
        input_schema: vec![int_field("x", 0)],
        output_schema: vec![int_field("y", 0)],
        const_pool: vec![TypedValue::Integer(1)],
        const_set_pool: vec![],
        range_pool: vec![],
        instructions: instrs,
        source_spans: vec![zero_span(); n],
        rule_map: vec![],
        artifact_hash: ArtifactHash::ZERO,
        compile_context: stub_compile_context(),
        typed_ir: stub_typed_ir(),
    };
    assert!(matches!(
        verify(cd),
        Err(VerifierError::NonZeroStackAtEnd { height: 1, .. })
    ));
}

// ── VerifierError::HitPolicyShapeMismatch ────────────────────────────────────

/// FIRST hit policy: RuleMatched not followed by Br or EndDecision.
#[test]
fn first_hit_policy_rule_matched_not_followed_by_br() {
    let mut cd = minimal_valid();
    // Replace Br(8) at index 7 with another RuleMatched(0) so that RuleMatched at 6
    // is followed by RuleMatched(0) — not Br or EndDecision.
    cd.instructions[7] = Instr::RuleMatched(RuleId(0));
    assert!(matches!(
        verify(cd),
        Err(VerifierError::HitPolicyShapeMismatch { .. })
    ));
}

/// UNIQUE hit policy: Br found immediately after RuleMatched.
#[test]
fn unique_hit_policy_br_after_rule_matched() {
    let mut cd = minimal_valid();
    // The minimal_valid is FIRST (has Br after RuleMatched). Change to UNIQUE → shape mismatch.
    cd.hit_policy = HitPolicy::Unique;
    assert!(matches!(
        verify(cd),
        Err(VerifierError::HitPolicyShapeMismatch { .. })
    ));
}

// ── VerifierError::ReservedOpcode ─────────────────────────────────────────────

#[test]
fn reserved_opcode_rejected() {
    let mut cd = minimal_valid();
    // Insert a reserved Call instruction before EndDecision.
    let end = cd.instructions.len() - 1;
    cd.instructions.insert(end, Instr::Call(BkmId(0)));
    cd.source_spans.insert(end, zero_span());
    assert!(matches!(
        verify(cd),
        Err(VerifierError::ReservedOpcode { .. })
    ));
}
