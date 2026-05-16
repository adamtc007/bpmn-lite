//! Bytecode verifier: `CompiledDecision` → `VerifiedDecision`.
//!
//! Enforces the invariants from `docs/dmn-lite-bytecode.md` §7.  A compiled
//! decision that passes verification is safe to execute by the stack VM.
//!
//! The verifier runs once at compile time; the VM never re-verifies.

use std::collections::BTreeSet;

use thiserror::Error;

use dmn_lite_types::{
    compiled::{CompiledDecision, VerifiedDecision},
    instr::Instr,
    ir::HitPolicy,
};

// ── VerifierError ─────────────────────────────────────────────────────────────

/// Errors produced when verifying a `CompiledDecision`.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerifierError {
    /// `instructions.len() != source_spans.len()`.
    #[error(
        "instructions/source_spans length mismatch: instructions={instructions}, source_spans={source_spans}"
    )]
    InstructionSpanLengthMismatch {
        instructions: usize,
        source_spans: usize,
    },

    /// A branch target address is out of bounds.
    #[error("branch target {target} out of bounds (instructions.len = {len}) at instruction {at}")]
    BranchTargetOutOfBounds { at: u32, target: u32, len: usize },

    /// `LoadField` references a `FieldId` that exceeds the input schema arity.
    #[error("LoadField references unknown FieldId {field} at instruction {at}")]
    UnknownLoadFieldId { at: u32, field: u32 },

    /// `StoreOutput`/`StoreOutputTos` references an output field not in the schema.
    #[error("StoreOutput references unknown OutputFieldId {field} at instruction {at}")]
    UnknownStoreOutputId { at: u32, field: u32 },

    /// `PushConst` references a const pool index that doesn't exist.
    #[error("PushConst references unknown ConstId {id} at instruction {at}")]
    UnknownConstId { at: u32, id: u32 },

    /// `PushConstSet` references a set pool index that doesn't exist.
    #[error("PushConstSet references unknown ConstSetId {id} at instruction {at}")]
    UnknownConstSetId { at: u32, id: u32 },

    /// `RangeCheck` references a range pool index that doesn't exist.
    #[error("RangeCheck references unknown RangeId {id} at instruction {at}")]
    UnknownRangeId { at: u32, id: u32 },

    /// `RuleMatched` references a rule ID not in the rule map.
    #[error("RuleMatched references unknown RuleId at instruction {at}")]
    UnknownRuleId { at: u32 },

    /// No `EndDecision` instruction exists in the program.
    #[error("missing EndDecision instruction")]
    MissingEndDecision,

    /// More than one `EndDecision` instruction was found.
    #[error("multiple EndDecision instructions found")]
    MultipleEndDecisions,

    /// The `EndDecision` instruction is not reachable from the entry point.
    #[error("unreachable EndDecision (no path from entry reaches it)")]
    UnreachableEndDecision,

    /// A reachable non-terminal instruction has no valid successor.
    #[error("reachable instruction at {at} has no successor (not EndDecision and not branching)")]
    DanglingInstruction { at: u32 },

    /// A type mismatch was detected in the abstract interpretation.
    #[error("type mismatch at instruction {at}: expected {expected}, found {found}")]
    TypeMismatch {
        at: u32,
        expected: String,
        found: String,
    },

    /// An instruction requires more stack values than are available.
    #[error(
        "stack underflow at instruction {at}: expected {expected} operands, stack has {actual}"
    )]
    StackUnderflow {
        at: u32,
        expected: usize,
        actual: usize,
    },

    /// Two execution paths reaching the same instruction have different stack heights.
    #[error("stack height mismatch at branch join {at}: paths produce heights {heights:?}")]
    StackJoinMismatch { at: u32, heights: Vec<usize> },

    /// Stack is non-empty at `EndDecision`.
    #[error("non-zero stack height ({height}) at EndDecision (instruction {at})")]
    NonZeroStackAtEnd { at: u32, height: usize },

    /// The emitted hit-policy shape doesn't match the declared policy.
    #[error("hit policy {policy} requires {expected_shape}, found {actual_shape}")]
    HitPolicyShapeMismatch {
        policy: String,
        expected_shape: String,
        actual_shape: String,
    },

    /// A reserved (v0.2+) opcode was found in a v0.1 artifact.
    #[error("reserved opcode {opcode} not permitted in Profile v0.1 (instruction {at})")]
    ReservedOpcode { at: u32, opcode: String },
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Verify a compiled decision's bytecode invariants.
///
/// On success, returns a `VerifiedDecision` that the VM accepts.
/// On failure, returns the first invariant violation found.
pub fn verify(decision: CompiledDecision) -> Result<VerifiedDecision, VerifierError> {
    let instrs = &decision.instructions;
    let spans = &decision.source_spans;
    let n = instrs.len();

    // §5.1-a: parallel vector lengths
    if instrs.len() != spans.len() {
        return Err(VerifierError::InstructionSpanLengthMismatch {
            instructions: instrs.len(),
            source_spans: spans.len(),
        });
    }

    if n == 0 {
        return Err(VerifierError::MissingEndDecision);
    }

    // §5.4: no reserved opcodes
    for (i, instr) in instrs.iter().enumerate() {
        if instr.is_reserved() {
            return Err(VerifierError::ReservedOpcode {
                at: i as u32,
                opcode: instr.mnemonic().into(),
            });
        }
    }

    // §5.1-b: validate branch targets and pool references
    let rule_id_set: BTreeSet<usize> = decision.rule_map.iter().map(|r| r.rule_id.0).collect();
    for (i, instr) in instrs.iter().enumerate() {
        let at = i as u32;
        match instr {
            Instr::Br(t) | Instr::BrFalse(t) | Instr::BrTrue(t) if (*t as usize) >= n => {
                return Err(VerifierError::BranchTargetOutOfBounds {
                    at,
                    target: *t,
                    len: n,
                });
            }
            Instr::Br(_) | Instr::BrFalse(_) | Instr::BrTrue(_) => {}
            Instr::LoadField(f) if f.0 >= decision.input_schema.len() => {
                return Err(VerifierError::UnknownLoadFieldId {
                    at,
                    field: f.0 as u32,
                });
            }
            Instr::LoadField(_) => {}
            Instr::StoreOutputTos(f) if f.0 as usize >= decision.output_schema.len() => {
                return Err(VerifierError::UnknownStoreOutputId { at, field: f.0 });
            }
            Instr::StoreOutputTos(_) => {}
            Instr::StoreOutput(f, _) if f.0 as usize >= decision.output_schema.len() => {
                return Err(VerifierError::UnknownStoreOutputId { at, field: f.0 });
            }
            Instr::StoreOutput(_, c) if c.0 as usize >= decision.const_pool.len() => {
                return Err(VerifierError::UnknownConstId { at, id: c.0 });
            }
            Instr::StoreOutput(_, _) => {}
            Instr::PushConst(c) if c.0 as usize >= decision.const_pool.len() => {
                return Err(VerifierError::UnknownConstId { at, id: c.0 });
            }
            Instr::PushConst(_) => {}
            Instr::PushConstSet(s) if s.0 as usize >= decision.const_set_pool.len() => {
                return Err(VerifierError::UnknownConstSetId { at, id: s.0 });
            }
            Instr::PushConstSet(_) => {}
            Instr::RangeCheck(r) if r.0 as usize >= decision.range_pool.len() => {
                return Err(VerifierError::UnknownRangeId { at, id: r.0 });
            }
            Instr::RangeCheck(_) => {}
            Instr::RuleMatched(rid) if !rule_id_set.contains(&rid.0) => {
                return Err(VerifierError::UnknownRuleId { at });
            }
            _ => {}
        }
    }

    // §5.1-c: exactly one EndDecision
    let end_positions: Vec<usize> = instrs
        .iter()
        .enumerate()
        .filter(|(_, i)| matches!(i, Instr::EndDecision))
        .map(|(idx, _)| idx)
        .collect();
    match end_positions.len() {
        0 => return Err(VerifierError::MissingEndDecision),
        1 => {}
        _ => return Err(VerifierError::MultipleEndDecisions),
    }
    let end_pos = end_positions[0] as u32;

    // §5.1-d: reachability analysis — forward dataflow via BTreeSet
    let reachable = compute_reachable(instrs);
    if !reachable.contains(&end_pos) {
        return Err(VerifierError::UnreachableEndDecision);
    }

    // Check that every reachable non-EndDecision non-branching instruction has a
    // reachable successor.
    for &pos in &reachable {
        let idx = pos as usize;
        let instr = &instrs[idx];
        if matches!(instr, Instr::EndDecision) {
            continue;
        }
        if matches!(instr, Instr::Br(_)) {
            // Unconditional jump: successor is the target, not idx+1.
            continue;
        }
        let is_conditional_branch = matches!(instr, Instr::BrFalse(_) | Instr::BrTrue(_));
        if !is_conditional_branch && idx + 1 >= n {
            return Err(VerifierError::DanglingInstruction { at: pos });
        }
    }

    // §5.2: abstract stack-height tracking
    verify_stack_heights(instrs, &reachable, n)?;

    // §5.5: hit-policy emission shape (simplified check)
    verify_hit_policy_shape(&decision)?;

    Ok(VerifiedDecision::new_verified(decision))
}

// ── Reachability ──────────────────────────────────────────────────────────────

fn compute_reachable(instrs: &[Instr]) -> BTreeSet<u32> {
    let mut reachable = BTreeSet::new();
    let mut worklist = vec![0u32];
    while let Some(pc) = worklist.pop() {
        if reachable.contains(&pc) || pc as usize >= instrs.len() {
            continue;
        }
        reachable.insert(pc);
        let idx = pc as usize;
        match &instrs[idx] {
            Instr::EndDecision => {}
            Instr::Br(t) => {
                worklist.push(*t);
            }
            Instr::BrFalse(t) | Instr::BrTrue(t) => {
                worklist.push(*t);
                worklist.push(pc + 1);
            }
            _ => {
                worklist.push(pc + 1);
            }
        }
    }
    reachable
}

// ── Stack-height verification ─────────────────────────────────────────────────

fn verify_stack_heights(
    instrs: &[Instr],
    reachable: &BTreeSet<u32>,
    n: usize,
) -> Result<(), VerifierError> {
    // height[i] = expected stack height BEFORE executing instruction i.
    // We propagate forward; at branch joins, heights must agree.
    let mut heights: Vec<Option<usize>> = vec![None; n];
    heights[0] = Some(0);

    // Process in reachable order (BTreeSet gives deterministic order).
    for &pos in reachable {
        let idx = pos as usize;
        let h = match heights[idx] {
            Some(h) => h,
            None => continue,
        };
        let at = pos;
        let delta = stack_delta(&instrs[idx]);
        match delta {
            StackDelta::Fixed { pop, push } => {
                if h < pop {
                    return Err(VerifierError::StackUnderflow {
                        at,
                        expected: pop,
                        actual: h,
                    });
                }
                let new_h = h - pop + push;
                let next = idx + 1;
                if !matches!(&instrs[idx], Instr::Br(_) | Instr::EndDecision) && next < n {
                    set_height(&mut heights, next as u32, new_h, at)?;
                }
                if let Instr::Br(t) | Instr::BrFalse(t) | Instr::BrTrue(t) = &instrs[idx] {
                    let branch_h = if matches!(&instrs[idx], Instr::BrFalse(_) | Instr::BrTrue(_)) {
                        // BrFalse/BrTrue pops the bool (already counted in delta),
                        // so branch target sees h-1.
                        new_h
                    } else {
                        new_h
                    };
                    set_height(&mut heights, *t, branch_h, at)?;
                }
                if matches!(&instrs[idx], Instr::EndDecision) && new_h != 0 {
                    return Err(VerifierError::NonZeroStackAtEnd { at, height: new_h });
                }
            }
        }
    }
    Ok(())
}

fn set_height(
    heights: &mut [Option<usize>],
    pos: u32,
    h: usize,
    at: u32,
) -> Result<(), VerifierError> {
    let idx = pos as usize;
    if idx >= heights.len() {
        return Ok(());
    }
    match heights[idx] {
        None => {
            heights[idx] = Some(h);
            Ok(())
        }
        Some(existing) if existing == h => Ok(()),
        Some(existing) => Err(VerifierError::StackJoinMismatch {
            at,
            heights: vec![existing, h],
        }),
    }
}

enum StackDelta {
    Fixed { pop: usize, push: usize },
}

fn stack_delta(instr: &Instr) -> StackDelta {
    use StackDelta::Fixed;
    match instr {
        Instr::LoadField(_) => Fixed { pop: 0, push: 1 },
        Instr::PushConst(_) => Fixed { pop: 0, push: 1 },
        Instr::PushConstSet(_) => Fixed { pop: 0, push: 1 },
        Instr::Pop => Fixed { pop: 1, push: 0 },
        Instr::Dup => Fixed { pop: 0, push: 1 },
        Instr::Eq | Instr::NotEq | Instr::Lt | Instr::Le | Instr::Gt | Instr::Ge => {
            Fixed { pop: 2, push: 1 }
        }
        Instr::InSet => Fixed { pop: 2, push: 1 },
        Instr::RangeCheck(_) => Fixed { pop: 1, push: 1 },
        Instr::IsNull | Instr::IsNotNull => Fixed { pop: 1, push: 1 },
        Instr::And | Instr::Or => Fixed { pop: 2, push: 1 },
        Instr::Not => Fixed { pop: 1, push: 1 },
        Instr::Br(_) => Fixed { pop: 0, push: 0 },
        Instr::BrFalse(_) | Instr::BrTrue(_) => Fixed { pop: 1, push: 0 },
        Instr::RuleMatched(_) => Fixed { pop: 0, push: 0 },
        Instr::StoreOutputTos(_) => Fixed { pop: 1, push: 0 },
        Instr::StoreOutput(_, _) => Fixed { pop: 0, push: 0 },
        Instr::EndDecision => Fixed { pop: 0, push: 0 },
        // Reserved — should not reach here (rejected above).
        _ => Fixed { pop: 0, push: 0 },
    }
}

// ── Hit-policy shape verification ────────────────────────────────────────────

fn verify_hit_policy_shape(decision: &CompiledDecision) -> Result<(), VerifierError> {
    let instrs = &decision.instructions;
    match decision.hit_policy {
        HitPolicy::First => {
            // Every RuleMatched should be followed by Br(end) or EndDecision.
            for (i, instr) in instrs.iter().enumerate() {
                if matches!(instr, Instr::RuleMatched(_)) {
                    let next = i + 1;
                    let ok = next < instrs.len()
                        && matches!(instrs[next], Instr::Br(_) | Instr::EndDecision);
                    if !ok {
                        return Err(VerifierError::HitPolicyShapeMismatch {
                            policy: "FIRST".into(),
                            expected_shape: "RuleMatched followed by Br(end) or EndDecision".into(),
                            actual_shape: format!(
                                "instruction {} after RuleMatched at {}",
                                instrs.get(next).map(|i| i.mnemonic()).unwrap_or("none"),
                                i
                            ),
                        });
                    }
                }
            }
        }
        HitPolicy::Unique => {
            // Every RuleMatched should NOT be followed by Br (fall-through expected).
            for (i, instr) in instrs.iter().enumerate() {
                if matches!(instr, Instr::RuleMatched(_)) {
                    let next = i + 1;
                    let is_br = next < instrs.len() && matches!(instrs[next], Instr::Br(_));
                    if is_br {
                        return Err(VerifierError::HitPolicyShapeMismatch {
                            policy: "UNIQUE".into(),
                            expected_shape: "RuleMatched followed by fall-through (no Br)".into(),
                            actual_shape: format!("Br after RuleMatched at {i}"),
                        });
                    }
                }
            }
        }
    }
    Ok(())
}
