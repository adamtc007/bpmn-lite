# EOP-DSL-AUTHORING-AUDIT-001 — BPMN design-DSL capability audit (FOR ADAM'S REVIEW)

Status: **AUDIT DELIVERED 2026-07-27 (evidence sweep, file:line receipts).
Awaiting Adam's review. The gap list in §4 is oriented toward the ratified
goal: a Sage ↔ REPL BPMN template design-and-persist session, fully wired
and testable.**

Legend: EXISTS (code present) / WIRED-LIVE (reachable from a live
endpoint or UI) / DORMANT (compiles, tested, no live caller) / NOT-FOUND.

## 1. State vs the intended architecture

| Intended capability | State | Evidence |
|---|---|---|
| XML transpiled INTO the DSL S-expr, then one common path | **NOT-FOUND** — XML and DSL are parallel frontends converging at `WorkflowExecutionPlan`, not at the S-expression | `bpmn-lite-authoring/src/importer.rs:13-320` (XML → IR → plan directly), `bpmn-lite-compiler/src/dsl/mod.rs:84-110` |
| Developer macros as design vocabulary | **EXISTS + WIRED-LIVE** as an authoring/mutation layer (NOT a compile phase — `dsl::compile` has no expansion pass). Both halves of the Template≠macro distinction live in `dsl/macros.rs`: developer builder fns (bounded-retry, xor-split-join, parallel-split-join) AND config-loaded `%param%` templates from `macros.yaml` | `dsl/macros.rs:11-160`, applied via `POST /api/dsl/macro/apply` → `AstMutator` → `to_sexpr` → re-`compile` (`rest.rs:1579-1826`) |
| DSL vocabulary as a sealed PACK, activated per workspace | **Half-built; seal DORMANT.** `pack_build.rs` builds real packs (blake3 content-hash version, G1–G6 gates, sealed closure manifests) but only via `xtask pack-build` from hand-written DAG YAMLs. Runtime loads loose manifest YAMLs (`SAGE_MANIFESTS_DIR`) into `ManifestPlaceholderRegistry` — **the seal is never verified at runtime (G4-class hollow gate)** | `dsl/pack_build.rs:499-531`, `xtask/src/main.rs:1231`, `rest.rs:1078-1090,1932` |
| DAG dual taxonomy (resource vs execution ordering) | **NOT-FOUND** — single execution topological order only | `dsl/frontend.rs:445`; empty greps in dag/plan/rpst/closure |
| Sage instructed into BPMN domain (embedding intent mapping) | **Stub.** `sage_utterance_gate` is substring keyword matching; Sage reasoning records are `Vec<()>`; zero Candle/embedding hits workspace-wide | `rest.rs:2007-2084,108,401` |
| UI for DSL authoring | **NONE.** `BpmnDemoPage` (ob-poc) is a runtime viewer; calls no DSL authoring endpoint | `ob-poc-ui-react/src/api/bpmn.ts:65-112` |
| XML import/export | EXISTS but DORMANT (test-only; no live endpoint) — round-trip fidelity fixed under F3 ruling (error catalog) | `importer.rs` + compat tests; `export_bpmn.rs` |

**WIRED-LIVE and load-bearing today:** `dsl::compile`
(lex→parse→lint→validate_dag), L4 closure/path-family validation,
`ManifestPlaceholderRegistry` (server + bus-handler + engine demo), and
five authoring endpoints: `POST /bpmn/compile/preview`,
`POST /api/dsl/macro/apply`, `POST /api/dsl/diagnostics/resolve`,
`POST /api/dsl/sage/utter`, `GET/POST /bpmn/templates`.

## 2. Test / review posture

Uneven: linter (4) / closure (11) / manifest_registry (9) /
lexer (7) well-tested; `macros.rs` has ONE test; `plan.rs` and `ast.rs`
have ZERO inline tests. The macro/refactor/utterance layer landed as a
single feature commit (`25d4cba`) with one review-fix pass (`059def5`)
and has never been blind-reviewed. Fuzz coverage: `dsl_compile` target
(F8.5) hammers lex→parse→lint→dag + gate parity nightly; the
macro/AstMutator layer is NOT under fuzz.

## 3. Rulings already made (2026-07-27, recorded in EOP-FUZZ §10)

- REST DSL admission: plan-tier (lint+dag+SESE+closure) RULED
  sufficient for the served/simulated surface; boundary condition — any
  future lower-and-execute of a stored plan on the real kernel must pass
  bytecode admission; D-O2 fuzz oracle is the drift check.
- Non-interrupting error boundaries: parse-time reject (implemented).
- Exporter error catalog: emit + round-trip cement (implemented).

## 4. Gap list blocking the Sage ↔ REPL design-and-persist session

Ordered by dependency, each with a testability note. NOT ratified as
work — this is the review agenda.

1. **Ratify frontend architecture** (fork): parallel-frontends (ratify
   what exists; XML importer stays a dormant compatibility path) vs
   build the XML→S-expr transpile. Everything below is unblocked by
   either ruling; recommendation remains ratify-parallel.
2. **Seal the pack loop**: runtime registry construction must load a
   sealed pack closure and hash-verify it (exact pin, not floor) instead
   of loose YAML. Receipt: tampered-pack red test + pinned-pack green;
   G4 discipline applied to the BPMN vocabulary itself.
3. **Macro layer hardening**: (a) blind review of
   macros.rs/refactor.rs/rest.rs authoring endpoints; (b) fuzz target
   over the macro-apply path (tape → utterance/macro-config →
   AstMutator → to_sexpr → compile; oracle: mutation output always
   re-compiles — correct-by-construction claim made testable);
   (c) tests for plan.rs/ast.rs.
4. **Sage utterance → intent**: replace the keyword gate with the
   embedding-driven mapping (ob-poc side; Candle per the settled
   decision) or explicitly ratify the keyword gate as the v1 REPL
   contract. The REPL can ship on the keyword gate; the decision is
   scope, not correctness.
5. **Session persistence**: templates persist via
   `store_postgres_templates` (live); the design SESSION (dialogue
   state, in-progress AST, undo) has no store. Define the
   session aggregate + store surface; testable Postgres-independent via
   MemoryStore mirror.
6. **REPL/UI wiring**: connect a client (ob-poc panel or CLI REPL) to
   the five live endpoints; end-to-end receipt = scripted session:
   utter → macro-apply → diagnostics → preview-compile → template
   persist → spawn instance in the simulator.
7. **Dual taxonomy**: decide build-or-drop. If build: second projection
   over the same plan (resource grouping), never a second source of
   truth (the DAG stays normative).

## 5. Fuzz-coverage posture (context for the review)

Core engines: 15-target fleet, both real bugs found by first-run
oracles (F2-KERNEL-001 kernel sweep gap; F8-COMPILER-001 lowering
panic), routing/faults/restarts/flag-storms/message-correlation all
under continuous fuzz. Adam's verdict 2026-07-27: core coverage
excellent; fuzz value lands on runtime logic/gated switching — further
fuzz investment should follow the authoring-layer work (item 3b), not
precede it.

---
*Drafted 2026-07-27 from the evidence sweep; amend in place after
Adam's review; the ratified subset becomes the next programme doc.*
