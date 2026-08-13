# H0 evidence — test topology inventory (R3 classification)

Covers all 40 Cargo integration-test targets (confirmed count: 48 raw `tests/*.rs`
files minus 8 non-target helper files pulled in via `mod`) and unit-test modules across
all 33 workspace members, per H0 work item 3 / Rule R3.

## Part 1 — Integration-test targets (40/40 classified)

| File | Subject crate | Imported workspace crates | Class | Notes |
| --- | --- | --- | --- | --- |
| bpmn-lite-authoring/tests/importer_compatibility_tests.rs | bpmn-lite-authoring | bpmn_lite_compiler | Inter-crate contract | regular dep |
| bpmn-lite-bus-handler/tests/graph_authored_plan_instantiation.rs | bpmn-lite-bus-handler | bpmn_lite_compiler, bpmn_lite_store, bpmn_lite_engine, dsl_bus_protocol, dsl_bus_server | **Multi-crate application** | → `xtask/tests/bus_graph_instantiation_vertical.rs` |
| bpmn-lite-bus-handler/tests/sage_macro_assembly_tests.rs | bpmn-lite-bus-handler | + bpmn_lite_store_postgres, bpmn_lite_types | **Multi-crate application** | plan's known candidate; → `xtask/tests/bus_postgres_vertical.rs` |
| bpmn-lite-engine/tests/a11_ffi_end_to_end.rs | bpmn-lite-engine | store, types, vm, dmn_lite_bridge, dmn_lite_compiler, dmn_lite_parser, ffi_catalogue, ffi_dispatcher, ffi_types | **Multi-crate application** | plan's known candidate; → `xtask/tests/ffi_vertical.rs` |
| bpmn-lite-engine/tests/correlation_content.rs | bpmn-lite-engine | compiler, store, types, vm | Inter-crate contract | all regular deps |
| bpmn-lite-engine/tests/differential_bpmn.rs | bpmn-lite-engine | compiler, store, types, vm | Inter-crate contract | |
| bpmn-lite-engine/tests/send_task.rs | bpmn-lite-engine | compiler, store, types, vm | Inter-crate contract | |
| bpmn-lite-ffi-http/tests/response_decode.rs | bpmn-lite-ffi-http | ffi_types | Inter-crate contract | regular dep |
| bpmn-lite-server-runner/tests/integration.rs | bpmn-lite-server-runner | engine, store, types, vm | **Multi-crate application** | runner is the app crate; → `xtask/tests/runner_application.rs` |
| bpmn-lite-server-runner/tests/orch_flags_array_limits.rs | bpmn-lite-server-runner | engine, ffi_grpc, ffi_http, store, types, dmn_lite_bridge, ffi_catalogue | **Multi-crate application** | full gRPC service stack; → `xtask/tests/runner_application.rs` |
| dmn-lite-analysis/tests/{config,cost,determinism,gap,overlap,sa001,unreachable}_tests.rs (7 files) | dmn-lite-analysis | dmn_lite_types + (compiler/parser via `tests/common/mod.rs`, minimum-construction) | Inter-crate contract | not flagged — genuine minimum-construction pattern |
| dmn-lite-analysis/tests/fixture_tests.rs | dmn-lite-analysis | compiler, parser, types | Inter-crate contract | |
| dmn-lite-compiler/tests/catalogue_tests.rs | dmn-lite-compiler | (none) | Intra-crate-equivalent | |
| dmn-lite-compiler/tests/{compile,emit,hash,verify}_tests.rs (4 files) | dmn-lite-compiler | parser, types | Inter-crate contract | regular deps |
| dmn-lite-compiler/tests/end_to_end.rs | dmn-lite-compiler | engine, parser, types, **analysis** | **Multi-crate application** | plan's known candidate, wider than stated (also touches analysis); → `xtask/tests/dmn_vertical.rs` |
| dmn-lite-engine/tests/differential_runner.rs (+ 7-file `differential/` subtree) | dmn-lite-engine | types, compiler, parser | Inter-crate contract (borderline — flag for explicit ratification, unusually large fixture surface) | |
| dmn-lite-engine/tests/{reference,vm,vm_trace}_tests.rs (3 files) | dmn-lite-engine | compiler, parser, types | Inter-crate contract | compiler/parser are dev-deps used only for fixture construction |
| dmn-lite-parser/tests/{arity,diagnostics,happy_path,lexer_edges,out_of_profile,recovery,round_trip,spans}.rs (8 files) | dmn-lite-parser | (none) | Intra-crate-equivalent | |
| utterance-engine/tests/candidate_coverage_inventory.rs | utterance-engine | designer_graph | Inter-crate contract | |
| utterance-engine/tests/evaluator_serving_packet_identity.rs | utterance-engine | (none) | Intra-crate-equivalent | |
| utterance-engine/tests/gameboard_disposition.rs | utterance-engine | bpmn_lite_compiler, designer_graph | Inter-crate contract | |
| utterance-engine/tests/shared_contract_compat.rs | utterance-engine | (external sem_os_* deps only) | Inter-crate contract (trivial) | not a workspace-sibling concern |

**Corrected totals: 10 intra-crate, 24 inter-crate contract, 6 multi-crate application
(10+24+6=40 ✓).** Multi-crate application set: bus-handler ×2, engine ×1,
server-runner ×2, dmn-lite-compiler ×1.

## Part 2 — Unit-test modules reaching across to a sibling workspace crate

Most cross-crate unit-test imports use the crate's own **regular** (non-dev)
dependencies and are unremarkable. Three findings warrant explicit peer-review
attention because the reach looks like test-setup convenience rather than a genuine
contract dependency:

| Crate | File | Sibling reached | Why flagged |
| --- | --- | --- | --- |
| **bpmn-lite-engine** | `src/tests.rs` | **bpmn_lite_authoring** (dev-dep) | Engine's own Cargo.toml documents a locked Phase-0 decision that it "does NOT depend on `bpmn-lite-authoring`" — yet its unit tests call `bpmn_lite_authoring::parse_workflow_yaml`/`compile_program_from_dto` directly. This contradicts the crate's own stated boundary; same failure class the plan is hunting for. **New finding, not in the plan's original evidence.** |
| **bpmn-lite-store-postgres** | `src/store_postgres.rs` `mod tests` | compiler, engine, kernel, store, types, vm, dsl_bus_storage, ffi_catalogue, ffi_dispatcher (7 dev-deps) | Confirms plan §1.3's known candidate — widest single-file cross-section in the workspace, mixing SQL-store contract tests with engine/compiler/kernel/FFI/bus-storage scenarios. |
| **bpmn-lite-server-designer** | `src/rest.rs` | 10 sibling crates (types, designer-graph, utterance-engine, authoring, compiler, engine, store, store-postgres, dmn-lite-parser, dsl-manifest) | H3-scoped: mirrors the plan's own objective #2 ("application composition... not in a capability crate's unit test suite"), since server-designer's unit tests reach nearly the whole application dependency graph. |
| **ffi-dispatcher** | `src/lib.rs` | bpmn_lite_types (dev-dep) | Scope-creep, not app-assembly: ffi-dispatcher's own Cargo.toml describes it as routing on `owner_type` with no stated relationship to bpmn-lite's domain types. Candidate for H1 dev-dep removal — build fixtures from `ffi_types`/`ffi_catalogue` only. |
| designer-graph | `ops.rs` | bpmn_lite_engine | Verify at H1 whether this is a regular or dev-only dependency before ruling. |

Crates confirmed clean (unit tests import nothing but self + crates.io test libs):
bpmn-lite-vm, ffi-types, bpmn-lite-ffi-grpc, bpmn-lite-bus-handler,
dmn-lite-bus-handler, dmn-lite-manifest-export, dsl-bus-client, dsl-bus-protocol,
dsl-bus-server, dsl-bus-storage, dsl-manifest, bpmn-lite-types (after excluding self).

## Part 3 — Workspace dev-dependencies flagged as application-assembly-only (H0 work item 3)

| Crate | Flagged dev-dependency | Sole use | Verdict |
| --- | --- | --- | --- |
| bpmn-lite-bus-handler | bpmn-lite-store-postgres | `sage_macro_assembly_tests.rs` | App-assembly only — flag for H1 removal after test moves to xtask |
| bpmn-lite-engine | bpmn-lite-authoring | `src/tests.rs` | Flag — also violates the crate's own documented boundary (see Part 2) |
| bpmn-lite-engine | dmn-lite-bridge, dmn-lite-compiler, dmn-lite-parser, ffi-catalogue, ffi-types | `a11_ffi_end_to_end.rs` | App-assembly only — matches plan's known candidate |
| bpmn-lite-store-postgres | bpmn-lite-compiler, bpmn-lite-engine, bpmn-lite-kernel, bpmn-lite-vm, bpmn-lite-authoring, dsl-bus-storage, ffi-dispatcher | `store_postgres.rs` `mod tests` | App-assembly only — matches plan's known candidate |
| dmn-lite-compiler | dmn-lite-engine, dmn-lite-analysis | `end_to_end.rs` | App-assembly only — matches plan's known candidate, wider than stated |
| ffi-dispatcher | bpmn-lite-types | `src/lib.rs` unit tests | Scope-creep, not app-assembly — narrower fix (drop dep) recommended over an xtask move |
| bpmn-lite-bus-handler | (also) bpmn-lite-store-postgres for `graph_authored_plan_instantiation.rs` | — | Same crate as row 1; both bus-handler multi-crate tests share this dep |

**Not flagged** (genuine minimum-construction inter-crate contract use, not
app-assembly): dmn-lite-analysis's `dmn-lite-compiler`/`dmn-lite-parser` dev-deps;
dmn-lite-bridge's same pair.

## Summary

40/40 integration-test targets classified; zero unclassified. 6 confirmed multi-crate
application targets map to the plan's proposed `xtask/tests/*` destinations (one of the
five plan-cited candidates — `dmn-lite-compiler/tests/end_to_end.rs` — is wider in
scope than originally described, also touching `dmn-lite-analysis`). One new
cross-crate unit-test leak (`bpmn-lite-engine/src/tests.rs` → `bpmn-lite-authoring`)
was found that isn't in the plan's original evidence and should be added to the H1
migration list.
