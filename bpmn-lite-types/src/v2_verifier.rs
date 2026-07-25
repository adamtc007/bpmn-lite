//! Tranche V3 — Verifier: Artifact Theorems V-1..V-9 (EOP-BPMN-ISA-002).
//!
//! Dual-stack abstract interpretation over the v2 `Instr` CFG built by
//! `artifact::successors()`/`artifact::race_arm_targets()` (V2.7 7.3). This
//! module owns the CONTROL-stack half (V-1..V-5, V-8); the OPERAND-stack
//! half (V-6) is already discharged by `artifact::verify_program`'s
//! pre-existing height walk, which V2.7's `stack_effect()` extension
//! already covers for v2 words — this module does not duplicate it.
//!
//! **Two facts the V2.7 addressing-scheme review established that this
//! module must honour (recorded, not re-derived):**
//!
//! 1. `artifact::v2_control_stack_effect()` is NOT total for `V2Fork` — the
//!    pushed barrier handle lands on the *spawned fibres'* control stacks,
//!    not the forking fibre's own. This module does not use that function
//!    at all for the walk below; `V2Fork` gets its own explicit transition
//!    rule (see `step`) that constructs each child's entry state directly,
//!    exactly the children-inherit-parent-stack-plus-barrier-handle
//!    semantics the review flagged as the single most likely place V-1
//!    balance goes subtly wrong if assumed rather than coded.
//! 2. The race scope's `[ h -- ]` pop happens at `V2RaceClose`, and arm
//!    target edges hang off it (V2.7 remediation r.2,
//!    `race_handle_is_popped_in_abstract_entry_state_at_each_arm_target`).
//!    This module's `V2RaceClose` transition pops before dispatching to
//!    arm targets, so that proof's property is preserved by construction,
//!    not just true of one hand-built fixture.
//!
//! V-1 (balance) is necessary but not sufficient on its own: two paths can
//! return the abstract stack to the same *depth* while disagreeing on its
//! *contents* (open guard A, open guard B, close-any, close-any — depth
//! returns to zero on both paths, but which scope closed first differs).
//! This module tracks stack CONTENTS (`Vec<ScopeToken>`, kind + static
//! opening address), not a scalar depth, and requires exact content
//! equality at every CFG merge point (mirroring `verify_program`'s
//! existing operand-height merge-consistency check, generalized from a
//! scalar to a structured value) — this is what discharges V-2.
//!
//! **V-1's soundness is bounded by `successors()`'s completeness (V3
//! review remediation forward note).** This walk only visits addresses
//! `successors()` tells it are reachable; V-1's "empty at every END" is
//! therefore only as sound as "every real v2 CFG edge is enumerated by
//! `successors()`." If a future v2 word or edge shape is added to
//! `Instr` without a matching `successors()` arm, this module will
//! silently under-visit — it has no independent way to notice a missing
//! edge, since it has no model of the ISA other than what `successors()`
//! exposes. Any future v2 word MUST land its `successors()` arm and its
//! `verify_v2_control_stack` transition together, not separately.
//!
//! **V-8 is deliberately NOT narrowed (V3 review disposition, forward
//! note).** Rejecting every backward v2 edge unconditionally is
//! correctly conservative and stays that way — but it is a real
//! constraint on V5: whatever BPMN/DSL construct V5's frontends lower
//! into a backward-pointing guard handler, race arm, or fork target (if
//! any such construct exists) cannot be expressed this way and needs a
//! different lowering. Recorded here so V5 checks this against its
//! actual lowering needs rather than discovering it by a rejected
//! compile.

use crate::artifact::{race_arm_targets, require_address, successors, ArtifactError};
use crate::types::{Addr, Instr};
use std::collections::{BTreeMap, VecDeque};

/// What kind of scope a control-stack entry represents. Distinct kinds are
/// never interchangeable at a close/pop site — `V2GuardEnd` closing a
/// `Race`-kind entry (reachable only via a malformed program, e.g. two
/// divergent branches opening different scope kinds before a shared
/// closer) is a V-2 nesting violation, not merely "stack non-empty."
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ScopeKind {
    Guard,
    GuardN,
    /// A18: `GUARD-R>`. Distinct from `Guard` even though both are
    /// interrupting — V-10's dominance check and the `V2CancelScope`
    /// target-kind check both key off this specifically, not off
    /// "any interrupting guard."
    GuardR,
    Race,
    Barrier,
}

impl ScopeKind {
    fn word(self) -> &'static str {
        match self {
            ScopeKind::Guard => "GUARD>",
            ScopeKind::GuardN => "GUARD-N>",
            ScopeKind::GuardR => "GUARD-R>",
            ScopeKind::Race => "RACE{",
            ScopeKind::Barrier => "FORK",
        }
    }
}

/// One entry on the abstract control stack: which kind of scope, and the
/// static address of the instruction that opened it. `opened_at` is what
/// lets `V2Join` verify it's popping the barrier belonging to *its own*
/// `pairing`, not merely "some barrier" (V-3's arity/pairing check is a
/// pop-time identity comparison, not a separate side-table lookup).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ScopeToken {
    kind: ScopeKind,
    opened_at: Addr,
}

type AbstractStack = Vec<ScopeToken>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct V2ControlStackLimits {
    pub(crate) max_control_depth: u32,
    pub(crate) max_barriers: u32,
    pub(crate) max_records: u32,
}

fn violation(address: usize, theorem: &str, reason: impl std::fmt::Display) -> ArtifactError {
    ArtifactError::InvalidInstruction {
        address: Addr::from(address as u32),
        reason: format!("{theorem}: {reason}"),
    }
}

/// Insert `state` as address `address`'s abstract entry state. If an entry
/// state already exists there (a CFG merge point, including a re-entrant
/// loop's second-and-later visits), it must be byte-for-byte identical —
/// disagreement is V-2 (two paths reach the same point with the control
/// stack in different shapes). Legal re-entrancy (V2.7's
/// `reentrant_fork_join_in_bounded_loop` fixture) revisits the same
/// address with the SAME computed state each iteration (same static
/// tokens, same order), so this check passes trivially for it — the same
/// mechanism that rejects illegal divergence admits legal repetition,
/// which is exactly the discriminating property 3.4 must prove.
fn propagate(
    entry_states: &mut BTreeMap<usize, AbstractStack>,
    worklist: &mut VecDeque<usize>,
    at_address: usize,
    address: usize,
    state: AbstractStack,
) -> Result<(), ArtifactError> {
    match entry_states.get(&address) {
        Some(existing) if *existing != state => Err(violation(
            at_address,
            "V-2",
            format!(
                "control-stack state disagrees at CFG merge point {address}: \
                 {existing:?} versus {state:?}"
            ),
        )),
        Some(_) => Ok(()),
        None => {
            entry_states.insert(address, state);
            worklist.push_back(address);
            Ok(())
        }
    }
}

fn pop_expecting(
    address: usize,
    state: &AbstractStack,
    expected: ScopeKind,
) -> Result<AbstractStack, ArtifactError> {
    let mut popped = state.clone();
    match popped.pop() {
        None => Err(violation(
            address,
            "V-1",
            format!(
                "control-stack underflow: {} closes a scope but none is open",
                expected.word()
            ),
        )),
        Some(top) if top.kind != expected => Err(violation(
            address,
            "V-2",
            format!(
                "nesting violation: {} expects to close a {:?}-kind scope, \
                 found {:?}-kind (opened at {})",
                expected.word(),
                expected,
                top.kind,
                top.opened_at
            ),
        )),
        Some(_) => Ok(popped),
    }
}

/// `pairing`'s Fork's own fan-out arity — 1 if, for any reason, `pairing`
/// doesn't address a `V2Fork` (should not happen: every `Barrier` token
/// this module ever constructs is built from a `V2Fork` it just matched
/// on; `1` is a safe, non-panicking fallback, not a claim this path is
/// expected).
fn fork_arity(pairing: Addr, instructions: &[Instr]) -> u32 {
    match instructions.get(pairing.index()) {
        Some(Instr::V2Fork { targets, .. }) => targets.len().max(1) as u32,
        _ => 1,
    }
}

/// How many concurrent dynamic fibre-instances share the CURRENT abstract
/// program point — the product of every enclosing `V2Fork`'s fan-out. A
/// stack with zero `Barrier` tokens has multiplicity 1 (no replication).
fn fibre_multiplicity(state: &AbstractStack, instructions: &[Instr]) -> u32 {
    state
        .iter()
        .filter(|token| token.kind == ScopeKind::Barrier)
        .fold(1u32, |acc, token| {
            acc.saturating_mul(fork_arity(token.opened_at, instructions))
        })
}

/// V3 review remediation (BLOCKING, V-7): the first cut counted
/// `max_records`/`max_barriers` as "number of distinct scope-opening
/// instructions this static walk visits" — which is not "maximum
/// simultaneously-live concurrency-table records." A guard inside a
/// `V2Fork`ed body is ONE static instruction but is live once per spawned
/// fibre; visiting it once under-counted by exactly the fork's fan-out.
///
/// Fix: at every visited address, the number of records simultaneously
/// live AT THAT POINT is `state.len()` (every currently-open scope on
/// this abstract fibre's stack) multiplied by `fibre_multiplicity`
/// (how many concurrent dynamic fibres share this static point, from
/// enclosing `V2Fork`s). `max_records`/`max_barriers` are the maximum of
/// that product over every visited address, not a running increment.
/// This is a deliberately CONSERVATIVE (may over-count, must never
/// under-count) upper bound — e.g. it does not attempt to prove two
/// mutually-exclusive `BrIf` branches can never be simultaneously live,
/// which would require real concurrent-liveness analysis, not a single
/// linear stack walk. Over-counting a runtime safety bound is safe;
/// under-counting it is the exact defect this fix closes. `loop_multiplier`
/// (passed in from `verify_program`'s existing backward-`BrCounterLt`
/// accounting) scales the final maxima to also cover re-entrant-loop
/// replication, which this walk's merge-convergence (by design, for
/// V-1/V-2 purposes) does not re-count on a loop's second-and-later visit
/// to the same address.
///
/// V3.1/V3.2 — the dual-stack abstract interpreter's control-stack half.
/// Discharges V-1 (balance; NOTE: soundness here depends on `successors()`
/// enumerating every real CFG edge — see the module-level note added by
/// V3 review remediation), V-2 (nesting via kind+identity), V-3 (static
/// `V2Join`/`V2Fork` pairing referential check — resolution itself is
/// exclusively the kernel's dynamic-handle job, never this function's;
/// the `Addr`/`RecordId` wall makes any reach into runtime resolution a
/// compile error, so there is nothing here that *could* reach for it),
/// V-4 (handler entry-state validity — derived separately per guard kind,
/// see the `V2Guard`/`V2GuardN` arms below), V-5 (reuses
/// `race_arm_targets`'s contiguity check, already built in V2.7), V-7
/// (see the doc comment above), and V-8 (backward-edge bounding,
/// generalized from `verify_program`'s v1-only check to cover v2 edges
/// too — kept unconditional per V3 review disposition; V5's frontends
/// must confirm they can live with a hard "no backward v2 edge, ever"
/// rule, since it is not narrowed here).
/// Which instructions are `successors()`'s existing "no continuation"
/// class — the terminal set V-11 proves reachability *to*. This is not a
/// new taxonomy: `successors()` already groups `End`/`EndTerminate`/`Fail`
/// under one arm (artifact.rs, "no fallthrough"), and its `V2CancelScope`
/// arm's own doc comment says its no-successor treatment is "matching
/// End/EndTerminate/Fail's no-fallthrough treatment above" — i.e. the
/// codebase already treats these four as one equivalence class of "static
/// dead end, by design." §18 ruling J's text names only `END`/
/// `END-TERMINATE` (the v2-relevant case it was written against), but a
/// literal reading that excludes `Fail`/`V2CancelScope` would reject
/// existing, cement-locked fixtures that legitimately end a reachable path
/// on one of them (`Fail`: `bpmn-lite-engine`'s `t_loop_3_counter_starts_at_zero`;
/// `V2CancelScope`: `bpmn-lite-kernel`'s cancel-scope fixtures, which
/// document it as "deliberately a dead end in the static CFG ... even
/// though the kernel does continue the calling fibre past it dynamically").
/// Adopting `successors()`'s own no-continuation grouping is the
/// non-inventive resolution — flagged here, not silently decided, since
/// it widens the ratified text rather than transcribing it verbatim.
fn is_terminal(instruction: &Instr) -> bool {
    matches!(
        instruction,
        Instr::End
            | Instr::EndTerminate
            | Instr::Fail { .. }
            | Instr::V2CancelScope
            // §18 ruling J's V5 lowering glue: `V2RouteZeroMatch`
            // unconditionally raises an Incident and has no successors
            // (`artifact::successors()`) — the same "static dead end, by
            // design" class as `Fail`/`V2CancelScope`, not a new taxonomy.
            | Instr::V2RouteZeroMatch
    )
}

/// V-11 (§7, §18 ruling J): every instruction reachable from the program's
/// entry point (address 0) must have a forward path to a terminal
/// instruction — the dual of `verify_program`'s existing forward-
/// reachability walk, which only proves *some* `End`/`EndTerminate` is
/// reachable (a single boolean), not that every reachable node can get to
/// one. Rejects any artifact containing a reachable instruction with no
/// path to any terminal, naming the offending address.
///
/// **Must run after V-8** (structurally enforced by call order in
/// `verify_program`: this is invoked only after `verify_v2_control_stack`
/// — which enforces V-8 — has already returned `Ok`). This is what makes
/// the V-8 interaction safe without any special-casing here: by the time
/// this walk runs, the only backward edge that can still exist in
/// `instructions` is a bounded `BrCounterLt` loop's own back-edge (every
/// v2 backward edge, and every unbounded v1 `Jump`/`BrIf`/`BrIfNot`
/// backward edge, was already rejected upstream). This function does
/// ordinary directed-graph reachability over `successors()`'s edges,
/// including that back-edge — and that is exactly correct, not merely
/// tolerated: a bounded loop's back-edge participating as a normal graph
/// edge does not manufacture false reachability, because reachability
/// here is computed as "reverse-BFS from terminal nodes," not "the
/// forward walk's own path." A loop with a forward exit has that exit
/// edge lead somewhere that (transitively) reaches a terminal, so every
/// node in the loop body reaches a terminal via the exit, back-edge or
/// not. A loop with NO forward exit has no such edge at all, so no node
/// in it is reverse-reachable from any terminal regardless of the
/// back-edge — the failure mode the ruling flags (mistaking the back-edge
/// itself for evidence of reachability) cannot occur in a reverse-BFS
/// formulation, only in a formulation that tried to read reachability off
/// the forward walk's own visitation order.
pub(crate) fn verify_terminal_reachability(instructions: &[Instr]) -> Result<(), ArtifactError> {
    let len = instructions.len();
    if len == 0 {
        // verify_program already rejects an empty instruction stream
        // before this ever runs; nothing to walk.
        return Ok(());
    }

    // Forward reachability from address 0, recording each visited
    // address's own successor set so the reverse graph below doesn't need
    // a second call to `successors()`.
    let mut forward_reachable = vec![false; len];
    let mut forward_edges: Vec<Vec<usize>> = vec![Vec::new(); len];
    let mut queue: VecDeque<usize> = VecDeque::new();
    forward_reachable[0] = true;
    queue.push_back(0);
    while let Some(address) = queue.pop_front() {
        let succs = successors(address, &instructions[address], instructions, len)?;
        for &next in &succs {
            if !forward_reachable[next] {
                forward_reachable[next] = true;
                queue.push_back(next);
            }
        }
        forward_edges[address] = succs;
    }

    // Reverse graph, built only from edges actually walked above.
    let mut reverse_edges: Vec<Vec<usize>> = vec![Vec::new(); len];
    for (from, tos) in forward_edges.iter().enumerate() {
        for &to in tos {
            reverse_edges[to].push(from);
        }
    }

    // Reverse-BFS from every reachable terminal instruction.
    let mut reaches_terminal = vec![false; len];
    let mut rev_queue: VecDeque<usize> = VecDeque::new();
    for address in 0..len {
        if forward_reachable[address] && is_terminal(&instructions[address]) {
            reaches_terminal[address] = true;
            rev_queue.push_back(address);
        }
    }
    while let Some(address) = rev_queue.pop_front() {
        for &pred in &reverse_edges[address] {
            if !reaches_terminal[pred] {
                reaches_terminal[pred] = true;
                rev_queue.push_back(pred);
            }
        }
    }

    for address in 0..len {
        if forward_reachable[address] && !reaches_terminal[address] {
            return Err(violation(
                address,
                "V-11",
                format!(
                    "instruction {:?} at address {address} has no path to any terminal \
                     instruction (END/END-TERMINATE) — unreachable end-state",
                    instructions[address]
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) fn verify_v2_control_stack(
    instructions: &[Instr],
    loop_multiplier: u64,
) -> Result<V2ControlStackLimits, ArtifactError> {
    let len = instructions.len();
    let mut entry_states: BTreeMap<usize, AbstractStack> = BTreeMap::new();
    let mut worklist: VecDeque<usize> = VecDeque::new();
    entry_states.insert(0, Vec::new());
    worklist.push_back(0);

    let mut max_control_depth = 0u32;
    let mut max_barriers = 0u32;
    let mut max_records = 0u32;

    // V-8: a v2 edge (guard handler, race-arm-via-}RACE, fork target) that
    // points backward is illegal — v2 has no native bounded-loop primitive
    // of its own; re-entrancy must route through v1's `BrCounterLt`, which
    // `verify_program` already bounds separately. Unlike that check (which
    // special-cases specific v1 opcodes), this one is edge-general: ANY
    // backward v2 edge is rejected outright, since nothing about a guard
    // handler, an armed race alternative, or a fork target is a loop
    // counter that could legitimize it.
    let check_forward = |from: usize, to: usize, edge: &str| -> Result<(), ArtifactError> {
        if to <= from {
            return Err(violation(
                from,
                "V-8",
                format!(
                    "backward {edge} edge to {to} — v2 has no bounded-loop \
                     primitive of its own; re-entrancy must route through a \
                     v1 BrCounterLt loop, not a backward v2 control edge"
                ),
            ));
        }
        Ok(())
    };

    while let Some(address) = worklist.pop_front() {
        let state = entry_states
            .get(&address)
            .cloned()
            .expect("address was only ever queued alongside its entry state");
        require_address(Addr::from(address as u32), len, Addr::from(address as u32), "v2 verifier")?;

        let depth = state.len() as u32;
        max_control_depth = max_control_depth.max(depth);

        // V-7 fix (see the function doc comment): live-record count at
        // THIS address, not an increment-per-visited-instruction. Applied
        // uniformly to every address, not just scope-opening ones — an
        // address deep inside several open scopes is itself evidence of
        // that many simultaneously-live records, independent of which
        // instruction happens to sit there.
        let multiplicity = fibre_multiplicity(&state, instructions);
        let barrier_count = state
            .iter()
            .filter(|token| token.kind == ScopeKind::Barrier)
            .count() as u32;
        max_records = max_records.max(depth.saturating_mul(multiplicity));
        max_barriers = max_barriers.max(barrier_count.saturating_mul(multiplicity));

        let instruction = &instructions[address];
        match instruction {
            // V-4, V3 review remediation (BLOCKING): `V2Guard` and
            // `V2GuardN` are given INDEPENDENT derivations, not one
            // shared arm — §5 defines their trigger behaviour
            // differently, and treating them identically was an
            // unjustified inference, not a transcription.
            //
            // `V2Guard` (interrupting): "GUARD> ... On trigger: unwind
            // members innermost-first, run handler." Unwinding happens
            // BEFORE the handler runs, and unwinding retires this guard's
            // own record along with everything nested inside it. By the
            // time the handler executes, this guard's own token no
            // longer exists on any stack — so the handler's entry state
            // is the state that existed BEFORE this `V2Guard`'s own push
            // (PRE-push).
            Instr::V2Guard { handler } => {
                check_forward(address, handler.index(), "guard handler")?;
                propagate(&mut entry_states, &mut worklist, address, handler.index(), state.clone())?;
                if address + 1 < len {
                    let mut pushed = state;
                    pushed.push(ScopeToken {
                        kind: ScopeKind::Guard,
                        opened_at: Addr::from(address as u32),
                    });
                    propagate(&mut entry_states, &mut worklist, address, address + 1, pushed)?;
                }
            }
            // `V2GuardN` (non-interrupting): "GUARD-N> ... on trigger,
            // spawn handler fibre WITHOUT unwinding members; record stays
            // armed." Unlike the interrupting case, this guard's own
            // record is explicitly NOT retired when the handler fires —
            // it stays a live, referenceable scope. The handler fibre is
            // a NEW fibre spawned from this still-armed activation, which
            // is exactly the shape `V2Fork`'s children-inherit-parent-
            // stack-plus-own-token pattern already establishes elsewhere
            // in this walker: a fibre spawned from an open scope inherits
            // that scope's own token, it does not skip past it. So the
            // handler's entry state is the state WITH this `V2GuardN`'s
            // own token still present (POST-push) — the SAME state the
            // fallthrough edge gets, not the pre-push state.
            //
            // This is a genuine divergence from treating `V2Guard`/
            // `V2GuardN` identically (the first V2.7/V3 cut's shared
            // arm, and the reviewer's own stated lean during the V3
            // addressing review) — flagged explicitly for confirmation,
            // not silently landed as settled.
            Instr::V2GuardN { handler } => {
                let mut pushed = state;
                pushed.push(ScopeToken {
                    kind: ScopeKind::GuardN,
                    opened_at: Addr::from(address as u32),
                });
                check_forward(address, handler.index(), "guard-n handler")?;
                propagate(&mut entry_states, &mut worklist, address, handler.index(), pushed.clone())?;
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, pushed)?;
                }
            }
            Instr::V2GuardEnd => {
                let popped = pop_expecting(address, &state, ScopeKind::Guard)?;
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, popped)?;
                }
            }
            Instr::V2GuardNEnd => {
                let popped = pop_expecting(address, &state, ScopeKind::GuardN)?;
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, popped)?;
                }
            }
            // A18 V-10 (dominance): `GUARD-R>`'s scope must dominate any
            // `FORK` within its extent — it may CONTAIN a complete
            // `FORK`/`JOIN` region (that falls out of ordinary well-
            // nestedness, no extra check needed: a `Barrier` token pushed
            // after this one and popped before this one closes is already
            // legal), but must never itself be opened while a `Barrier`
            // token is already on the entry stack (i.e. nested inside a
            // fork's own child fibre). Rolling back shared instance-wide
            // state — `domain_payload`/`flags`/`join_expected`/session
            // stack — while sibling fork branches keep running
            // concurrently is exactly the barrier-starvation hazard this
            // forecloses (a real reproduction of the un-guarded version of
            // this hazard is `fork_join_barrier_starves_when_a_member_dies_without_arriving_via_guard_rollback`
            // in `bpmn-lite-kernel`'s test suite — this check makes that
            // shape unreachable via `GUARD-R>` by construction, not merely
            // caught downstream).
            Instr::V2GuardR => {
                if let Some(enclosing_fork) = state.iter().find(|token| token.kind == ScopeKind::Barrier) {
                    return Err(violation(
                        address,
                        "V-10",
                        format!(
                            "GUARD-R> opened while nested inside a FORK (barrier opened at \
                             {}) — a rollback-capable guard must dominate any FORK within its \
                             extent, never be contained by one",
                            enclosing_fork.opened_at
                        ),
                    ));
                }
                if address + 1 < len {
                    let mut pushed = state;
                    pushed.push(ScopeToken {
                        kind: ScopeKind::GuardR,
                        opened_at: Addr::from(address as u32),
                    });
                    propagate(&mut entry_states, &mut worklist, address, address + 1, pushed)?;
                }
            }
            Instr::V2GuardREnd => {
                let popped = pop_expecting(address, &state, ScopeKind::GuardR)?;
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, popped)?;
                }
            }
            // §18 v0.10 ruling I: `GUARD-TIMER>` arms the guard scope it
            // immediately follows. "Immediately" is enforced structurally
            // here (`top.opened_at.index() + 1 == address`), not merely
            // "some guard is open somewhere on the stack" — the ratified
            // design is an arming trigger established "at guard-open
            // time," and adjacency is the cheapest static proof of that
            // without introducing per-record runtime bookkeeping. Legal
            // after any of the three guard-kind opens (`V2Guard`/
            // `V2GuardN`/`V2GuardR`) — arming composes uniformly across
            // interrupting/non-interrupting/rollback, per the ruling's own
            // text ("the guard doesn't care whether the work inside is
            // synchronous... indifferent to disposition"). No state
            // change (as `V2ArmTimer`'s identical arm below) — arming
            // touches no concurrency record and no control stack.
            Instr::V2GuardArmTimer => {
                match state.last() {
                    Some(top)
                        if matches!(
                            top.kind,
                            ScopeKind::Guard | ScopeKind::GuardN | ScopeKind::GuardR
                        ) && top.opened_at.index() + 1 == address =>
                    {
                        if address + 1 < len {
                            propagate(&mut entry_states, &mut worklist, address, address + 1, state)?;
                        }
                    }
                    Some(top) => {
                        return Err(violation(
                            address,
                            "GUARD-TIMER>",
                            format!(
                                "must immediately follow the guard-open instruction it arms — \
                                 top of stack is {:?}-kind, opened at {} (this instruction is \
                                 not its immediate successor)",
                                top.kind, top.opened_at
                            ),
                        ));
                    }
                    None => {
                        return Err(violation(
                            address,
                            "GUARD-TIMER>",
                            "no open guard scope to arm",
                        ));
                    }
                }
            }
            // Post-close remediation (V&S §13 amendment v0.5 ruling A,
            // restored — see `Instr::V2GuardTimerCycle`'s own doc comment
            // for the full rationale). Two static shape requirements, both
            // checked here rather than left to the kernel to discover at
            // runtime (fail-closed, per CLAUDE.md): (1) must immediately
            // follow the `V2GuardArmTimer` it bounds — checked by
            // instruction identity at `address - 1`, not merely "some
            // guard is open," since `V2GuardArmTimer` is itself optional
            // and a `GUARD-N>` opened without one must not accept a
            // dangling `GUARD-TIMER-CYCLE>` two addresses later by
            // coincidence; (2) the guard it bounds must be `GUARD-N>`
            // specifically — `GUARD>`/`GUARD-R>` retire their record on
            // trigger (never rearm), so bounding a repeat count on either
            // is meaningless, not merely unusual.
            Instr::V2GuardTimerCycle { max_fires } => {
                if *max_fires == 0 {
                    return Err(violation(
                        address,
                        "GUARD-TIMER-CYCLE>",
                        "max_fires must be >= 1 — a cycle that never fires once is not a cycle",
                    ));
                }
                let immediate_predecessor_is_arm_timer =
                    address > 0 && matches!(instructions.get(address - 1), Some(Instr::V2GuardArmTimer));
                match state.last() {
                    Some(top) if top.kind == ScopeKind::GuardN && immediate_predecessor_is_arm_timer => {
                        if address + 1 < len {
                            propagate(&mut entry_states, &mut worklist, address, address + 1, state)?;
                        }
                    }
                    Some(top) if top.kind == ScopeKind::GuardN => {
                        return Err(violation(
                            address,
                            "GUARD-TIMER-CYCLE>",
                            "must immediately follow the GUARD-TIMER> instruction it bounds \
                             (this instruction is not V2GuardArmTimer's immediate successor)",
                        ));
                    }
                    Some(top) => {
                        return Err(violation(
                            address,
                            "GUARD-TIMER-CYCLE>",
                            format!(
                                "only a GUARD-N> scope's timer may carry a repeat cycle — top \
                                 of stack is {:?}-kind, opened at {}",
                                top.kind, top.opened_at
                            ),
                        ));
                    }
                    None => {
                        return Err(violation(
                            address,
                            "GUARD-TIMER-CYCLE>",
                            "no open guard scope to bound",
                        ));
                    }
                }
            }
            // §18 v0.10 ruling I, second arming-trigger kind (BoundaryError
            // v2 migration): `GUARD-ERROR>` arms the guard scope it's part
            // of an arming run for. Unlike `V2GuardArmTimer` (which may
            // only ever be the FIRST instruction after guard-open — one
            // timer per guard), `V2GuardArmError` instructions may stack N
            // deep (multiple error routes on one guard), so adjacency is
            // "immediately after guard-open, OR immediately after another
            // arm instruction (`V2GuardArmTimer`/`V2GuardArmError`/
            // `V2GuardTimerCycle`) that is itself part of the same
            // contiguous arming run" — checking only the immediate
            // predecessor is sufficient (not recursive) because that
            // predecessor was already validated by this same rule when the
            // verifier's forward walk reached it.
            Instr::V2GuardArmError { error_code, .. } => {
                let immediate_predecessor_is_arm = address > 0
                    && matches!(
                        instructions.get(address - 1),
                        Some(Instr::V2GuardArmTimer)
                            | Some(Instr::V2GuardArmError { .. })
                            | Some(Instr::V2GuardTimerCycle { .. })
                    );
                match state.last() {
                    Some(top)
                        if matches!(
                            top.kind,
                            ScopeKind::Guard | ScopeKind::GuardN | ScopeKind::GuardR
                        ) && (top.opened_at.index() + 1 == address
                            || immediate_predecessor_is_arm) =>
                    {
                        // Bytecode-level duplicate-catch-all check (defense
                        // in depth alongside `verifier.rs`'s IR-level 8d
                        // check — lowering is not itself verified, and
                        // hand-assembled bytecode fixtures bypass the IR
                        // layer entirely): at most one `error_code: None`
                        // arm per guard's arming run.
                        if error_code.is_none() {
                            let dup = (top.opened_at.index() + 1..address).any(|i| {
                                matches!(
                                    instructions.get(i),
                                    Some(Instr::V2GuardArmError { error_code: None, .. })
                                )
                            });
                            if dup {
                                return Err(violation(
                                    address,
                                    "GUARD-ERROR>",
                                    "at most one catch-all (error_code: None) GUARD-ERROR> arm \
                                     is allowed per guard",
                                ));
                            }
                        } else {
                            // Same defense-in-depth, generalized to a
                            // specific code (`verifier.rs`'s IR-level 8e):
                            // two arms for the same code on one guard would
                            // silently leave the second unreachable at
                            // runtime (`error_routes`'s `.find()` matches
                            // only the first), not rejected.
                            let dup = (top.opened_at.index() + 1..address).any(|i| {
                                matches!(
                                    instructions.get(i),
                                    Some(Instr::V2GuardArmError { error_code: Some(other), .. })
                                        if other == error_code.as_ref().unwrap()
                                )
                            });
                            if dup {
                                return Err(violation(
                                    address,
                                    "GUARD-ERROR>",
                                    format!(
                                        "at most one GUARD-ERROR> arm for error code '{}' is \
                                         allowed per guard",
                                        error_code.as_ref().unwrap()
                                    ),
                                ));
                            }
                        }
                        if address + 1 < len {
                            propagate(&mut entry_states, &mut worklist, address, address + 1, state)?;
                        }
                    }
                    Some(top) => {
                        return Err(violation(
                            address,
                            "GUARD-ERROR>",
                            format!(
                                "must immediately follow the guard-open instruction it arms, or \
                                 another arm instruction arming the same guard — top of stack is \
                                 {:?}-kind, opened at {} (this instruction is not part of that \
                                 guard's arming run)",
                                top.kind, top.opened_at
                            ),
                        ));
                    }
                    None => {
                        return Err(violation(
                            address,
                            "GUARD-ERROR>",
                            "no open guard scope to arm",
                        ));
                    }
                }
            }
            Instr::V2RaceOpen { .. } => {
                if address + 1 < len {
                    let mut pushed = state;
                    pushed.push(ScopeToken {
                        kind: ScopeKind::Race,
                        opened_at: Addr::from(address as u32),
                    });
                    propagate(&mut entry_states, &mut worklist, address, address + 1, pushed)?;
                }
            }
            // Arm words only register (V2.7 remediation r.2) — no state
            // change, plain fallthrough.
            Instr::V2ArmTimer { .. } | Instr::V2ArmMsg { .. } | Instr::V2ArmEffect { .. } => {
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, state)?;
                }
            }
            Instr::V2RaceClose => {
                let popped = pop_expecting(address, &state, ScopeKind::Race)?;
                for target in race_arm_targets(address, instructions)? {
                    check_forward(address, target.index(), "race arm (via }RACE)")?;
                    propagate(&mut entry_states, &mut worklist, address, target.index(), popped.clone())?;
                }
            }
            Instr::V2Fork { targets, pairing } => {
                for target in targets.iter().copied() {
                    check_forward(address, target.index(), "fork")?;
                    // The children-inherit-parent-stack-plus-barrier
                    // semantics MUST be coded explicitly here (V2.7
                    // addressing-review handoff note) — this is not
                    // derived from `v2_control_stack_effect`, which
                    // returns `None` for `V2Fork` precisely because this
                    // effect is cross-fibre and that function is scoped
                    // to a single fibre's own stack.
                    let mut child_state = state.clone();
                    child_state.push(ScopeToken {
                        kind: ScopeKind::Barrier,
                        opened_at: *pairing,
                    });
                    propagate(&mut entry_states, &mut worklist, address, target.index(), child_state)?;
                }
            }
            Instr::V2Join { pairing } => {
                // V-3, static half: the pairing must reference an actual
                // V2Fork. Dynamic resolution (which activation, which
                // arrival is last) is exclusively the kernel's job — this
                // function never touches a RecordId/Handle, only Addr, so
                // there is no runtime-resolution code path here to get
                // wrong.
                if !matches!(
                    instructions.get(pairing.index()),
                    Some(Instr::V2Fork { .. })
                ) {
                    return Err(violation(
                        address,
                        "V-3",
                        format!("pairing {pairing} does not reference a V2Fork instruction"),
                    ));
                }
                let popped = match state.last() {
                    Some(top) if top.kind == ScopeKind::Barrier && top.opened_at == *pairing => {
                        pop_expecting(address, &state, ScopeKind::Barrier)?
                    }
                    Some(top) => {
                        return Err(violation(
                            address,
                            "V-3",
                            format!(
                                "V2Join pairing {pairing} does not match the barrier on top \
                                 of the control stack (kind {:?}, opened at {})",
                                top.kind, top.opened_at
                            ),
                        ));
                    }
                    None => {
                        return Err(violation(
                            address,
                            "V-1",
                            "control-stack underflow: JOIN closes a barrier but none is open",
                        ));
                    }
                };
                if address + 1 < len {
                    propagate(&mut entry_states, &mut worklist, address, address + 1, popped)?;
                }
            }
            Instr::V2CancelScope => {
                // `( -- ) [ h ]` — reads the top scope without popping it
                // (matches `v2_control_stack_effect`'s `Peek`
                // classification). Modeling assumption (flagged for 3.5):
                // requires a non-empty control stack — there must be a
                // scope to cancel. No successors (matches
                // `successors()`'s no-fallthrough treatment): unwind
                // semantics do not resume this fibre.
                if state.is_empty() {
                    return Err(violation(
                        address,
                        "V-1",
                        "CANCEL-SCOPE with an empty control stack — nothing to cancel",
                    ));
                }
                // A18/V-10 companion check: `V2CancelScope` only ever
                // carries a rollback snapshot to restore when its target
                // is `GUARD-R>`-opened — `ConcurrencyRecord::rollback_*`
                // is `None` for `V2Guard`/`V2GuardN` records (see
                // `Instr::V2Guard`'s doc comment). Rejecting a
                // mistargeted `V2CancelScope` here, statically, means the
                // kernel's own runtime check in `v2_rollback_guard_scope`
                // ("handle carries no rollback snapshot") can never
                // actually fire against a verified program — this is the
                // fail-closed version of that guarantee, not a second
                // independent opinion that might disagree with it.
                if let Some(top) = state.last() {
                    if top.kind != ScopeKind::GuardR {
                        return Err(violation(
                            address,
                            "V-10",
                            format!(
                                "CANCEL-SCOPE targets a {} scope (opened at {}) — only a \
                                 GUARD-R> scope carries a rollback snapshot to restore",
                                top.kind.word(),
                                top.opened_at
                            ),
                        ));
                    }
                }
            }
            Instr::End => {
                if !state.is_empty() {
                    return Err(violation(
                        address,
                        "V-1",
                        format!(
                            "control stack not empty at program end: {} scope(s) still open ({:?})",
                            state.len(),
                            state
                        ),
                    ));
                }
            }
            // `Instr::EndTerminate` is deliberately NOT matched here — same
            // exemption `Instr::Fail` already gets (`Fail` is likewise
            // absent from this match and falls through to the `_` arm
            // below). Both kill the whole instance rather than completing
            // one fibre's own scopes: V-1 exists to prevent an orphaned
            // scope (a fibre ending while a record it opened stays live),
            // which cannot happen when every fibre dies and every record
            // retires with it. Falling through to `_` means the walk ends
            // with no complaint about the open stack — `EndTerminate` has
            // zero successors per the terminal grouping in `successors()`
            // (matches `Fail`/`V2CancelScope`/`End`/`EndTerminate`), so the
            // walk simply terminates here. The K-invariant side of this
            // exemption (every concurrency-table record still `Armed` when
            // `EndTerminate` fires must actually be retired) is enforced at
            // the kernel level, not statically here — see
            // `Instr::EndTerminate`'s kernel handler.
            //
            // v1 instructions, and v2 words with no control-stack effect
            // (V2WaitFor/V2WaitUntil/V2WaitMsg/V2AwaitEffect — `None` per
            // `v2_control_stack_effect`, genuinely total for these, unlike
            // V2Fork): state passes through unchanged along whatever
            // `successors()` already computes.
            _ => {
                for successor in successors(address, instruction, instructions, len)? {
                    propagate(&mut entry_states, &mut worklist, address, successor, state.clone())?;
                }
            }
        }
    }

    // Scale by loop_multiplier (u32-saturating): a scope-opening
    // instruction inside a bounded loop's body is visited once by this
    // walk (subsequent iterations converge in `propagate` without
    // re-deriving a "new" live-record count), but at runtime it re-arms
    // once per loop iteration. `max_control_depth` is deliberately NOT
    // scaled — depth is a per-fibre nesting property, unaffected by how
    // many times the same static address is dynamically revisited.
    let loop_multiplier_u32 = u32::try_from(loop_multiplier).unwrap_or(u32::MAX);
    Ok(V2ControlStackLimits {
        max_control_depth,
        max_barriers: max_barriers.saturating_mul(loop_multiplier_u32),
        max_records: max_records.saturating_mul(loop_multiplier_u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{ArtifactEnvelope, ExecutableWorkflow};
    use crate::types::CompiledProgram;
    use std::collections::BTreeMap as StdBTreeMap;

    fn empty_compiled_program(instructions: Vec<Instr>) -> CompiledProgram {
        CompiledProgram {
            bytecode_version: [0u8; 32],
            program: instructions,
            debug_map: StdBTreeMap::new(),
            join_plan: StdBTreeMap::new(),
            wait_plan: StdBTreeMap::new(),
            message_name_map: StdBTreeMap::new(),
            write_set: StdBTreeMap::new(),
            task_manifest: Vec::new(),
            flag_symbol_table: StdBTreeMap::new(),
            data_objects: StdBTreeMap::new(),
            ffi_task_decls: StdBTreeMap::new(),
            v2_ffi_task_decls: StdBTreeMap::new(),
            v2_corr_sources: StdBTreeMap::new(),
            v2_guard_budgets: StdBTreeMap::new(),
            default_guard_budget: crate::transition::ScopeFailureBudget::conservative_default(),
            default_retry_policy: crate::transition::RetryPolicy::conservative_default(),
        }
    }

    fn admit(instructions: Vec<Instr>) -> V2ControlStackLimits {
        verify_v2_control_stack(&instructions, 1).expect("expected this program to be admitted")
    }

    fn reject(instructions: Vec<Instr>) -> ArtifactError {
        verify_v2_control_stack(&instructions, 1).expect_err("expected this program to be rejected")
    }

    // ─── V-7: max_records must reflect fork-fibre multiplicity, not
    // static-instruction-visit count ────────────────────────────

    /// V3 review remediation (BLOCKING, V-7). Reviewer's required
    /// fixture: a `V2Fork` spawning N=3 fibres, each of which jumps into
    /// ONE SHARED `V2Guard` instruction (not three separate guards at
    /// three separate addresses — that case would have happened to give
    /// the right total even under the old bug, since 3 distinct visited
    /// addresses each incremented by 1 also totals 3, masking the defect).
    /// The old code counted "was this address visited," giving 1 (one
    /// static `V2Guard`, visited once — `propagate`'s merge-convergence
    /// means a second/third arrival at the same address with the same
    /// state doesn't re-run the counting logic). The fix must show 3 live
    /// guards, since all three forked fibres can hold the guard open
    /// simultaneously.
    #[test]
    fn v7_fork_spawning_n_fibres_into_one_shared_guard_yields_max_records_n_not_one() {
        let instructions = vec![
            /* 0 */ Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(3), Addr::new(4)]),
                pairing: Addr::new(0),
            },
            /* 1 */ Instr::V2CancelScope, // unreachable filler, never targeted
            /* 2 */ Instr::Jump { target: Addr::new(5) }, // branch A -> shared guard
            /* 3 */ Instr::Jump { target: Addr::new(5) }, // branch B -> shared guard
            /* 4 */ Instr::Jump { target: Addr::new(5) }, // branch C -> shared guard
            /* 5 */ Instr::V2Guard { handler: Addr::new(7) }, // ONE static guard, 3 fibres
            /* 6 */ Instr::V2GuardEnd,
            /* 7 */ Instr::V2Join { pairing: Addr::new(0) }, // handler edge (pre-push
            //       state = [Barrier(0)]) and GuardEnd's fallthrough (also
            //       [Barrier(0)], post-pop) converge here cleanly — both see
            //       the same content, no V-2 merge mismatch.
            /* 8 */ Instr::End,
        ];
        let limits = admit(instructions);
        // The old (pre-fix) code counted "was this address visited" and
        // would total 1 (one static V2Guard, visited once — `propagate`'s
        // merge-convergence means branches B and C arriving at the same
        // address with the same state don't re-run the counting logic).
        // The fix's peak is 6, not 3: at address 6 (V2GuardEnd's entry,
        // i.e. inside the guard's own scope AND still inside the fork's
        // barrier), depth is 2 (Barrier + Guard both open) and
        // fibre_multiplicity is 3 (the fork's arity) — 2 * 3 = 6. This is
        // the deliberately conservative direction documented on
        // `verify_v2_control_stack`: the barrier is one shared record
        // referenced by all 3 fibres, not 3 separate barrier records, but
        // counting it as if replicated over-approximates rather than
        // under-approximates, which is the safe direction for a runtime
        // bound. Either way, 6 (or even the guard-only reading of 3) is
        // categorically different from the pre-fix answer of 1 — the
        // property under test is "N-fold, not 1-fold," not the exact
        // constant.
        assert_eq!(
            limits.max_records, 6,
            "one guard instruction reached by 3 forked fibres must NOT collapse \
             to 1 live record (the pre-fix defect)"
        );
    }

    // ─── V-4, per-kind, V3 review remediation (BLOCKING): derived and
    // fixture-proven independently for V2Guard vs V2GuardN ──────

    /// `V2Guard` (interrupting): handler entry state is PRE-push. The
    /// handler target here is an `End` with no `V2GuardEnd` before it —
    /// this only admits if the handler is entered with an EMPTY stack
    /// (the same empty state the program started with, before this
    /// guard's own token would exist).
    #[test]
    fn v4_guard_handler_entry_state_is_pre_push() {
        admit(vec![
            /* 0 */ Instr::V2Guard { handler: Addr::new(3) },
            /* 1 */ Instr::V2GuardEnd,
            /* 2 */ Instr::End, // main line: guard opened then closed, empty at End
            /* 3 */ Instr::End, // handler: must ALSO be empty (pre-push) to admit
        ]);
    }

    /// `V2GuardN` (non-interrupting): handler entry state is POST-push —
    /// the handler inherits this `V2GuardN`'s own still-armed token,
    /// mirroring `V2Fork`'s children-inherit-parent-stack-plus-own-token
    /// pattern. The handler target here is a `V2GuardNEnd` — this only
    /// admits if the handler's entry state actually HAS a `GuardN`-kind
    /// token on top to pop; under the (rejected) pre-push model this
    /// would be a V-1 underflow instead.
    #[test]
    fn v4_guardn_handler_entry_state_is_post_push_inherits_own_token() {
        admit(vec![
            /* 0 */ Instr::V2GuardN { handler: Addr::new(3) },
            /* 1 */ Instr::V2GuardNEnd,
            /* 2 */ Instr::End,
            /* 3 */ Instr::V2GuardNEnd, // handler: pops the still-armed GuardN token
            /* 4 */ Instr::End,
        ]);
    }

    // ─── GUARD-ERROR>: duplicate error-code arms ─────────────────

    /// Bytecode-level counterpart to `verifier.rs`'s IR-level 8e: two
    /// `V2GuardArmError` arms on one guard's arming run with the SAME
    /// specific code must be rejected — a hand-assembled fixture bypasses
    /// the IR layer entirely, so this is the only check standing between
    /// it and the silent second-arm-unreachable hazard.
    #[test]
    fn guard_arm_error_duplicate_specific_code_rejected() {
        let err = reject(vec![
            /* 0 */ Instr::V2Guard { handler: Addr::new(5) },
            /* 1 */
            Instr::V2GuardArmError {
                error_code: Some("SANCTIONS_HIT".into()),
                handler: Addr::new(5),
            },
            /* 2 */
            Instr::V2GuardArmError {
                error_code: Some("SANCTIONS_HIT".into()),
                handler: Addr::new(5),
            },
            /* 3 */ Instr::V2GuardEnd,
            /* 4 */ Instr::End,
            /* 5 */ Instr::End,
        ]);
        assert!(
            err.to_string().contains("SANCTIONS_HIT"),
            "expected the duplicate-code rejection to name the offending code, got: {err}"
        );
    }

    /// Sanity counterpart: two `V2GuardArmError` arms with DIFFERENT
    /// specific codes on one guard remain admitted.
    #[test]
    fn guard_arm_error_distinct_specific_codes_admitted() {
        admit(vec![
            /* 0 */ Instr::V2Guard { handler: Addr::new(5) },
            /* 1 */
            Instr::V2GuardArmError {
                error_code: Some("SANCTIONS_HIT".into()),
                handler: Addr::new(5),
            },
            /* 2 */
            Instr::V2GuardArmError {
                error_code: Some("TIMEOUT".into()),
                handler: Addr::new(5),
            },
            /* 3 */ Instr::V2GuardEnd,
            /* 4 */ Instr::End,
            /* 5 */ Instr::End,
        ]);
    }

    // ─── V-1: balance ───────────────────────────────────────────

    #[test]
    fn v1_admits_a_balanced_guard() {
        let limits = admit(vec![
            Instr::V2Guard { handler: Addr::new(3) },
            Instr::V2GuardEnd,
            Instr::End,
            Instr::End,
        ]);
        assert_eq!(limits.max_control_depth, 1);
    }

    #[test]
    fn v1_rejects_unclosed_guard_at_end() {
        let err = reject(vec![Instr::V2Guard { handler: Addr::new(2) }, Instr::End, Instr::End]);
        assert!(format!("{err}").contains("V-1"));
    }

    #[test]
    fn v1_rejects_close_with_nothing_open() {
        let err = reject(vec![Instr::V2GuardEnd, Instr::End]);
        assert!(format!("{err}").contains("V-1"));
    }

    // ─── V-2: nesting/identity, not just depth ─────────────────

    #[test]
    fn v2_rejects_wrong_kind_close() {
        // Opens a Race, closes as if it were a Guard — depth-balanced,
        // kind-mismatched.
        let err = reject(vec![
            Instr::V2RaceOpen { arm_count: 0 },
            Instr::V2GuardEnd,
            Instr::End,
        ]);
        assert!(format!("{err}").contains("V-2"), "{err}");
    }

    #[test]
    fn v2_rejects_cross_path_bracket_violation() {
        // Two divergent branches open DIFFERENT scope kinds, then merge
        // into a single V2GuardEnd. Both branches are individually
        // depth-balanced (depth 1 going into the merge) but disagree on
        // WHAT is open — exactly the discriminating case the addressing
        // review called out. Structurally: a BrIf choosing between a
        // V2Guard-opening path and a V2RaceOpen-opening path, both
        // targeting the same V2GuardEnd.
        let instructions = vec![
            /* 0 */ Instr::BrIf { target: Addr::new(3) },
            /* 1 */ Instr::V2Guard { handler: Addr::new(5) }, // guard branch (not taken)
            /* 2 */ Instr::Jump { target: Addr::new(4) },
            /* 3 */ Instr::V2RaceOpen { arm_count: 0 }, // race branch (taken)
            /* 4 */ Instr::V2GuardEnd, // merge point: expects Guard, race branch has Race
            /* 5 */ Instr::End,
            /* 6 */ Instr::End,
        ];
        let err = reject(instructions);
        let msg = format!("{err}");
        assert!(msg.contains("V-2"), "{msg}");
    }

    // ─── V-3: static pairing ────────────────────────────────────

    #[test]
    fn v3_rejects_join_pairing_not_a_fork() {
        let err = reject(vec![
            Instr::V2Join { pairing: Addr::new(0) }, // pairing points at itself, not a Fork
            Instr::End,
        ]);
        assert!(format!("{err}").contains("V-3"), "{err}");
    }

    #[test]
    fn v3_rejects_join_pairing_mismatched_fork() {
        // Two forks; JOIN claims to belong to the wrong one.
        let instructions = vec![
            /* 0 */ Instr::V2Fork {
                targets: Box::new([Addr::new(3)]),
                pairing: Addr::new(0),
            },
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(4)]),
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::End,
            /* 3 */ Instr::V2Join { pairing: Addr::new(1) }, // wrong pairing: barrier on stack is 0
            /* 4 */ Instr::End,
        ];
        let err = reject(instructions);
        assert!(format!("{err}").contains("V-3"), "{err}");
    }

    // ─── V-5: race shape (reuses race_arm_targets, sanity-checked here) ──

    #[test]
    fn v5_rejects_arm_count_mismatch() {
        let err = reject(vec![
            Instr::V2RaceOpen { arm_count: 2 },
            Instr::V2ArmMsg { target: Addr::new(3), name: 1 },
            Instr::V2RaceClose,
            Instr::End,
        ]);
        // race_arm_targets returns an InvalidInstruction with its own
        // (non "V-N"-prefixed) reason; just confirm it's rejected, not the
        // exact tag.
        assert!(matches!(err, ArtifactError::InvalidInstruction { .. }));
    }

    // ─── V-8: backward v2 edges ─────────────────────────────────

    #[test]
    fn v8_rejects_backward_guard_handler() {
        // The verifier's entry point is always address 0, so the guard
        // must be reached by an ordinary forward edge first; the
        // violation under test is specifically the guard's OWN handler
        // edge pointing backward, not the path leading to the guard.
        let err = reject(vec![
            /* 0 */ Instr::Jump { target: Addr::new(1) },
            /* 1 */ Instr::V2Guard { handler: Addr::new(0) }, // backward!
            /* 2 */ Instr::V2GuardEnd,
            /* 3 */ Instr::End,
        ]);
        assert!(format!("{err}").contains("V-8"), "{err}");
    }

    // ─── V-11: terminal reachability (§18 ruling J) ────────────

    fn admit_terminal_reachability(instructions: Vec<Instr>) {
        verify_terminal_reachability(&instructions)
            .expect("expected this program to be admitted by V-11")
    }

    fn reject_terminal_reachability(instructions: Vec<Instr>) -> ArtifactError {
        verify_terminal_reachability(&instructions)
            .expect_err("expected this program to be rejected by V-11")
    }

    /// V-11 positive: a plain forward path from `START` to `End` — the
    /// simplest possible instance of "every reachable instruction has a
    /// path to a terminal."
    #[test]
    fn v11_admits_a_straight_line_path_to_end() {
        admit_terminal_reachability(vec![
            /* 0 */ Instr::Jump { target: Addr::new(1) },
            /* 1 */ Instr::End,
        ]);
    }

    /// V-11 negative: address 3 is reachable (the `BrIf` target edge) but
    /// is the LAST instruction in the stream and not terminal-kind — no
    /// successor, no path to any `End`/`EndTerminate`/`Fail`/
    /// `V2CancelScope`. Must be rejected, and the error must name address
    /// 3 specifically (not just "some" instruction).
    #[test]
    fn v11_rejects_a_reachable_dead_end_naming_the_offending_address() {
        let err = reject_terminal_reachability(vec![
            /* 0 */ Instr::PushBool(true),
            /* 1 */ Instr::BrIf { target: Addr::new(3) },
            /* 2 */ Instr::End, // fallthrough branch: fine, reaches a terminal
            /* 3 */ Instr::CancelWait { wait_id: 0 }, // taken branch: dead end
        ]);
        let msg = format!("{err}");
        assert!(msg.contains("V-11"), "{msg}");
        assert!(
            msg.contains("address 3") || msg.contains(" 3 "),
            "error must name the offending address (3): {msg}"
        );
        match err {
            ArtifactError::InvalidInstruction { address, .. } => {
                assert_eq!(address, Addr::new(3), "must name address 3, not merely reject");
            }
            other => panic!("expected InvalidInstruction naming an address, got {other}"),
        }
    }

    /// An instruction unreachable from `START` (never targeted by any
    /// edge) is outside V-11's scope entirely — this is the same "dual of
    /// the start-reachability walk" framing: V-11 only obligates addresses
    /// the forward walk actually visits, exactly as `verify_program`'s own
    /// forward walk only ever visits (and therefore only ever validates)
    /// reachable addresses. A stray, never-targeted dead-end instruction
    /// must NOT cause a V-11 rejection.
    #[test]
    fn v11_ignores_an_instruction_unreachable_from_start() {
        admit_terminal_reachability(vec![
            /* 0 */ Instr::End,
            /* 1 */ Instr::CancelWait { wait_id: 0 }, // never targeted by anything
        ]);
    }

    /// V-11 + V-8 interaction (Adam's ruling explicitly asks for this
    /// fixture): a bounded `IncCounter`/`BrCounterLt` loop whose ONLY exit
    /// is `BrCounterLt`'s forward fallthrough edge into a terminal. The
    /// loop's own back-edge (`BrCounterLt`'s `target: Addr::new(0)`) is a
    /// real, legal, backward CFG edge — bounded loops are the one
    /// backward-edge shape still standing by the time V-11 runs (V-8
    /// forbids every OTHER backward edge unconditionally) — and this test
    /// proves the walk treats that back-edge as an ordinary graph edge
    /// rather than mistaking it for evidence of reachability: every node
    /// in the loop body reaches `End` via the FORWARD exit edge, not via
    /// the cycle.
    #[test]
    fn v11_admits_a_bounded_loop_whose_only_exit_is_a_forward_conditional_edge_to_end() {
        admit_terminal_reachability(vec![
            /* 0 */ Instr::IncCounter { counter_id: 0 },
            /* 1 */ Instr::PushBool(true), // loop-body filler, keeps this a >1-instr body
            /* 2 */ Instr::Pop,
            /* 3 */ Instr::BrCounterLt { counter_id: 0, limit: 3, target: Addr::new(0) },
            /* 4 */ Instr::End, // the loop's only exit: BrCounterLt's forward fallthrough
        ]);
    }

    /// V-11 negative, the loop-shaped case: a bounded loop with NO forward
    /// exit at all — `BrCounterLt` is the last instruction, so its
    /// fallthrough edge doesn't exist, leaving only the backward edge.
    /// Every node in the loop body can reach the loop head, but nothing
    /// reaches a terminal — must be rejected, not admitted by mistaking
    /// the cycle for a path out.
    #[test]
    fn v11_rejects_a_bounded_loop_with_no_forward_exit() {
        let err = reject_terminal_reachability(vec![
            /* 0 */ Instr::IncCounter { counter_id: 0 },
            /* 1 */ Instr::BrCounterLt { counter_id: 0, limit: 3, target: Addr::new(0) }, // last instr: no fallthrough
        ]);
        assert!(format!("{err}").contains("V-11"), "{err}");
    }

    /// V-11 admitted through the REAL `ArtifactEnvelope::from_legacy_program`
    /// path (not just the standalone function) — proves V-11 is actually
    /// wired into `verify_program` and runs after V-8, using a full,
    /// legally-verified bounded-loop-with-parallel-fork program (adapted
    /// from `reentrant_fork_join_in_bounded_loop_is_admitted`, which only
    /// exercises `verify_v2_control_stack` directly).
    #[test]
    fn v11_bounded_loop_with_fork_join_admitted_through_the_real_artifact_construction_path() {
        let instructions = vec![
            /* 0 */ Instr::IncCounter { counter_id: 0 },
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(3), Addr::new(5)]),
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::V2CancelScope, // dead filler, never targeted
            /* 3 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 4 */ Instr::Jump { target: Addr::new(7) },
            /* 5 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 6 */ Instr::Jump { target: Addr::new(7) },
            /* 7 */ Instr::BrCounterLt { counter_id: 0, limit: 3, target: Addr::new(0) },
            /* 8 */ Instr::End, // the loop's only exit
        ];
        let program = empty_compiled_program(instructions);
        let envelope = ArtifactEnvelope::from_legacy_program(program, "v11-test")
            .expect("bounded loop with a forward-only exit to End must be admitted by V-11");
        let bytes = envelope.canonical_bytes().unwrap();
        ExecutableWorkflow::verify(&bytes).expect("must re-verify after round-trip");
    }

    /// V-11 rejected through the REAL `ArtifactEnvelope::from_legacy_program`
    /// path — a `V2Fork` branch (B) whose `V2Join` is never wired to a
    /// continuation: it is the LAST instruction in the stream, so it has
    /// no successors at all (`successors()`'s generic fallthrough only
    /// applies when a next address exists). Branch A wires its `V2Join`
    /// forward to a real `End` and is fine on its own; branch B's
    /// unreachable-end-state is exactly the "gateway branch with no route
    /// to any END" shape V-11 exists to catch, proving V-11 is enforced
    /// end to end (not merely reachable via the standalone function) and
    /// that it inspects EVERY reachable instruction, not just whether
    /// *some* End exists anywhere in the program (branch A's End would
    /// already satisfy `verify_program`'s pre-existing single-boolean
    /// `reachable_end` check on its own).
    #[test]
    fn v11_unreachable_end_state_rejected_through_the_real_artifact_construction_path() {
        let instructions = vec![
            /* 0 */ Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(5)]),
                pairing: Addr::new(0),
            },
            /* 1 */ Instr::V2CancelScope, // dead filler, never targeted
            /* 2 */ Instr::V2Join { pairing: Addr::new(0) }, // branch A -> addr3
            /* 3 */ Instr::Jump { target: Addr::new(4) },
            /* 4 */ Instr::End,
            /* 5 */ Instr::V2Join { pairing: Addr::new(0) }, // branch B: last instruction, no
            //       continuation wired at all — dead end.
        ];
        let program = empty_compiled_program(instructions);
        let err = ArtifactEnvelope::from_legacy_program(program, "v11-test")
            .expect_err("a fork branch with no path to a terminal must be rejected");
        assert!(format!("{err}").contains("V-11"), "{err}");
    }

    // ─── legal re-entrant FORK/JOIN must be ADMITTED ───────────

    #[test]
    fn reentrant_fork_join_in_bounded_loop_is_admitted() {
        let instructions = vec![
            /* 0 */ Instr::IncCounter { counter_id: 0 },
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(3), Addr::new(5)]),
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::V2CancelScope, // dead filler, matches V2.7's fixture shape
            /* 3 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 4 */ Instr::Jump { target: Addr::new(7) },
            /* 5 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 6 */ Instr::Jump { target: Addr::new(7) },
            /* 7 */ Instr::BrCounterLt { counter_id: 0, limit: 3, target: Addr::new(0) },
            /* 8 */ Instr::End,
        ];
        let limits = admit(instructions);
        // V-7 fix: 2, not 1 — the fork has arity 2 (targets: [3, 5]), so
        // both V2Join addresses are visited with one Barrier token on the
        // stack whose fork_arity is 2, giving fibre_multiplicity 2 at
        // each. `admit()` calls the standalone function with
        // loop_multiplier=1 (no further scaling for the BrCounterLt
        // bound here) — the loop-multiplier scaling itself is exercised
        // separately through the real `ArtifactEnvelope` path in
        // `ex_oracle_program_admitted_through_the_real_artifact_construction_path`.
        assert_eq!(limits.max_barriers, 2);
    }

    // ─── V2.7's own fixtures, re-verified end to end through the real
    // ArtifactEnvelope construction path (proves V3 is actually wired in,
    // not just callable standalone) ──────────────────────────────

    fn ex_oracle_program() -> Vec<Instr> {
        vec![
            /* 0  */ Instr::V2Guard { handler: Addr::new(8) },
            /* 1  */ Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(4)]),
                pairing: Addr::new(1),
            },
            /* 2  */ Instr::V2Join { pairing: Addr::new(1) },
            /* 3  */ Instr::Jump { target: Addr::new(6) },
            /* 4  */ Instr::V2Join { pairing: Addr::new(1) },
            /* 5  */ Instr::Jump { target: Addr::new(6) },
            /* 6  */ Instr::V2GuardEnd,
            /* 7  */ Instr::End,
            /* 8  */ Instr::V2RaceOpen { arm_count: 2 },
            /* 9  */ Instr::PushI64(5_000),
            /* 10 */ Instr::V2ArmTimer { target: Addr::new(13) },
            /* 11 */ Instr::V2ArmMsg { target: Addr::new(14), name: 42 },
            /* 12 */ Instr::V2RaceClose,
            /* 13 */ Instr::Jump { target: Addr::new(15) },
            /* 14 */ Instr::Jump { target: Addr::new(15) },
            /* 15 */ Instr::End,
        ]
    }

    #[test]
    fn ex_oracle_program_admitted_through_the_real_artifact_construction_path() {
        let program = empty_compiled_program(ex_oracle_program());
        let envelope = ArtifactEnvelope::from_legacy_program(program, "v3-test")
            .expect("EX-oracle-shaped program must be admitted end to end");
        assert!(envelope.limits().max_control_depth() >= 1);
        let bytes = envelope.canonical_bytes().unwrap();
        ExecutableWorkflow::verify(&bytes).expect("must re-verify after round-trip");
    }

    /// V-9, structurally: a v2-only artifact carries none of the v1
    /// join-plan/wait-plan side tables. V5.3 (§18, landed 2026-07-23)
    /// deleted `race_plan`/`boundary_map` (and their only producer, v1's
    /// boundary-timer mechanism) entirely — `ArtifactMetadata` no longer
    /// even has those fields, so the "carries none of the v1 race-plan/
    /// boundary-route side tables" half of this requirement is now closed
    /// at the type level (stronger than any runtime check: there is
    /// nothing left to populate). `join_plan`/`wait_plan` remain typed
    /// fields (their own v1 producers — `Instr::Join`/`WaitMsg` — are also
    /// deleted this same step, but the side-table types themselves are
    /// unrelated to this test's scope) and are checked here as before.
    #[test]
    fn v9_v2_only_artifact_carries_no_v1_join_wait_side_tables() {
        let program = empty_compiled_program(ex_oracle_program());
        let envelope = ArtifactEnvelope::from_legacy_program(program, "v3-test").unwrap();
        let metadata = envelope.metadata();
        assert!(metadata.join_plan().is_empty());
        assert!(metadata.wait_plan().is_empty());
    }

    // V5.3 (§18, landed 2026-07-23): the V-9 active-rejection test that used
    // to sit here (`v9_mixed_v1_v2_artifact_with_populated_race_plan_is_
    // actively_rejected`) proved a mixed v1/v2 artifact — a real v1
    // `Instr::WaitAny` with a populated `race_plan` alongside a `V2Guard`
    // in the same instruction stream — was actively rejected, not merely
    // absent by construction, because "a mixed v1/v2 artifact IS
    // type-level constructible" at the time it was written (this test's
    // own doc comment said so explicitly: "since `Instr` is one enum
    // containing both v1 and v2 variants, nothing stops a `Vec<Instr>`
    // from mixing them"). That premise is no longer true: `Instr::WaitAny`
    // and `race_plan`/`RaceId`/`RacePlanEntry`/`WaitArm` are deleted
    // outright this step — there is no longer any way to construct the
    // mixed program this test built, so the runtime check it exercised
    // (`verify_program`'s old "v2-bearing artifact must carry none of the
    // v1 side tables" active check) was deleted alongside it, per
    // `bpmn-lite-types/src/artifact.rs`'s own in-place note. This isn't a
    // silent loss of coverage — the property the test proved (a v1/v2 mix
    // is impossible) now holds unconditionally, for every artifact, by
    // construction, which is strictly stronger than "rejected by a
    // targeted runtime check when someone tries."

    /// `docs/todo/EOP-EX-BPMN-ISA-002.md`'s draft oracle program — an
    /// interrupting guard over a `V2Fork` whose branch B contains a race
    /// with a message alternative, branch A a plain timer wait. Proves
    /// the program this document's hand-trace is built on actually
    /// decodes and passes V3's real verifier (both branches converge on
    /// address 14 with identical control-stack content), rather than
    /// resting on the doc's own unverified claim.
    #[test]
    fn ex_oracle_draft_v2_program_from_eop_ex_doc_is_admitted() {
        let instructions = vec![
            /* 0  */ Instr::V2Guard { handler: Addr::new(16) },
            /* 1  */ Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(6)]),
                pairing: Addr::new(1),
            },
            /* 2  */ Instr::PushI64(60_000),
            /* 3  */ Instr::V2WaitFor,
            /* 4  */ Instr::V2Join { pairing: Addr::new(1) },
            /* 5  */ Instr::Jump { target: Addr::new(14) },
            /* 6  */ Instr::V2RaceOpen { arm_count: 2 },
            /* 7  */ Instr::PushI64(30_000),
            /* 8  */ Instr::V2ArmTimer { target: Addr::new(11) },
            /* 9  */ Instr::V2ArmMsg { target: Addr::new(12), name: 100 },
            /* 10 */ Instr::V2RaceClose,
            /* 11 */ Instr::Jump { target: Addr::new(13) },
            /* 12 */ Instr::Jump { target: Addr::new(13) },
            /* 13 */ Instr::V2Join { pairing: Addr::new(1) },
            /* 14 */ Instr::V2GuardEnd,
            /* 15 */ Instr::End,
            /* 16 */ Instr::ExecNative { task_type: 0, argc: 0, retc: 0 },
            /* 17 */ Instr::End,
        ];
        let mut program = empty_compiled_program(instructions);
        // task_manifest needs one entry for the handler's ExecNative
        // (task_type 0) to pass verify_program's manifest-bound check.
        program.task_manifest = vec!["NotifyCancelled".to_string()];
        let envelope = crate::artifact::ArtifactEnvelope::from_legacy_program(program, "ex-oracle-draft")
            .expect("EOP-EX-BPMN-ISA-002's draft program must be admitted");
        let bytes = envelope.canonical_bytes().unwrap();
        crate::artifact::ExecutableWorkflow::verify(&bytes).expect("must round-trip");
    }

    #[test]
    fn unbalanced_ex_oracle_variant_rejected_through_the_real_artifact_construction_path() {
        // A hand-shrunk variant of the EX-oracle fixture with the
        // V2GuardEnd removed — the guard opens on the main line but never
        // closes before End, a direct V-1 violation.
        let broken = vec![
            Instr::V2Guard { handler: Addr::new(7) },
            Instr::V2Fork {
                targets: Box::new([Addr::new(2), Addr::new(4)]),
                pairing: Addr::new(1),
            },
            Instr::V2Join { pairing: Addr::new(1) },
            Instr::Jump { target: Addr::new(6) },
            Instr::V2Join { pairing: Addr::new(1) },
            Instr::Jump { target: Addr::new(6) },
            Instr::End, // no V2GuardEnd before this End — V-1 violation
            Instr::End,
        ];
        let program = empty_compiled_program(broken);
        let err = ArtifactEnvelope::from_legacy_program(program, "v3-test")
            .expect_err("must reject an unbalanced control stack");
        assert!(format!("{err}").contains("V-1"), "{err}");
    }

    // ─── 3.4 — mutation corpus: legal admitted, illegal rejected,
    // zero panics ──────────────────────────────────────────────
    //
    // Each mutation below starts from a known-legal seed (the EX-oracle
    // fixture or the re-entrant FORK/JOIN loop) and flips exactly one
    // structural property. A verifier that admits any of these is unsound
    // regardless of what else passes — these are not "nice to have" extra
    // coverage, they are the two discriminating cases the addressing
    // review and Adam's V3 disposition both named explicitly: re-entrant
    // FORK/JOIN must be admitted (already proven above,
    // `reentrant_fork_join_in_bounded_loop_is_admitted`), and a cross-path
    // bracket violation must be rejected (already proven above,
    // `v2_rejects_cross_path_bracket_violation`). The mutations here round
    // out the corpus with every other way a hand-assembled v2 program can
    // be malformed, so a green run here is more than "V-1..V-8 didn't
    // regress" — it is "each theorem individually still fires."

    fn assert_rejected_no_panic(instructions: Vec<Instr>, expect_tag: &str) {
        let result = std::panic::catch_unwind(|| verify_v2_control_stack(&instructions, 1));
        let result = result.unwrap_or_else(|_| {
            panic!("verify_v2_control_stack panicked on a malformed mutation — 3.4 requires zero panics")
        });
        let err = result.expect_err("mutation was expected to be rejected");
        assert!(
            format!("{err}").contains(expect_tag),
            "expected a {expect_tag} rejection, got: {err}"
        );
    }

    #[test]
    fn mutation_missing_race_open_before_race_close_rejected() {
        // }RACE with no RACE{ at all before it in the program — an empty
        // control stack, so this is caught as V-1 underflow before
        // `race_arm_targets`'s own backward-scan check would even run.
        assert_rejected_no_panic(
            vec![
                Instr::V2ArmMsg { target: Addr::new(2), name: 1 },
                Instr::V2RaceClose,
                Instr::End,
            ],
            "V-1",
        );
    }

    #[test]
    fn mutation_guardn_close_on_plain_guard_rejected() {
        // Opens a plain GUARD>, closes with <GUARD-N — kind mismatch, the
        // "distinct opcodes, not a flag" requirement made adversarial.
        let instructions = vec![
            Instr::V2Guard { handler: Addr::new(3) },
            Instr::V2GuardNEnd,
            Instr::End,
            Instr::End,
        ];
        let err = verify_v2_control_stack(&instructions, 1).expect_err("must reject");
        assert!(format!("{err}").contains("V-2"), "{err}");
    }

    #[test]
    fn mutation_join_before_any_fork_rejected() {
        assert_rejected_no_panic(
            vec![Instr::V2Join { pairing: Addr::new(0) }, Instr::End],
            "V-3",
        );
    }

    #[test]
    fn mutation_race_arm_target_backward_rejected() {
        let instructions = vec![
            /* 0 */ Instr::Jump { target: Addr::new(1) },
            /* 1 */ Instr::V2RaceOpen { arm_count: 1 },
            /* 2 */ Instr::V2ArmMsg { target: Addr::new(0), name: 1 }, // backward!
            /* 3 */ Instr::V2RaceClose,
            /* 4 */ Instr::End,
        ];
        let err = verify_v2_control_stack(&instructions, 1).expect_err("must reject");
        assert!(format!("{err}").contains("V-8"), "{err}");
    }

    #[test]
    fn mutation_fork_target_backward_rejected() {
        let instructions = vec![
            /* 0 */ Instr::Jump { target: Addr::new(1) },
            /* 1 */ Instr::V2Fork {
                targets: Box::new([Addr::new(0)]), // backward!
                pairing: Addr::new(1),
            },
            /* 2 */ Instr::End,
        ];
        let err = verify_v2_control_stack(&instructions, 1).expect_err("must reject");
        assert!(format!("{err}").contains("V-8"), "{err}");
    }

    #[test]
    fn mutation_end_with_open_race_rejected() {
        let instructions = vec![
            Instr::V2RaceOpen { arm_count: 0 },
            Instr::End, // no }RACE before this End
        ];
        let err = verify_v2_control_stack(&instructions, 1).expect_err("must reject");
        assert!(format!("{err}").contains("V-1"), "{err}");
    }

    #[test]
    fn mutation_cancel_scope_with_empty_stack_rejected() {
        let instructions = vec![Instr::V2CancelScope];
        let err = verify_v2_control_stack(&instructions, 1).expect_err("must reject");
        assert!(format!("{err}").contains("V-1"), "{err}");
    }

    #[test]
    fn mutation_operand_underflow_arm_timer_without_push_rejected_by_verify_program() {
        // r.1 made V2ArmTimer pop its duration from the operand stack; a
        // program that omits the PushI64 must be rejected by the
        // OPERAND-stack half (verify_program's existing height walk,
        // V-6), independent of this module's control-stack half.
        use crate::artifact::ArtifactEnvelope;
        let instructions = vec![
            Instr::V2RaceOpen { arm_count: 1 },
            Instr::V2ArmTimer { target: Addr::new(3) }, // no PushI64 first!
            Instr::V2RaceClose,
            Instr::End,
        ];
        let program = empty_compiled_program(instructions);
        let err = ArtifactEnvelope::from_legacy_program(program, "v3-fuzz")
            .expect_err("operand-stack underflow must be rejected");
        assert!(format!("{err}").contains("underflow"), "{err}");
    }

    // ─── property test: no panic on arbitrary small v2-shaped programs ──
    //
    // Not a claim of semantic coverage (most random sequences are
    // trivially rejected on the first instruction) — this is purely a
    // robustness property: whatever the input, `verify_v2_control_stack`
    // must return `Result`, never panic. Addresses are kept in-bounds so
    // the fuzzer spends its budget on structural (not merely
    // out-of-bounds) malformation.
    mod no_panic_property {
        use super::*;
        use proptest::prelude::*;

        fn arb_instr(len: u32) -> impl Strategy<Value = Instr> {
            let addr = (0..len.max(1)).prop_map(Addr::new);
            prop_oneof![
                addr.clone().prop_map(|a| Instr::V2Guard { handler: a }),
                Just(Instr::V2GuardEnd),
                addr.clone().prop_map(|a| Instr::V2GuardN { handler: a }),
                Just(Instr::V2GuardNEnd),
                Just(Instr::V2GuardR),
                Just(Instr::V2GuardREnd),
                Just(Instr::V2GuardArmTimer),
                (0u32..4).prop_map(|n| Instr::V2GuardTimerCycle { max_fires: n }),
                (0u16..4).prop_map(|n| Instr::V2RaceOpen { arm_count: n }),
                addr.clone().prop_map(|a| Instr::V2ArmTimer { target: a }),
                addr.clone().prop_map(|a| Instr::V2ArmMsg { target: a, name: 1 }),
                Just(Instr::V2RaceClose),
                addr.clone().prop_map(|a| Instr::V2Fork {
                    targets: Box::new([a]),
                    pairing: a,
                }),
                addr.clone().prop_map(|a| Instr::V2Join { pairing: a }),
                Just(Instr::V2WaitFor),
                Just(Instr::V2CancelScope),
                Just(Instr::PushI64(1)),
                addr.clone().prop_map(|a| Instr::Jump { target: a }),
                Just(Instr::End),
                Just(Instr::V2RouteZeroMatch),
                Just(Instr::V2LoadPlaceholderMatch {
                    placeholder: "p".to_string(),
                    expected_value: "v".to_string(),
                }),
                Just(Instr::V2MiIndexLive { collection_flag: 0, index: 0 }),
                Just(Instr::V2MiLoadElement { collection_flag: 0, index: 0 }),
                Just(Instr::V2MiArityCheck { collection_flag: 0, max: 1 }),
            ]
        }

        proptest! {
            #[test]
            fn arbitrary_small_v2_program_never_panics(
                instructions in (1usize..10).prop_flat_map(|len| {
                    prop::collection::vec(arb_instr(len as u32), len)
                }),
            ) {
                let result = std::panic::catch_unwind(|| verify_v2_control_stack(&instructions, 1));
                prop_assert!(result.is_ok(), "verify_v2_control_stack panicked on {:?}", instructions);
            }
        }
    }
}
