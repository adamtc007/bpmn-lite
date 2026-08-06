# BPMN mapper Phase 9 receipt

**Date:** 4 August 2026
**Toolchains:** Rust 1.95; cargo-fuzz 0.13.2 on nightly
**Posture:** mapper gates green; workspace baseline exceptions recorded below

## Property and boundary tests

- `utterance-engine` adds six property families covering canonical context
  injectivity, board permutation identity, phrase collisions, finite total
  ordering, workbook refusal/round-trip safety and arbitrary-text parser/pair
  serialization safety.
- `bpmn-lite-server-designer` adds two property families proving arbitrary text
  cannot panic deterministic binding extraction and arbitrary typed answers
  cannot bypass an identifier slot contract.
- The named-feature workspace suite passed with 59 `utterance-engine` tests and
  four explicit model-dependent ignores; the designer crate's 52 tests passed.
- All-feature unit-state construction is deliberately model-free. Real model
  evaluation must opt in with `BPMN_LITE_TEST_ENABLE_MODELS`, preventing an
  ordinary CI test from downloading weights or changing its evidence producer.

## Fuzz targets and scheduling

Four bounded targets were added:

| project | target | input cap | seeded runs | peak RSS |
|---|---|---:|---:|---:|
| `utterance-engine` | `semantic_board_decode` | 64 KiB | 1,000 | 60 MiB |
| `utterance-engine` | `phrase_index` | 16 KiB | 1,000 | 490 MiB |
| `utterance-engine` | `workbook_transition` | 4 KiB | 1,000 | 68 MiB |
| `bpmn-lite-server-designer` | `bpmn_binding_extract` | 16 KiB | 1,000 | 50 MiB |

All four seeded runs completed without a crash. Discovery is now 19 targets
across seven fuzz projects. The nightly workflow already executes one target
per matrix job, so the existing 1,200-second target budget is unchanged; four
jobs add 80 target-minutes without extending the per-target or critical-path
budget.

Every fuzz project has a cargo-fuzz-resolved lockfile. The runner now invokes
cargo-fuzz from a neutral external directory and compares `Cargo.lock` before
and after every target. A lock rewrite fails the run. A repeat regression replay
left the aggregate lock diff byte-identical and executed the committed
F8-COMPILER-001 XML case (`execs: 2`, `cov: 1601`).

## CI and performance

Production gates now name the mapper contract/property tests, semantic
coverage, serving/corpus serializer identity, hermetic old-bundle refusal,
proposal tests and the existing discovered regression replay. A release-mode
performance receipt is generated and uploaded without inventing a threshold.

Native measurements are recorded in
`docs/receipts/bpmn-mapper-performance-2026-08-04.md`. The legal 15-candidate
board measured p95 10.459 microseconds for governed exact evidence, 11.500 for
all candidate serializations and 73.459 for all bounded candidate pairs.
Candle cold/warm/full-board measurements, memory and full-versus-K=12 accuracy
remain explicitly unavailable because there is no admitted v3 bundle or
independent evaluation evidence.

## Gate result

Green:

- layering guard;
- Q9 guard self-test and live repository check;
- named-feature workspace build;
- named-feature workspace tests, serially, including available PostgreSQL
  tests;
- named-feature workspace documentation;
- committed fuzz regression replay, with lock integrity;
- changed-source rustfmt checks.

Known baseline exceptions, outside the mapper change set:

- workspace-wide `cargo fmt --all -- --check` reports pre-existing DMN source
  formatting drift; every Rust file changed by this phase passes Rust 1.95
  rustfmt with child traversal disabled;
- workspace Clippy with `-D warnings` stops on two pre-existing collapsible-if
  warnings in `bpmn-lite-kernel` and a match-like-matches plus
  too-many-arguments warning in `bpmn-lite-compiler`;
- rustdoc completes but reports existing private/broken-link warnings in
  unrelated crates.

No warning suppression or unrelated source formatting was folded into this
phase. Those exceptions prevent representing the entire historical workspace
gate as green, but the mapper-specific Phase 9 implementation and gates are
green.
