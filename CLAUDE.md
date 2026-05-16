# CLAUDE.md — bpmn-lite

> **Last reviewed:** 2026-05-16
> **Repo:** github.com/adamtc007/bpmn-lite
> **Status:** A3–A11 complete (FFI infrastructure + first end-to-end proof)
> **Related:** github.com/adamtc007/dmn-lite (decision vocabulary); github.com/adamtc007/ob-poc (BNY onboarding platform — consumes via gRPC)
> **V&S:** `ob-poc/todo/dmn-lite/bpmn-dmn-lite-vs-v1_1.md`

bpmn-lite is the **process vocabulary** of the compilation-and-execution kernel described in V&S v1.1. It compiles BPMN 2.0 XML to verified bytecode, executes process instances on a fiber-based stack VM, and dispatches foreign function calls to registered execution owners via the FFI catalogue. It is deployed as a standalone gRPC service; ob-poc calls it over the wire.

---

## Quick Start

```bash
# Build everything
cargo build

# Run tests (excludes Postgres integration tests)
cargo test --workspace --exclude bpmn-lite-store-postgres

# Run Postgres integration tests (requires DATABASE_URL)
BPMN_LITE_TEST_DATABASE_URL="postgresql:///data_designer" \
  cargo test -p bpmn-lite-store-postgres -- --ignored

# Start gRPC server (port 50051)
cargo x bpmn-lite start

# With Postgres store
cargo x bpmn-lite start --database-url postgresql:///data_designer

# Smoke test (spawns server, runs fixtures, tears down)
cargo run -p xtask -- smoke --spawn-server

# Stress test
cargo run -p xtask -- stress --spawn-server --instances 300 --workers 16
```

---

## Workspace Structure

```
bpmn-lite/
├── Cargo.toml                    workspace root (12 members)
├── bpmn-lite-types/              IDs, value types, Instr ISA, ProcessInstance, Fiber,
│                                 CompiledProgram (+ ffi_bindings: FfiTaskDecl, DataObjectDecl,
│                                 BindingSource/Target), RuntimeEvent
├── bpmn-lite-compiler/           BPMN XML → IR → bytecode lowering + verification.
│                                 Parser: serviceTask (external-job + FFI), dataObject,
│                                 gateway conditions. verify_ffi_schemas (A6).
├── bpmn-lite-vm/                 Fiber-based stack machine. Opcodes include ExecNative
│                                 (external-job park) and ExecFfi (in-process FFI signal).
│                                 json_path module (A7): read/write/canonicalise domain_payload.
├── bpmn-lite-engine/             Orchestration facade: start/run/complete/fail/signal/cancel.
│                                 handle_ffi_dispatch (A8): ExecFfi → FfiDispatcher → output binding.
├── bpmn-lite-store/              ProcessStore trait + MemoryStore.
├── bpmn-lite-store-postgres/     PostgresProcessStore + migrations 001–024.
│                                 Migrations 023–024: ffi_template, ffi_invocation_record.
│                                 PostgresFfiTemplateStore.
├── bpmn-lite-authoring/          YAML/DTO authoring pipeline → CompiledProgram.
│                                 Standalone from the engine; no mutual dep.
├── bpmn-lite-server/             tonic gRPC server (port 50051). Activate/complete/fail jobs,
│                                 start/cancel/inspect instances, event fanout.
├── ffi-types/                    Vocabulary-neutral FFI protocol types.
│                                 FfiTemplate, FieldSchema, SchemaKind, Idempotency,
│                                 FfiCall, FfiResult, ForeignFunctionInvocationRecord,
│                                 FfiExecutionOwner trait, FfiCatalogueSnapshot trait,
│                                 GLOBAL_TENANT_ID, compute_template_id (BLAKE3).
├── ffi-catalogue/                FfiTemplateStore trait, MemoryFfiTemplateStore, FfiCatalogue
│                                 (cache-front). CatalogueSnapshot implements FfiCatalogueSnapshot.
├── ffi-dispatcher/               FfiDispatcher: owner registry + ExecFfi routing.
│                                 validate_coverage() for startup checks.
└── xtask/                        Smoke/stress harness CLI.
```

---

## Architecture

### Two dispatch paths (permanent)

| Opcode | Dispatch | Use case |
|--------|----------|----------|
| `Instr::ExecNative` | External-job queue → gRPC worker polls | Human approval, async callbacks, long-running external work |
| `Instr::ExecFfi` | In-process FfiDispatcher (A8) | Decisions, HTTP calls, gRPC calls — millisecond latency |

The compiler emits `ExecNative` for `<zeebe:taskDefinition type="...">` and `ExecFfi` for `<bpmn:taskDefinition implementation="<64hex>">`.

### BPMN annotation grammar (FFI)

```xml
<bpmn:dataObject id="do_score">
  <bpmn:extensionElements>
    <bpmn:dataType primitive="integer" role="input"/>
  </bpmn:extensionElements>
</bpmn:dataObject>

<bpmn:serviceTask id="CheckScore">
  <bpmn:extensionElements>
    <bpmn:taskDefinition implementation="<64-char hex BLAKE3 template_id>">
      <bpmn:input  target="score"    expression="${do_score}"/>
      <bpmn:output target="do_eligible" source="eligible"/>
    </bpmn:taskDefinition>
  </bpmn:extensionElements>
</bpmn:serviceTask>
```

Data object storage assignment (lowering):
- `bool`, `integer` → `DataObjectStorage::Flag(FlagKey)` — fits in `bpmn_lite_types::Value`
- `float`, `string`, `SemOsDomain` → `DataObjectStorage::DomainPayload(path)` — canonical JSON via `json_path`

### FFI call lifecycle

```
ExecFfi opcode hit
  → VM: TickOutcome::ExecFfi { template_id, pc, invocation_id }
  → Engine: FfiDispatcher::dispatch(FfiCall)
      → build input_payload from CompiledFfiInputBinding (FlagRef/DomainPayloadRef/Literal)
      → write RuntimeEvent::FfiInvocationPending
      → owner.invoke(call).await → FfiResult
      → apply outputs (FlagWrite or DomainPayloadWrite via json_path)
      → write RuntimeEvent::FfiInvocationCompleted
      → advance fiber.pc
```

### Three outcomes (A2 §8)

| FfiResult | Effect |
|-----------|--------|
| `Success { output_payload, .. }` | Apply output bindings, advance pc, continue fiber |
| `NoMatch { .. }` | Skip bindings, advance pc, continue fiber |
| `Incident { error_class, .. }` | Route via error_route_map (BusinessRejection) or create Incident, park fiber |

---

## A-Phase Implementation Status

| Phase | Delta | Description | Status |
|-------|-------|-------------|--------|
| A1 | — | FFI design decisions | ✅ |
| A2 | — | FFI Foreign Function Contract spec | ✅ |
| A3 | Δ8 | `flag_symbol_table` in CompiledProgram | ✅ |
| A4 | Δ3 | ffi-types / ffi-catalogue / ffi-dispatcher crates | ✅ |
| A5 | Δ6+Δ2p | BPMN data-object parser + FFI annotation parser + lowering | ✅ |
| A6 | Δ2v | `verify_ffi_schemas` compile-time schema checker | ✅ |
| A7 | Δ9 | `json_path` module (read/write/canonicalise domain_payload) | ✅ |
| A8 | Δ1 | Engine in-process FFI dispatch (`handle_ffi_dispatch`) | ✅ |
| A9 | Δ4 | FFI output binding — landed inside A8 | ✅ |
| A10 | — | `dmn-lite-bridge` crate (in dmn-lite repo) | ✅ |
| A11 | — | First end-to-end test: BPMN → ExecFfi → dmn-lite → result | ✅ |
| A12 | Δ7 | Publish compiled BPMN process as FFI template | ⬜ |
| A13 | — | HTTP FFI execution owner | ⬜ |
| A14 | — | gRPC FFI execution owner | ⬜ |
| A15 | — | bpmn-lite static analysis (dead branches, FFI signature coverage) | ⬜ |
| A16 | — | Multi-tenant Postgres (RLS enforcement) | ⬜ |
| A17 | — | Hot restart of in-flight FFI calls | ⬜ |
| A18 | — | Sub-process invocation (bpmn-lite calls bpmn-lite as sub-process) | ⬜ |

Design documents: `ob-poc/todo/bpmn-lite/` (a0–a2 notes) and `ob-poc/todo/dmn-lite/` (v&s v1.1, arch commitments v0.3).

---

## Key Invariants

- **No runtime expression interpretation.** No FEEL, no JUEL, no embedded scripts. Every artifact is compiled before execution.
- **ExecFfi never reaches the VM directly.** The engine intercepts `TickOutcome::ExecFfi` and handles dispatch. The VM arm returns this outcome; the engine loop catches it.
- **Bounded computation.** `estimate_instr_count` enforces a cost ceiling at compile time.
- **DataObject nodes are structural.** They have no sequence-flow edges and zero bytecode. The verifier excludes them from the reachability check.
- **flag_symbol_table is preserved.** `lower()` inverts its intern map and stores it in `CompiledProgram.flag_symbol_table` (A3). The FFI binding layer uses it to resolve data-object names to FlagKeys.
- **Two versions of ffi-types problem.** The workspace `[patch]` section redirects any git dep on `github.com/adamtc007/bpmn-lite` to the local path. This keeps exactly one copy of `ffi-types` in the build graph. Without it, `DmnLiteOwner: FfiExecutionOwner` fails to unify.

---

## Monorepo Development (ob-poc)

When working inside the ob-poc monorepo with both bpmn-lite and dmn-lite as sibling directories:

1. The `[patch]` block in `bpmn-lite/Cargo.toml` makes `ffi-types` resolve to the local path even when `dmn-lite-bridge` declares a git dep.
2. Build from `bpmn-lite/` workspace root, not from `ob-poc/` root — the patch is scoped to this workspace.
3. `ob-poc` does not track `bpmn-lite/` or `dmn-lite/` as git submodules; they are independent repos inside the same local directory.

---

## Test Counts (2026-05-16)

| Crate | Tests |
|-------|-------|
| bpmn-lite-types | 0 |
| bpmn-lite-compiler | 32 |
| bpmn-lite-vm | 41 |
| bpmn-lite-engine (unit) | 45 |
| bpmn-lite-engine (a11 integration) | 3 |
| ffi-types | 15 |
| ffi-catalogue | 8 |
| ffi-dispatcher | 12 |
| **Total** | **224** |
