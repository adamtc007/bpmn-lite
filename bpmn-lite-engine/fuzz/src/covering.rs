//! F7 — covering-array topology corpus (ratified 2026-07-26): constrain
//! the cartesian explosion of valid execution graphs by covering the
//! LOCAL logic alphabet deterministically instead of sampling whole DAGs.
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
//!   factor 2  switch outcome      — XOR both routes (taken/untaken), MI
//!             collection lengths {0, 1, 4} (zero-match / single / max);
//!   factor 3  nesting depth       — every (gateway, content) at depth 1
//!             and every (gateway, gateway′, content) at depth 2; the
//!             NON-LOCAL factor pure pairwise adjacency would miss (the
//!             depth family is where the plateau broke: 13,591 → 14,951).
//!
//! Division of labour: enumeration guarantees the STRUCTURE coverage;
//! coverage-guided libFuzzer keeps ownership of the DYNAMICS (completion
//! order, timer jumps, fault injection) — the covering shapes are written
//! as tape seeds so the fuzzer mutates runtime behaviour from guaranteed
//! structural starting points, and `covering_corpus_compiles_and_steps_
//! clean` runs the whole corpus deterministically in CI regardless.
//!
//! Recorded limits (no silent gaps):
//! - Boundary participates in adjacency/singles only (top level): nesting
//!   it inside an AND branch is illegal SESE (cemented in the grammar
//!   tests), and the tape grammar cannot express Boundary-in-XOR even
//!   though it compiles — widening the grammar there is a separate step.
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
    XorTaken,
    XorUntaken,
    BoundaryInterrupting,
    BoundaryNonInterrupting,
    Mi0,
    Mi1,
    Mi4,
}

pub const ALL_ARCHETYPES: [Archetype; 10] = [
    Archetype::Task,
    Archetype::And2,
    Archetype::And3,
    Archetype::XorTaken,
    Archetype::XorUntaken,
    Archetype::BoundaryInterrupting,
    Archetype::BoundaryNonInterrupting,
    Archetype::Mi0,
    Archetype::Mi1,
    Archetype::Mi4,
];

/// Gateway wrappers for the nesting factors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gateway {
    And,
    XorTaken,
    XorUntaken,
}

pub const ALL_GATEWAYS: [Gateway; 3] = [Gateway::And, Gateway::XorTaken, Gateway::XorUntaken];

/// Archetypes legal INSIDE a gateway branch (Boundary excluded — see
/// module header).
pub const NESTABLE: [Archetype; 8] = [
    Archetype::Task,
    Archetype::And2,
    Archetype::And3,
    Archetype::XorTaken,
    Archetype::XorUntaken,
    Archetype::Mi0,
    Archetype::Mi1,
    Archetype::Mi4,
];

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
            Archetype::XorTaken => Block::Xor {
                take_guarded: true,
                guarded: vec![Block::Task],
            },
            Archetype::XorUntaken => Block::Xor {
                take_guarded: false,
                guarded: vec![Block::Task],
            },
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
            } if matches!(guarded[..], [Block::Task]) => Some(if *take_guarded {
                Archetype::XorTaken
            } else {
                Archetype::XorUntaken
            }),
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
    /// Wrap a region in this gateway. AND pairs the region with a plain
    /// second branch so the join is a real synchronizer.
    pub fn wrap(self, region: Vec<Block>) -> Block {
        match self {
            Gateway::And => Block::And {
                branches: vec![region, vec![Block::Task]],
            },
            Gateway::XorTaken => Block::Xor {
                take_guarded: true,
                guarded: region,
            },
            Gateway::XorUntaken => Block::Xor {
                take_guarded: false,
                guarded: region,
            },
        }
    }

    /// Classify the outermost gateway of a block and return its nested
    /// region (for AND: the FIRST branch, matching `wrap`).
    pub fn unwrap(block: &Block) -> Option<(Gateway, &[Block])> {
        match block {
            Block::And { branches } if branches.len() == 2 => {
                Some((Gateway::And, &branches[0]))
            }
            Block::Xor {
                take_guarded,
                guarded,
            } => Some((
                if *take_guarded {
                    Gateway::XorTaken
                } else {
                    Gateway::XorUntaken
                },
                guarded,
            )),
            _ => None,
        }
    }
}

/// The covering corpus:
///   singles   — every archetype alone (10);
///   pairs     — every ORDERED archetype adjacency [A, B] (100);
///   depth 1   — every gateway wrapping every nestable content (24);
///   depth 2   — every gateway∘gateway′ wrapping every nestable content
///               (72, minus the And2-ambiguity dupes noted below).
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
        encode_block(block, crate::MAX_DEPTH, &mut budget, &mut bytes);
    }
    bytes
}

fn encode_block(block: &Block, depth: u8, budget: &mut u32, bytes: &mut Vec<u8>) {
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
                encode_region(branch, depth - 1, budget, bytes);
            }
        }
        Block::Xor {
            take_guarded,
            guarded,
        } => {
            assert!(depth > 0, "Xor below MAX_DEPTH nesting");
            bytes.push(5);
            bytes.push(u8::from(*take_guarded)); // bool = b & 1
            encode_region(guarded, depth - 1, budget, bytes);
        }
        Block::Boundary { interrupting } => {
            assert!(
                depth == crate::MAX_DEPTH,
                "grammar emits Boundary at top level only"
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

fn encode_region(blocks: &[Block], depth: u8, budget: &mut u32, bytes: &mut Vec<u8>) {
    assert!(
        (1..=3).contains(&blocks.len()),
        "nested region must be 1-3 blocks"
    );
    bytes.push(blocks.len() as u8 - 1);
    for block in blocks {
        encode_block(block, depth, budget, bytes);
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
    /// libFuzzer mutates the runtime-dynamics suffix.
    #[test]
    #[ignore = "writes files; invoked by cargo xtask fuzz seed"]
    fn write_covering_seeds() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("seeds/engine_graph");
        std::fs::create_dir_all(&dir).expect("create seeds dir");
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
