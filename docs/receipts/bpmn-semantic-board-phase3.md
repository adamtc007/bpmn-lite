# Phase 3 semantic-board receipt

**Shared release:** `v0.1.6`, immutable revision
`fa51217ffd2218edea82c175e45ffa11d9eb7cf9`

## Cold dependency resolution

Executed from `/tmp` with an isolated `CARGO_HOME`, so the developer's
`~/.cargo/config.toml` patches were outside Cargo's configuration hierarchy:

```text
cargo check -p utterance-engine --no-default-features --locked
```

The check passed and compiled the following packages from the exact git source:

```text
utterance-engine
├── sem_os_ontology (dsl rev fa51217f)
│   ├── dsl_types (dsl rev fa51217f)
│   └── sem_os_types (dsl rev fa51217f)
└── sem_os_policy (dsl rev fa51217f)
    ├── sem_os_core (dsl rev fa51217f)
    │   ├── dsl-core (dsl rev fa51217f)
    │   ├── dsl_types (dsl rev fa51217f)
    │   └── sem_os_types (dsl rev fa51217f)
    └── sem_os_ontology (dsl rev fa51217f)
```

`bpmn-lite-server-designer` resolves the same graph through its
`utterance-engine` dependency. `Cargo.lock` records the full immutable git URL
and revision for all six shared packages; none is a `[[patch.unused]]` entry.

## Semantic receipts

- all 19 operations and 7 productions have exactly one exhaustive semantic
  contract;
- every contract contains typed arguments, governed phrases, examples and a
  nearest-neighbour negative contrast;
- unrepresentable candidates never enter a production board;
- same inputs reproduce the same board hash;
- revision, anchor, policy, semantic text, argument kind, phrase and contrast
  changes each move the board hash;
- mismatched anchor key/id pairs fail closed;
- an incomplete test registry fails the catalogue-coverage assertion.

## Gates

```text
designer-graph: 59 passed
utterance-engine: 39 unit + 1 inventory + 1 doc test passed
utterance-engine Clippy (--all-targets --no-default-features --no-deps -D warnings): passed
```

The identical Clippy command without `--no-deps` reaches unchanged dependency
lint debt in `bpmn-lite-compiler` (`match_like_matches_macro` and the existing
12-argument guard-lowering function). The Phase 3 package itself is warning
free; no `#[allow]` was added to conceal dependency debt.
