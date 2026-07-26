//! F7 — covering-array topology corpus (ratified 2026-07-26; alphabet
//! widened same day): constrain the cartesian explosion of valid
//! execution graphs by covering the LOCAL logic alphabet deterministically
//! instead of sampling whole DAGs.
//!
//! The model (Adam's framing, confirmed): the atomic unit is a typed
//! logic pair — `(node → next)` plus the switch semantics the BPMN verb
//! permits at that point. The explosion is their arbitrary composition
//! (`v^n`); the constraint is a covering array — every interaction of the
//! declared factors appears in at least one enumerated shape, ~O(v²)
//! shapes instead of `v^n`:
//!
//!   factor 1  archetype adjacency — every ORDERED pair (A → B) of block
//!             archetypes adjacent at the top level;
//!   factor 2  switch outcome      — XOR both routes × both default
//!             shapes (empty / task-bearing — the two-sided tear catch);
//!             OR named-subset outcomes {both, one, none} (none =
//!             zero-match); MI collection lengths {0, 1, 4};
//!   factor 3  nesting depth       — every (gateway, content) at depth 1
//!             and every (gateway, gateway′, content) at depth 2; the
//!             NON-LOCAL factor pure pairwise adjacency would miss — plus
//!             the XOR×Boundary nestings (legal: XOR is not a
//!             synchronizing barrier).
//!
//! Division of labour: enumeration guarantees the STRUCTURE coverage;
//! coverage-guided libFuzzer keeps ownership of the DYNAMICS (completion
//! order, timer jumps, fault injection) — the covering shapes are written
//! as tape seeds so the fuzzer mutates runtime behaviour from guaranteed
//! structural starting points, and `covering_corpus_compiles_and_steps_
//! clean` runs the whole corpus deterministically in CI regardless.
//!
//! Recorded limits (no silent gaps):
//! - Boundary nests under XOR only: any And/Or ancestor is a
//!   synchronizing barrier its handler end-event escapes (V-1, cemented
//!   in the grammar's legality tests) — adjacency/singles/XOR-nesting
//!   cover it.
//! - The corpus covers depth ≤ 2 exhaustively; depth 3 remains the random
//!   grammar's territory (MAX_DEPTH), reached by mutation from the seeds.

use crate::{gen_shape, Block, Shape, Tape};

/// The local logic alphabet: canonical instantiations of every block
/// family × switch outcome. One entry = one letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Archetype {
    Task,
    /// Parallel, 2 branches × single task.
    And2,
    /// Parallel, 3 branches × single task.
    And3,
    /// XOR, guard taken, empty default flow.
    XorTaken,
    /// XOR, guard untaken, empty default flow.
    XorUntaken,
    /// XOR, guard taken, task-bearing default region (two-sided tear:
    /// default task bound 0).
    XorTakenDefault,
    /// XOR, guard untaken, task-bearing default region (default runs).
    XorUntakenDefault,
    /// Inclusive, both branches activated (full subset).
    Or2Both,
    /// Inclusive, first branch only (proper subset).
    Or2One,
    /// Inclusive, no branch activated (zero-match probe).
    Or2None,
    BoundaryInterrupting,
    BoundaryNonInterrupting,
    Mi0,
    Mi1,
    Mi4,
}

pub const ALL_ARCHETYPES: [Archetype; 15] = [
    Archetype::Task,
    Archetype::And2,
    Archetype::And3,
    Archetype::XorTaken,
    Archetype::XorUntaken,
    Archetype::XorTakenDefault,
    Archetype::XorUntakenDefault,
    Archetype::Or2Both,
    Archetype::Or2One,
    Archetype::Or2None,
    Archetype::BoundaryInterrupting,
    Archetype::BoundaryNonInterrupting,
    Archetype::Mi0,
    Archetype::Mi1,
    Archetype::Mi4,
];

/// Gateway wrappers for the nesting factors — one letter per gateway
/// family × switch outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gateway {
    And,
    XorTaken,
    XorUntaken,
    OrBoth,
    OrOne,
    OrNone,
}

pub const ALL_GATEWAYS: [Gateway; 6] = [
    Gateway::And,
    Gateway::XorTaken,
    Gateway::XorUntaken,
    Gateway::OrBoth,
    Gateway::OrOne,
    Gateway::OrNone,
];

/// XOR-only gateways — the ones Boundary may nest under (no synchronizing
/// barrier).
pub const XOR_GATEWAYS: [Gateway; 2] = [Gateway::XorTaken, Gateway::XorUntaken];

/// Archetypes legal INSIDE any gateway branch (Boundary excluded here; it
/// gets its own XOR-only nesting family).
pub const NESTABLE: [Archetype; 13] = [
    Archetype::Task,
    Archetype::And2,
    Archetype::And3,
    Archetype::XorTaken,
    Archetype::XorUntaken,
    Archetype::XorTakenDefault,
    Archetype::XorUntakenDefault,
    Archetype::Or2Both,
    Archetype::Or2One,
    Archetype::Or2None,
    Archetype::Mi0,
    Archetype::Mi1,
    Archetype::Mi4,
];

fn xor(take_guarded: bool, default: Vec<Block>) -> Block {
    Block::Xor {
        take_guarded,
        guarded: vec![Block::Task],
        default,
    }
}

fn or2(first: bool, second: bool) -> Block {
    Block::Or {
        branches: vec![(first, vec![Block::Task]), (second, vec![Block::Task])],
    }
}

impl Archetype {
    pub fn block(self) -> Block {
        match self {
            Archetype::Task => Block::Task,
            Archetype::And2 => Block::And {
                branches: vec![vec![Block::Task], vec![Block::Task]],
            },
            Archetype::And3 => Block::And {
                branches: vec![vec![Block::Task], vec![Block::Task], vec![Block::Task]],
            },
            Archetype::XorTaken => xor(true, vec![]),
            Archetype::XorUntaken => xor(false, vec![]),
            Archetype::XorTakenDefault => xor(true, vec![Block::Task]),
            Archetype::XorUntakenDefault => xor(false, vec![Block::Task]),
            Archetype::Or2Both => or2(true, true),
            Archetype::Or2One => or2(true, false),
            Archetype::Or2None => or2(false, false),
            Archetype::BoundaryInterrupting => Block::Boundary { interrupting: true },
            Archetype::BoundaryNonInterrupting => Block::Boundary {
                interrupting: false,
            },
            Archetype::Mi0 => Block::Mi { collection_len: 0 },
            Archetype::Mi1 => Block::Mi { collection_len: 1 },
            Archetype::Mi4 => Block::Mi { collection_len: 4 },
        }
    }

    /// Inverse of `block()` for canonical instantiations — the coverage
    /// witness recomputes coverage FROM the shapes, so it must classify,
    /// not trust construction.
    pub fn classify(block: &Block) -> Option<Archetype> {
        match block {
            Block::Task => Some(Archetype::Task),
            Block::And { branches }
                if branches.iter().all(|b| matches!(b[..], [Block::Task])) =>
            {
                match branches.len() {
                    2 => Some(Archetype::And2),
                    3 => Some(Archetype::And3),
                    _ => None,
                }
            }
            Block::Xor {
                take_guarded,
                guarded,
                default,
            } if matches!(guarded[..], [Block::Task]) => {
                match (&default[..], take_guarded) {
                    ([], true) => Some(Archetype::XorTaken),
                    ([], false) => Some(Archetype::XorUntaken),
                    ([Block::Task], true) => Some(Archetype::XorTakenDefault),
                    ([Block::Task], false) => Some(Archetype::XorUntakenDefault),
                    _ => None,
                }
            }
            Block::Or { branches } if branches.len() == 2 => {
                let all_tasks = branches.iter().all(|(_, b)| matches!(b[..], [Block::Task]));
                if !all_tasks {
                    return None;
                }
                match (branches[0].0, branches[1].0) {
                    (true, true) => Some(Archetype::Or2Both),
                    (true, false) => Some(Archetype::Or2One),
                    (false, false) => Some(Archetype::Or2None),
                    (false, true) => None,
                }
            }
            Block::Boundary { interrupting: true } => Some(Archetype::BoundaryInterrupting),
            Block::Boundary {
                interrupting: false,
            } => Some(Archetype::BoundaryNonInterrupting),
            Block::Mi { collection_len: 0 } => Some(Archetype::Mi0),
            Block::Mi { collection_len: 1 } => Some(Archetype::Mi1),
            Block::Mi { collection_len: 4 } => Some(Archetype::Mi4),
            _ => None,
        }
    }
}

impl Gateway {
    /// Wrap a region in this gateway. AND/OR pair the region with a plain
    /// second branch so the join is a real synchronizer; the region rides
    /// the FIRST branch (activated per the OR outcome letter).
    pub fn wrap(self, region: Vec<Block>) -> Block {
        match self {
            Gateway::And => Block::And {
                branches: vec![region, vec![Block::Task]],
            },
            Gateway::XorTaken => Block::Xor {
                take_guarded: true,
                guarded: region,
                default: vec![],
            },
            Gateway::XorUntaken => Block::Xor {
                take_guarded: false,
                guarded: region,
                default: vec![],
            },
            Gateway::OrBoth => Block::Or {
                branches: vec![(true, region), (true, vec![Block::Task])],
            },
            Gateway::OrOne => Block::Or {
                branches: vec![(true, region), (false, vec![Block::Task])],
            },
            Gateway::OrNone => Block::Or {
                branches: vec![(false, region), (false, vec![Block::Task])],
            },
        }
    }

    /// Classify the outermost gateway of a block and return its nested
    /// region (the FIRST branch, matching `wrap`).
    pub fn unwrap(block: &Block) -> Option<(Gateway, &[Block])> {
        match block {
            Block::And { branches } if branches.len() == 2 => {
                Some((Gateway::And, &branches[0]))
            }
            Block::Xor {
                take_guarded,
                guarded,
                default,
            } if default.is_empty() => Some((
                if *take_guarded {
                    Gateway::XorTaken
                } else {
                    Gateway::XorUntaken
                },
                guarded,
            )),
            Block::Or { branches } if branches.len() == 2 => {
                let gateway = match (branches[0].0, branches[1].0) {
                    (true, true) => Gateway::OrBoth,
                    (true, false) => Gateway::OrOne,
                    (false, false) => Gateway::OrNone,
                    (false, true) => return None,
                };
                Some((gateway, &branches[0].1))
            }
            _ => None,
        }
    }
}

/// The covering corpus:
///   singles      — every archetype alone;
///   pairs        — every ORDERED archetype adjacency [A, B];
///   depth 1      — every gateway wrapping every nestable content, plus
///                  the XOR×Boundary nestings;
///   depth 2      — every gateway∘gateway′ wrapping every nestable
///                  content.
pub fn covering_shapes() -> Vec<Shape> {
    let mut shapes = Vec::new();
    for a in ALL_ARCHETYPES {
        shapes.push(Shape {
            blocks: vec![a.block()],
        });
    }
    for a in ALL_ARCHETYPES {
        for b in ALL_ARCHETYPES {
            shapes.push(Shape {
                blocks: vec![a.block(), b.block()],
            });
        }
    }
    for g in ALL_GATEWAYS {
        for c in NESTABLE {
            shapes.push(Shape {
                blocks: vec![g.wrap(vec![c.block()])],
            });
        }
    }
    for g in XOR_GATEWAYS {
        for b in [
            Archetype::BoundaryInterrupting,
            Archetype::BoundaryNonInterrupting,
        ] {
            shapes.push(Shape {
                blocks: vec![g.wrap(vec![b.block()])],
            });
        }
    }
    for g in ALL_GATEWAYS {
        for g2 in ALL_GATEWAYS {
            for c in NESTABLE {
                shapes.push(Shape {
                    blocks: vec![g.wrap(vec![g2.wrap(vec![c.block()])])],
                });
            }
        }
    }
    // Dedupe structurally (e.g. And-wrap of Task == And2 single).
    let mut seen = std::collections::BTreeSet::new();
    shapes.retain(|shape| seen.insert(format!("{shape:?}")));
    shapes
}

/// Encode a covering shape as the tape prefix `gen_shape` decodes back to
/// exactly this shape — mirrors `gen_block`'s byte reads move for move,
/// cement-locked by the round-trip test. Panics on shapes the grammar
/// cannot express (a covering shape outside the grammar is a corpus bug).
pub fn encode_shape(shape: &Shape) -> Vec<u8> {
    let mut bytes = Vec::new();
    assert!(
        (1..=3).contains(&shape.blocks.len()),
        "top-level region must be 1-3 blocks"
    );
    bytes.push(shape.blocks.len() as u8 - 1); // count = 1 + b%3
    let mut budget = crate::BLOCK_BUDGET;
    for block in &shape.blocks {
        encode_block(block, crate::MAX_DEPTH, false, &mut budget, &mut bytes);
    }
    bytes
}

fn encode_block(
    block: &Block,
    depth: u8,
    under_barrier: bool,
    budget: &mut u32,
    bytes: &mut Vec<u8>,
) {
    assert!(*budget > 0, "covering shape exceeds BLOCK_BUDGET");
    *budget -= 1;
    match block {
        Block::Task => bytes.push(0),
        Block::And { branches } => {
            assert!(depth > 0, "And below MAX_DEPTH nesting");
            assert!(
                (2..=3).contains(&branches.len()),
                "grammar emits 2-3 AND branches"
            );
            bytes.push(3);
            bytes.push(branches.len() as u8 - 2); // 2 + b%2
            for branch in branches {
                encode_region(branch, depth - 1, true, budget, bytes);
            }
        }
        Block::Xor {
            take_guarded,
            guarded,
            default,
        } => {
            assert!(depth > 0, "Xor below MAX_DEPTH nesting");
            bytes.push(5);
            bytes.push(u8::from(*take_guarded)); // bool = b & 1
            bytes.push(u8::from(!default.is_empty())); // has_default
            encode_region(guarded, depth - 1, under_barrier, budget, bytes);
            if !default.is_empty() {
                encode_region(default, depth - 1, under_barrier, budget, bytes);
            }
        }
        Block::Or { branches } => {
            assert!(depth > 0, "Or below MAX_DEPTH nesting");
            assert!(
                (2..=3).contains(&branches.len()),
                "grammar emits 2-3 OR branches"
            );
            bytes.push(8);
            bytes.push(branches.len() as u8 - 2); // 2 + b%2
            for (active, branch) in branches {
                bytes.push(u8::from(*active));
                encode_region(branch, depth - 1, true, budget, bytes);
            }
        }
        Block::Boundary { interrupting } => {
            assert!(
                !under_barrier,
                "grammar never emits Boundary under a synchronizing barrier"
            );
            bytes.push(6);
            bytes.push(u8::from(*interrupting));
        }
        Block::Mi { collection_len } => {
            assert!(*collection_len <= 4, "MI length 0-4");
            bytes.push(7);
            bytes.push(*collection_len); // len = b%5
        }
    }
}

fn encode_region(
    blocks: &[Block],
    depth: u8,
    under_barrier: bool,
    budget: &mut u32,
    bytes: &mut Vec<u8>,
) {
    assert!(
        (1..=3).contains(&blocks.len()),
        "nested region must be 1-3 blocks"
    );
    bytes.push(blocks.len() as u8 - 1);
    for block in blocks {
        encode_block(block, depth, under_barrier, budget, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{drive_shape, emit_process, BpmnLiteEngine, FuzzClock, MemoryStore};
    use bpmn_lite_store::WorkflowStore;
    use bpmn_lite_types::TenantId;
    use std::collections::BTreeSet;
    use std::sync::Arc;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("build test runtime")
    }

    /// The corpus is what it claims: coverage is RECOMPUTED from the
    /// shapes via classification, never trusted from construction.
    #[test]
    fn covering_corpus_covers_the_declared_interactions() {
        let shapes = covering_shapes();
        let mut pairs = BTreeSet::new();
        let mut depth1 = BTreeSet::new();
        let mut depth2 = BTreeSet::new();
        for shape in &shapes {
            for window in shape.blocks.windows(2) {
                if let (Some(a), Some(b)) =
                    (Archetype::classify(&window[0]), Archetype::classify(&window[1]))
                {
                    pairs.insert((a, b));
                }
            }
            for block in &shape.blocks {
                if let Some((g, region)) = Gateway::unwrap(block) {
                    if let [inner] = region {
                        if let Some(c) = Archetype::classify(inner) {
                            depth1.insert((g, c));
                        }
                        if let Some((g2, region2)) = Gateway::unwrap(inner) {
                            if let [inner2] = region2 {
                                if let Some(c2) = Archetype::classify(inner2) {
                                    depth2.insert((g, g2, c2));
                                }
                            }
                        }
                    }
                }
            }
        }
        for a in ALL_ARCHETYPES {
            for b in ALL_ARCHETYPES {
                assert!(pairs.contains(&(a, b)), "missing adjacency {a:?} -> {b:?}");
            }
        }
        for g in ALL_GATEWAYS {
            for c in NESTABLE {
                assert!(depth1.contains(&(g, c)), "missing depth-1 {g:?}({c:?})");
            }
        }
        for g in XOR_GATEWAYS {
            for b in [
                Archetype::BoundaryInterrupting,
                Archetype::BoundaryNonInterrupting,
            ] {
                assert!(
                    depth1.contains(&(g, b)),
                    "missing XOR×Boundary nesting {g:?}({b:?})"
                );
            }
        }
        for g in ALL_GATEWAYS {
            for g2 in ALL_GATEWAYS {
                for c in NESTABLE {
                    assert!(
                        depth2.contains(&(g, g2, c)),
                        "missing depth-2 {g:?}({g2:?}({c:?}))"
                    );
                }
            }
        }
    }

    /// Cement-locks the encoder/grammar coupling: every covering shape,
    /// encoded to tape bytes and decoded back through the REAL grammar,
    /// reproduces itself exactly. A grammar change that breaks the
    /// encoding fails here by name, not silently in the seed corpus.
    #[test]
    fn covering_corpus_round_trips_through_the_tape() {
        for shape in covering_shapes() {
            let bytes = encode_shape(&shape);
            let mut tape = Tape::new(&bytes);
            let decoded = gen_shape(&mut tape);
            assert_eq!(
                format!("{decoded:?}"),
                format!("{shape:?}"),
                "tape round-trip diverged"
            );
        }
    }

    /// The guaranteed-structure half of the hybrid, run deterministically:
    /// every covering shape compiles (G-A over the ENUMERATED corpus, not
    /// a random sample) and steps clean under the full oracle set with
    /// LCG runtime tapes.
    #[test]
    fn covering_corpus_compiles_and_steps_clean() {
        let shapes = covering_shapes();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        runtime().block_on(async {
            for shape in &shapes {
                let generated = emit_process(shape);
                let store: Arc<dyn WorkflowStore> = Arc::new(MemoryStore::new());
                let engine = BpmnLiteEngine::new_with_runtime_context(
                    store,
                    TenantId::default(),
                    Arc::new(FuzzClock::new()),
                );
                if let Err(error) = engine.compile(&generated.xml).await {
                    panic!("G-A red (covering): {error}\nshape: {shape:?}\n{}", generated.xml);
                }
                let bytes: Vec<u8> = (0..96)
                    .map(|_| {
                        seed = seed
                            .wrapping_mul(6364136223846793005)
                            .wrapping_add(1442695040888963407);
                        (seed >> 33) as u8
                    })
                    .collect();
                let mut tape = Tape::new(&bytes);
                drive_shape(shape, &mut tape).await;
            }
        });
    }

    /// Seed writer (run via `cargo xtask fuzz seed`): each covering shape
    /// becomes a tape-prefix seed under seeds/engine_graph/, from which
    /// libFuzzer mutates the runtime-dynamics suffix. Pre-cleans stale
    /// cov-*.bin so an alphabet change never leaves orphaned seeds.
    #[test]
    #[ignore = "writes files; invoked by cargo xtask fuzz seed"]
    fn write_covering_seeds() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/engine_graph");
        std::fs::create_dir_all(&dir).expect("create seeds dir");
        for entry in std::fs::read_dir(&dir).expect("read seeds dir") {
            let path = entry.expect("dir entry").path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("cov-") && n.ends_with(".bin"))
            {
                std::fs::remove_file(&path).expect("remove stale seed");
            }
        }
        let shapes = covering_shapes();
        for (index, shape) in shapes.iter().enumerate() {
            let mut bytes = encode_shape(shape);
            // Runtime-dynamics suffix for the fuzzer to mutate from.
            bytes.extend(std::iter::repeat(2u8).take(24));
            std::fs::write(dir.join(format!("cov-{index:03}.bin")), &bytes)
                .expect("write seed");
        }
        println!("wrote {} covering seeds to {}", shapes.len(), dir.display());
    }
}

