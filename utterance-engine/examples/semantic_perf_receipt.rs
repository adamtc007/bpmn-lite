//! Hermetic mapper microbenchmark receipt.
//!
//! This deliberately excludes Candle: Phase 6 has no admitted v3 bundle, so
//! manufacturing model timings would be false provenance. The output names
//! every unavailable measurement instead of silently omitting it.

use std::hint::black_box;
use std::time::{Duration, Instant};

use utterance_engine::board::PolicyFilter;
use utterance_engine::bpmn_board::build_bpmn_semantic_board;
use utterance_engine::exact::{governed_exact, serialize_candidate};
use utterance_engine::pair::{serialize_candidate_pair, PairTokenBudget};

const SAMPLES: usize = 2_000;

fn measure(mut operation: impl FnMut()) -> (f64, f64) {
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        operation();
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    (
        micros(samples[SAMPLES / 2]),
        micros(samples[SAMPLES * 95 / 100]),
    )
}

fn micros(duration: Duration) -> f64 {
    duration.as_nanos() as f64 / 1_000.0
}

fn main() {
    let state = utterance_engine::fixtures::enumeration_classes()
        .expect("enumeration fixtures")
        .into_iter()
        .find(|state| state.class_id == "mid_sequence_task")
        .expect("mid-sequence fixture");
    let anchor = state.anchor_key.zip(state.anchor_id);
    let board = build_bpmn_semantic_board(
        &state.dag,
        anchor,
        "perf-revision",
        &PolicyFilter::default(),
    )
    .expect("semantic board");
    let context = utterance_engine::context::minimal("pack.perf", "perf-revision");

    println!("# BPMN mapper performance receipt");
    println!();
    println!("- samples per native measurement: {SAMPLES}");
    println!("- architecture: {}", std::env::consts::ARCH);
    println!("- OS: {}", std::env::consts::OS);
    println!(
        "- legal mid-sequence board size: {}",
        board.candidates.len()
    );
    println!();
    println!("| measurement | board size | p50 µs | p95 µs |");
    println!("|---|---:|---:|---:|");

    for requested_size in [4_usize, 8, 12] {
        let mut policy = PolicyFilter::default();
        let keep_non_abstention = requested_size - 1;
        for candidate in board
            .candidates
            .iter()
            .take(board.candidates.len() - 1)
            .skip(keep_non_abstention)
        {
            policy
                .denied
                .insert(candidate.canonical_id.as_str().to_string());
        }
        let (p50, p95) = measure(|| {
            black_box(
                build_bpmn_semantic_board(&state.dag, anchor, "perf-revision", &policy)
                    .expect("bounded board build"),
            );
        });
        println!("| semantic board construction | {requested_size} | {p50:.3} | {p95:.3} |");
    }

    let (p50, p95) = measure(|| {
        black_box(governed_exact(&board, "attach an interrupting guard"));
    });
    println!(
        "| governed exact lane | {} | {p50:.3} | {p95:.3} |",
        board.candidates.len()
    );

    let (p50, p95) = measure(|| {
        for candidate in &board.candidates {
            black_box(serialize_candidate(candidate));
        }
    });
    println!(
        "| serialize all semantic candidates | {} | {p50:.3} | {p95:.3} |",
        board.candidates.len()
    );

    let (p50, p95) = measure(|| {
        for candidate in &board.candidates {
            black_box(
                serialize_candidate_pair(
                    "attach an interrupting guard after two days",
                    &context,
                    candidate,
                    PairTokenBudget::default(),
                )
                .expect("pair serialization"),
            );
        }
    });
    println!(
        "| serialize all bounded candidate pairs | {} | {p50:.3} | {p95:.3} |",
        board.candidates.len()
    );

    println!();
    println!("## Explicitly unavailable");
    println!();
    println!("- Candle batch 4/8/12/20/26: no admitted v3 bundle; Phase 6 remains blocked.");
    println!("- Cold bundle load and warm inference: no admitted v3 bundle.");
    println!("- Full-board versus K=12 accuracy: no admitted v3 evaluation evidence.");
    println!("- Board sizes 20/26: no reviewed position exposes that many legal actions; no synthetic authority board was fabricated.");
    println!("- Peak RSS/per-request allocation: no allocator or portable process-memory probe is installed; fuzz RSS remains a separate sanitizer measurement.");
    println!();
    println!(
        "No regression threshold is asserted until an owner ratifies a stable runner and baseline."
    );
}
