# CLAUDE.md — bpmn-lite

> **Last reviewed:** 2026-05-17
> **Repo:** github.com/adamtc007/bpmn-lite
> **Status:** A1–A19 complete; B0–B9 + B11–B12 complete; 597 tests passing
> **Related:** github.com/adamtc007/ob-poc (BNY onboarding platform — consumes via gRPC)
> **V&S:** `ob-poc/todo/dmn-lite/bpmn-dmn-lite-vs-v1_2.md`
> **Arch commitments:** `ob-poc/todo/dmn-lite/architecture-commitments-v0_6.md`

bpmn-lite is the **compilation-and-execution kernel** described in V&S v1.1. It currently ships two compiled vocabularies in a single workspace:

- **bpmn-lite** — process vocabulary: BPMN 2.0 XML → verified bytecode → fiber-based stack VM, exposed as a gRPC service.
- **dmn-lite** — decision vocabulary: s-expression DSL → verified bytecode → stack VM, registered into the FFI catalogue via `dmn-lite-bridge`.

bpmn-lite is deployed as a standalone gRPC service (port 50051); ob-poc calls it over the wire. dmn-lite decisions are invoked in-process from bpmn-lite via `Instr::ExecFfi`.

---

## Quick Start

```bash
# Build everything (single unified workspace)
cargo build --workspace

# Run all tests (excludes Postgres integration tests)
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
bpmn-lite/                          edition 2024, resolver "3", rust 1.95
├── Cargo.toml                      workspace root (18 members)
│
│  ── BPMN-Lite (process vocabulary) ──
├── bpmn-lite-types/                IDs, value types, Instr ISA, ProcessInstance, Fiber,
│                                   CompiledProgram (+ ffi_bindings: FfiTaskDecl, DataObjectDecl,
│                                   BindingSource/Target), RuntimeEvent
├── bpmn-lite-compiler/             BPMN XML → IR → bytecode lowering + verification.
│                                   Parser: serviceTask (external-job + FFI), dataObject,
│                                   gateway conditions. verify_ffi_schemas (A6).
├── bpmn-lite-vm/                   Fiber-based stack machine. Opcodes include ExecNative
│                                   (external-job park) and ExecFfi (in-process FFI signal).
│                                   json_path module (A7): read/write/canonicalise domain_payload.
├── bpmn-lite-engine/               Orchestration facade: start/run/complete/fail/signal/cancel.
│                                   handle_ffi_dispatch (A8): ExecFfi → FfiDispatcher → output binding.
├── bpmn-lite-store/                ProcessStore trait + MemoryStore.
├── bpmn-lite-store-postgres/       PostgresProcessStore + migrations 001–024.
│                                   Migrations 023–024: ffi_template, ffi_invocation_record.
│                                   PostgresFfiTemplateStore.
├── bpmn-lite-authoring/            YAML/DTO authoring pipeline → CompiledProgram.
│                                   Standalone from the engine; no mutual dep.
├── bpmn-lite-server/               tonic gRPC server (port 50051). Activate/complete/fail jobs,
│                                   start/cancel/inspect instances, event fanout.
│
│  ── FFI infrastructure (vocabulary-neutral) ──
├── ffi-types/                      FfiTemplate, FieldSchema, SchemaKind, Idempotency,
│                                   FfiCall, FfiResult, ForeignFunctionInvocationRecord,
│                                   FfiExecutionOwner trait, FfiCatalogueSnapshot trait,
│                                   GLOBAL_TENANT_ID, compute_template_id (BLAKE3).
├── ffi-catalogue/                  FfiTemplateStore trait, MemoryFfiTemplateStore, FfiCatalogue
│                                   (cache-front). CatalogueSnapshot implements FfiCatalogueSnapshot.
├── ffi-dispatcher/                 FfiDispatcher: owner registry + ExecFfi routing.
│                                   validate_coverage() for startup checks.
│
│  ── DMN-Lite (decision vocabulary, consolidated B0) ──
├── dmn-lite-types/                 IDs (DomainId, ValueId, FieldId, RuleId, SnapshotId),
│                                   IR types, CompiledDecision, VerifiedDecision, ArtifactHash,
│                                   AnalysisReport, EvalError, TypedInputContext.
├── dmn-lite-parser/                S-expression DSL → AST (Source). Multi-error recovery.
├── dmn-lite-compiler/              Source + Catalogue → CompiledDecision. resolve, type-check,
│                                   emit bytecode, BLAKE3 artifact hash. verify() → VerifiedDecision.
├── dmn-lite-engine/                Two evaluators on a single contract:
│                                   - reference: tree-walking, obviously-correct, no short-circuit
│                                   - vm: production stack machine, short-circuit, verified-only
├── dmn-lite-analysis/              Static analysis on VerifiedDecision:
│                                   SA-001 (UNIQUE+catch-all), overlap, unreachable-rule, gap,
│                                   cost ceiling, string-input precision warning.
├── dmn-lite-bridge/                FfiExecutionOwner impl (A10). DmnLiteOwner: register_decision
│                                   → FfiTemplate; invoke(FfiCall) → FfiResult via stack VM.
│
└── xtask/                          Smoke/stress harness CLI.
```

### Extraction path

dmn-lite is consolidated into the bpmn-lite workspace for build coherence and architectural symmetry. The dmn-lite crates remain logically independent and could be extracted into a standalone workspace if external adoption materialises. The FFI integration is via the `dmn-lite-bridge` crate, which is the only thing that would need to be left behind on extraction.

---

## Architecture

### Two dispatch paths (permanent)

| Opcode | Dispatch | Use case |
|--------|----------|----------|
| `Instr::ExecNative` | External-job queue → gRPC worker polls | Human approval, async callbacks, long-running external work |
| `Instr::ExecFfi` | In-process FfiDispatcher (A8) | Decisions (dmn-lite), HTTP calls, gRPC calls — millisecond latency |

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

### dmn-lite compilation pipeline

```
Source (s-expression DSL)
  ↓ dmn-lite-parser::parse()
AST
  ↓ dmn-lite-compiler::compile(source, catalogue, source_text)
CompiledDecision { bytecode, typed_ir, artifact_hash, ... }
  ↓ dmn-lite-compiler::verify()
VerifiedDecision                  ← only this reaches the VM
  ↓ dmn-lite-engine::evaluate()
EvaluationOutput { output, trace }
```

**Artifact hash:** `BLAKE3(normalised_source + resolved_entity_ids + compiled_ir)`. Same source against same catalogue → same hash.

**Type-state separation:** `VerifiedDecision(CompiledDecision)` is a newtype. The VM accepts only `VerifiedDecision`. `new_verified()` is callable only from `verify()`.

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
| A10 | — | `dmn-lite-bridge` crate (now in-workspace) | ✅ |
| A11 | — | First end-to-end test: BPMN → ExecFfi → dmn-lite → result | ✅ |
| B0 | — | dmn-lite consolidated into bpmn-lite workspace | ✅ |
| B1 | — | Dockerfile (cargo-chef, bpmn-lite/ context, docker-smoke) | ✅ |
| B5 | — | `docker-ffi-smoke`: dmn-lite FFI deployed proof | ✅ |
| B6 | — | HTTP FFI contract design (`b6-http-ffi-contract.md`) | ✅ |
| B7-B8 | — | `bpmn-lite-ffi-http` crate + `docker-http-smoke` deployed proof | ✅ |
| B9 | — | `docker-heterogeneous-smoke`: HTTP + dmn-lite in one process (tag `v0.1.0-heterogeneous-ffi`) | ✅ |
| A16 | Δ4 | Tenancy enforcement: RLS (025), `bpmn_lite_app` role (026), `tenants` directory (027), `set_tenant_context` at atomic paths, admin/runtime URL split, `verify_not_superuser` WARN | ✅ |
| A17 | Δ2 | Hot restart: `detect_interrupted_ffi_calls`; Idempotent auto-recover; NonIdempotent → Incident + Failed | ✅ |
| A18 | Δ3 | Tenancy remediation: A16-Audit findings resolved; `rows_affected` validation on 5 write methods; per-tenant scheduler via `tenants` table | ✅ |
| A19 | Δ4 | DB-enforced immutability: `integrity_hash` (BLAKE3) at creation; BEFORE UPDATE trigger (029) guards 7 immutable fields; `quarantine_state` + `InstanceQuarantined` event | ✅ |
| L0 | Δ3 | Pool schema: `tenant_pools` table (030), `pool_id` column on `tenants` (031), default pool seed + FK (032); `list_tenants_in_pool` on ProcessStore + both impls | ✅ |
| B2 | — | `docker-compose.yml` at repo root; admin/runtime URL split in Docker context | ✅ |
| B3 | — | Standard gRPC health endpoint (`tonic-health`, `grpc.health.v1.Health`) | ✅ |
| B11 | — | `verify_not_superuser` WARN → hard error; `BPMN_LITE_ALLOW_SUPERUSER=1` dev override | ✅ |
| B12 | — | Three-vocabulary heterogeneous proof: HTTP + dmn-lite + gRPC in one BPMN process; `docker-heterogeneous-smoke` updated | ✅ |

Design documents: `ob-poc/todo/bpmn-lite/` and `ob-poc/todo/dmn-lite/` — see architecture-commitments-v0_6.md and bpmn-dmn-lite-vs-v1_2.md.

---

## dmn-lite DSL Grammar (Profile v0.1)

```lisp
(define-decision <name>
  :hit-policy first | unique
  :inputs  ((<field> :type bool | integer | decimal | string | enum
                     :domain <domain-name>)
             ...)
  :outputs ((<field> :type <type> :domain <domain-name>)
             ...)
  :rules   ((rule <id>
               :when (<predicate> ...)  ; or (*) for catch-all
               :then (<assignment> ...))
             ...))
```

Predicate forms: `(field = literal)`, `(field != literal)`, `(field in [lo .. hi])`, `(field in (v1 v2 ...))`, `(*)` catch-all.

### dmn-lite analysis findings

| Finding | Severity | Description |
|---------|----------|-------------|
| `UniqueWithCatchAll` | Error | UNIQUE hit-policy with a catch-all rule — always produces MultipleMatches |
| `Overlap { rule_a, rule_b }` | Warning (UNIQUE) / Info (FIRST) | Two rules match the same input |
| `UnreachableRule { unreachable, shadowing }` | Error | Earlier rule subsumes later rule under FIRST |
| `Gap { field_gaps }` | Warning | Input combination no rule matches |
| `CostCeilingExceeded` | Error | Predicate count exceeds configurable ceiling (default 10,000) |
| `AnalysisLimitedByStringInput` | Info | String/decimal fields are treated as opaque; analysis is approximate |

### dmn-lite Profile Roadmap

| Profile | Status | Features |
|---------|--------|----------|
| v0.1 | ✅ Complete | integer/bool/enum/decimal/string, FIRST/UNIQUE, static analysis |
| v0.2 | ⬜ | Quantifiers + aggregation over governed collections, ANY/RULE_ORDER/COLLECT |
| v0.3 | ⬜ | DMN XML import linter, FEEL unary-test recogniser, temporal predicates |
| v0.4 | ⬜ | DRD cross-decision dependencies, path expressions |
| v0.5 | ⬜ | BKM non-recursive functions |

### dmn-lite-bridge

Registers compiled decisions as FFI templates for bpmn-lite processes. `owner_metadata` is the 32-byte BLAKE3 `artifact_hash` of the `VerifiedDecision` — template identity is unique per compiled artifact.

**Marshalling:** JSON `input_payload` → `TypedInputContext`. `Bool` → `TypedValue::Bool`, `I64` → `TypedValue::Integer`, `F64` → `TypedValue::Decimal`, `String` → `TypedValue::Str`. For `SemOsDomain` fields, an optional `ValueResolver` trait converts symbol strings to `TypedValue::Enum { domain_id, value_id }`; without it, falls back to `Str` (causes `InputTypeMismatch` — A12 scope for Sem OS catalogue integration).

### Two-valued null semantics

Per `dmn-lite-semantics.md` §3.2: null on either side of `=` or `!=` produces `false` (not an error, not null-propagation). This differs from SQL three-valued logic. The reference evaluator never short-circuits; the VM does. Phase 1.5 differential harness proves equivalence across 3,000+ generated inputs per fixture.

---

## Key Invariants

- **No runtime expression interpretation.** No FEEL, no JUEL, no embedded scripts. Every artifact is compiled before execution.
- **ExecFfi never reaches the VM directly.** The engine intercepts `TickOutcome::ExecFfi` and handles dispatch. The VM arm returns this outcome; the engine loop catches it.
- **Bounded computation.** `estimate_instr_count` enforces a cost ceiling at compile time.
- **DataObject nodes are structural.** They have no sequence-flow edges and zero bytecode. The verifier excludes them from the reachability check.
- **flag_symbol_table is preserved.** `lower()` inverts its intern map and stores it in `CompiledProgram.flag_symbol_table` (A3). The FFI binding layer uses it to resolve data-object names to FlagKeys.
- **Single workspace, single ffi-types copy.** Since B0, all consumers (including `dmn-lite-bridge`) take `ffi-types` via the workspace path dep. No `[patch]` mechanism required.
- **dmn-lite VerifiedDecision type-state.** The VM accepts only `VerifiedDecision`. `new_verified()` is callable only from `verify()`.

---

## Cross-repo dependency: ob-poc-types

bpmn-lite consumes `ob-poc-types` from the ob-poc repo as a **git dependency pinned by rev**:

```toml
ob-poc-types = { git = "https://github.com/adamtc007/ob-poc.git", rev = "397470cb" }
```

**Pin:** `397470cb` (ob-poc `codex/acp-workflow-validity-harness`, 2026-05-16). Bump deliberately when the ob-poc-types interface changes — update the workspace-level dep in `bpmn-lite/Cargo.toml`.

---

## Test Counts (2026-05-16, post-B0 consolidation)

| Crate | Tests |
|-------|-------|
| bpmn-lite-types | 0 |
| bpmn-lite-compiler | 32 |
| bpmn-lite-vm | 41 |
| bpmn-lite-engine (unit) | 45 |
| bpmn-lite-engine (a11 integration) | 3 |
| bpmn-lite-authoring | varies |
| ffi-types | 15 |
| ffi-catalogue | 8 |
| ffi-dispatcher | 12 |
| dmn-lite-parser | 78 |
| dmn-lite-compiler | 39 |
| dmn-lite-engine | 48 |
| dmn-lite-analysis | 57 |
| dmn-lite-bridge | 6 |
| bpmn-lite-ffi-http | 5 |
| bpmn-lite-types (integrity) | 10 |
| **Total (workspace, excl. postgres)** | **597 passing, 5 ignored** |
