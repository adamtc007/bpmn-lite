//! Bytecode emitter tests — Phase 1.4 §3.4.
//!
//! Verifies instruction sequences, pool deduplication, branch-target patching,
//! and hit-policy-specific emission patterns produced by `emit::emit`.

use dmn_lite_compiler::{compile, load_catalogue_from_str};
use dmn_lite_parser::parse;
use dmn_lite_types::{
    instr::Instr,
    ir::{HitPolicy, TypedValue},
};

// ── Catalogue helpers ─────────────────────────────────────────────────────────

/// Minimal catalogue with one non-enum domain `N` (integer fields).
/// A non-enum domain triggers `DomainOnNonEnum` warning but compiles fine.
const INT_CAT: &str = r#"
snapshot_id = "019c0a5d-0000-7000-8000-000000000099"
snapshot_version = "test"
created_at = "2026-01-01T00:00:00Z"

[[domain]]
name = "N"
domain_id = "019c0a5d-0000-7000-8000-000000000001"
description = "integers"
"#;

fn int_cat() -> dmn_lite_compiler::Catalogue {
    load_catalogue_from_str(INT_CAT).expect("int_cat must load")
}

fn compile_ok(src: &str) -> dmn_lite_types::CompiledDecision {
    let cat = int_cat();
    // DomainOnNonEnum is only a warning for the N domain; compile succeeds.
    compile(parse(src).expect("parse"), &cat, src).expect("compile")
}

// Helper: short source for a one-input one-output FIRST decision.
fn one_rule_first(predicate: &str, out_val: i64) -> String {
    format!(
        "(define-decision t :hit-policy first \
         :inputs  ((x :type integer :domain N)) \
         :outputs ((y :type integer :domain N)) \
         :rules   ((rule r001 :when ({predicate}) :then ((y = {out_val}))) \
                   (rule r999 :when (*) :then ((y = -1)))))"
    )
}

// ── §6.1 Comparison predicates ────────────────────────────────────────────────

/// Single `=` comparison on a FIRST decision: exact instruction sequence.
///
/// Expected (with const dedup: x=1 and y=1 share const[0]):
///  0: LoadField(0)       ; x
///  1: PushConst(0)       ; 1
///  2: Eq
///  3: BrFalse(→ r999)
///  4: PushConst(0)       ; 1 (output — deduped)
///  5: StoreOutputTos(0)  ; y
///  6: RuleMatched(0)
///  7: Br(→ EndDecision)
///  [r999: RuleMatched + Br + EndDecision…]
#[test]
fn single_eq_comparison_first_has_load_push_eq_brfalse() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = -1)))))"#;
    let cd = compile_ok(src);
    let instrs = &cd.instructions;
    // r001 starts at 0: LoadField, PushConst, Eq, BrFalse
    assert!(
        matches!(instrs[0], Instr::LoadField(f) if f.0 == 0),
        "LoadField(0)"
    );
    assert!(matches!(instrs[1], Instr::PushConst(_)), "PushConst");
    assert!(matches!(instrs[2], Instr::Eq), "Eq");
    assert!(matches!(instrs[3], Instr::BrFalse(_)), "BrFalse");
    // RuleMatched(0) followed by Br for FIRST
    let rm_pos = instrs
        .iter()
        .position(|i| matches!(i, Instr::RuleMatched(r) if r.0 == 0))
        .unwrap();
    assert!(
        matches!(instrs[rm_pos + 1], Instr::Br(_)),
        "Br after RuleMatched for FIRST"
    );
}

/// `<` comparison emits `Lt` instruction.
#[test]
fn comparison_lt_emits_lt() {
    let src = one_rule_first("(x < 10)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::Lt)));
}

/// `<=` emits `Le`.
#[test]
fn comparison_le_emits_le() {
    let src = one_rule_first("(x <= 10)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::Le)));
}

/// `>` emits `Gt`.
#[test]
fn comparison_gt_emits_gt() {
    let src = one_rule_first("(x > 10)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::Gt)));
}

/// `>=` emits `Ge`.
#[test]
fn comparison_ge_emits_ge() {
    let src = one_rule_first("(x >= 10)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::Ge)));
}

/// `!=` emits `NotEq`.
#[test]
fn comparison_not_eq_emits_not_eq() {
    let src = one_rule_first("(x != 10)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::NotEq)));
}

// ── §6.2 InSet predicates ─────────────────────────────────────────────────────

/// `in (...)` emits LoadField + PushConstSet + InSet.
#[test]
fn in_set_emits_load_push_set_in_set() {
    let src = one_rule_first("(x in (1 2 3))", 1);
    let cd = compile_ok(&src);
    let instrs = &cd.instructions;
    let pos_lf = instrs
        .iter()
        .position(|i| matches!(i, Instr::LoadField(_)))
        .unwrap();
    let pos_pcs = instrs
        .iter()
        .position(|i| matches!(i, Instr::PushConstSet(_)))
        .unwrap();
    let pos_ins = instrs
        .iter()
        .position(|i| matches!(i, Instr::InSet))
        .unwrap();
    assert!(pos_lf < pos_pcs && pos_pcs < pos_ins);
    assert_eq!(cd.const_set_pool.len(), 1);
    assert_eq!(cd.const_set_pool[0].len(), 3);
}

// ── §6.3 Range predicates ─────────────────────────────────────────────────────

/// `in [lower .. upper]` emits LoadField + RangeCheck with correct bounds.
#[test]
fn range_emits_load_range_check_with_bounds() {
    let src = one_rule_first("(x in [1 .. 100])", 1);
    let cd = compile_ok(&src);
    let instrs = &cd.instructions;
    let pos_lf = instrs
        .iter()
        .position(|i| matches!(i, Instr::LoadField(_)))
        .unwrap();
    let pos_rc = instrs
        .iter()
        .position(|i| matches!(i, Instr::RangeCheck(_)))
        .unwrap();
    assert!(pos_lf < pos_rc);
    assert_eq!(cd.range_pool.len(), 1);
    let r = &cd.range_pool[0];
    assert_eq!(r.lower, Some(TypedValue::Integer(1)));
    assert_eq!(r.upper, Some(TypedValue::Integer(100)));
    assert!(r.lower_inclusive && r.upper_inclusive);
}

// ── §6.4 Null tests ───────────────────────────────────────────────────────────

#[test]
fn is_null_emits_is_null() {
    let src = one_rule_first("(x is-null)", 1);
    let cd = compile_ok(&src);
    assert!(cd.instructions.iter().any(|i| matches!(i, Instr::IsNull)));
}

#[test]
fn is_not_null_emits_is_not_null() {
    let src = one_rule_first("(x is-not-null)", 1);
    let cd = compile_ok(&src);
    assert!(
        cd.instructions
            .iter()
            .any(|i| matches!(i, Instr::IsNotNull))
    );
}

// ── §6.5 Not predicate ────────────────────────────────────────────────────────

#[test]
fn not_predicate_emits_inner_then_not() {
    let src = one_rule_first("(not (x = 5))", 1);
    let cd = compile_ok(&src);
    let instrs = &cd.instructions;
    let pos_eq = instrs.iter().position(|i| matches!(i, Instr::Eq)).unwrap();
    let pos_not = instrs.iter().position(|i| matches!(i, Instr::Not)).unwrap();
    assert!(pos_eq < pos_not);
}

// ── Catch-all rules ───────────────────────────────────────────────────────────

/// A catch-all rule emits no predicate instructions before RuleMatched.
#[test]
fn catch_all_has_no_predicate_instructions() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r999 :when (*) :then ((y = 99)))))"#;
    let cd = compile_ok(src);
    let instrs = &cd.instructions;
    assert!(!instrs.iter().any(|i| matches!(i, Instr::LoadField(_))));
    assert!(
        !instrs
            .iter()
            .any(|i| matches!(i, Instr::Eq | Instr::InSet | Instr::RangeCheck(_)))
    );
    assert!(instrs.iter().any(|i| matches!(i, Instr::RuleMatched(_))));
}

// ── Hit-policy emission shape ─────────────────────────────────────────────────

/// FIRST: every RuleMatched is followed by Br or EndDecision.
#[test]
fn first_policy_rule_matched_followed_by_br_or_end() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))))"#;
    let cd = compile_ok(src);
    let instrs = &cd.instructions;
    for (i, instr) in instrs.iter().enumerate() {
        if matches!(instr, Instr::RuleMatched(_)) {
            assert!(
                matches!(instrs[i + 1], Instr::Br(_) | Instr::EndDecision),
                "RuleMatched at {i} not followed by Br/EndDecision"
            );
        }
    }
}

/// UNIQUE: no Br immediately after RuleMatched.
#[test]
fn unique_policy_no_br_after_rule_matched() {
    let src = r#"(define-decision t :hit-policy unique
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))))"#;
    let cd = compile_ok(src);
    assert_eq!(cd.hit_policy, HitPolicy::Unique);
    let instrs = &cd.instructions;
    for (i, instr) in instrs.iter().enumerate() {
        if matches!(instr, Instr::RuleMatched(_)) {
            assert!(
                !matches!(instrs[i + 1], Instr::Br(_)),
                "UNIQUE has Br at {i}+1"
            );
        }
    }
}

// ── Pool deduplication ────────────────────────────────────────────────────────

/// Same constant value used multiple times is interned once.
#[test]
fn const_pool_deduplication() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N) (z :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 5) (z = 5)) :then ((y = 5)))))"#;
    let cd = compile_ok(src);
    assert_eq!(cd.const_pool.len(), 1, "5 should be interned once");
    assert_eq!(cd.const_pool[0], TypedValue::Integer(5));
}

/// Same range used in multiple rules is interned once.
#[test]
fn range_pool_deduplication() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x in [1 .. 10])) :then ((y = 1)))
                  (rule r002 :when ((x in [1 .. 10])) :then ((y = 2)))))"#;
    let cd = compile_ok(src);
    assert_eq!(cd.range_pool.len(), 1, "same range interned once");
}

// ── EndDecision ───────────────────────────────────────────────────────────────

/// EndDecision is always the last instruction.
#[test]
fn end_decision_is_last_instruction() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let cd = compile_ok(src);
    assert!(matches!(
        cd.instructions.last().unwrap(),
        Instr::EndDecision
    ));
}

// ── Rule map ──────────────────────────────────────────────────────────────────

/// Rule map entry count matches the number of rules.
#[test]
fn rule_map_count_matches_rule_count() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let cd = compile_ok(src);
    assert_eq!(cd.rule_map.len(), 3);
}

/// Rule map entry addresses are strictly increasing (source order).
#[test]
fn rule_map_addrs_strictly_increasing() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 1)))
                  (rule r002 :when ((x = 2)) :then ((y = 2)))
                  (rule r999 :when (*) :then ((y = 0)))))"#;
    let cd = compile_ok(src);
    let addrs: Vec<u32> = cd.rule_map.iter().map(|e| e.entry_addr).collect();
    assert!(addrs.windows(2).all(|w| w[0] < w[1]), "addrs: {:?}", addrs);
}

/// BrFalse for r001 points to r002's entry, and both Br(end) instructions
/// point to EndDecision.
#[test]
fn two_rule_first_brfalse_targets_next_rule() {
    let src = r#"(define-decision t :hit-policy first
        :inputs  ((x :type integer :domain N))
        :outputs ((y :type integer :domain N))
        :rules   ((rule r001 :when ((x = 1)) :then ((y = 10)))
                  (rule r002 :when ((x = 2)) :then ((y = 20)))))"#;
    let cd = compile_ok(src);
    let instrs = &cd.instructions;
    let r002_entry = cd.rule_map[1].entry_addr;
    // First BrFalse in the stream belongs to r001.
    let r001_brfalse_target = instrs
        .iter()
        .find_map(|i| {
            if let Instr::BrFalse(t) = i {
                Some(*t)
            } else {
                None
            }
        })
        .expect("r001 must have BrFalse");
    assert_eq!(r001_brfalse_target, r002_entry, "r001 BrFalse → r002");
    // All Br(end) must point to the EndDecision (last instruction).
    let end_addr = (instrs.len() - 1) as u32;
    let all_br_targets_are_end = instrs
        .iter()
        .filter_map(|i| if let Instr::Br(t) = i { Some(*t) } else { None })
        .all(|t| t == end_addr);
    assert!(all_br_targets_are_end, "all Br → EndDecision");
}
