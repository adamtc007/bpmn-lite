# Shared-crates remediation Phase 6 blueprint

**Phase:** 6 — define the Sage/REPL boundary
**Date:** 5 August 2026
**Shared base:** DSL `5ac7da7a513744e907ca110484c3a6a9472ae985` on `feat/semantic-embedder`
**BPMN base:** `2665c06ad42ef51a54e42c7739546edfc6ccbf49` on `refactor/semantic-embedder`
**ob-poc base:** `333975b7c453758f5fabfdba76b2a0875df5da05` on `refactor/semantic-embedder-adapter`

## Forensic ruling

Sage is currently an `ob-poc` application UI/runtime, not a transport-neutral shared protocol. The live request, session, persona, persistence, route and response types are owned by `ob-poc` in `rust/src/api/repl_routes_v2.rs`, `rust/src/repl/` and `rust/crates/ob-poc-sage`. Cargo metadata proves that `rust/crates/dsl-sage` has no consumer other than its own integration-test self-dependency.

Fork F2 therefore stands: Phase 6 creates no `repl-contracts` crate. A shared contract may be reconsidered only after a second real consumer demonstrates a stable, domain-neutral protocol seam.

## Invariants and absolute boundaries

1. `ob-poc` retains its Sage routes, wire types, persona/phase vocabulary, persistence, UI behavior and `ob-poc-sage` dependency graph unchanged.
2. The orphan `dsl-sage` package is retired, not moved into the shared DSL workspace and not renamed to imply a shared runtime.
3. Removing `dsl-sage` must not alter any production dependency edge. Cargo metadata before and after must show zero lost consumers because no production consumer exists.
4. Historical evidence documents may continue to describe the former crate, but active workspace membership, lockfile package data and generated public-surface inventory must not advertise it.
5. The BPMN compatibility endpoint keeps its existing route and response JSON fields. New request context is optional, so existing callers continue to deserialize.
6. The local BPMN keyword gate is named and documented as a designer compatibility classifier, never as a shared Sage engine.
7. Retry suggestions use only a caller-supplied BPMN node identity. Missing selection fails closed with an explanatory `none` response; no synthetic node identity is emitted.
8. Diagnostic import suggestions use only an explicit import token or caller-supplied unresolved verb. Missing verb context fails closed; no `ob-poc` verb or domain is invented.
9. An unqualified explicit verb is interpreted in the BPMN host domain and emitted as a qualified `bpmn:<verb>` candidate. A caller-supplied qualified external verb retains its explicit domain.
10. Preview compiler support for explicitly authored external invocation manifests is not redesigned in this phase. Phase 2 owns manifest/pack reconciliation; Phase 6 removes only implicit command fallback authority.
11. No persistent hash, UUID namespace, proposal/workbook schema, database schema, runtime service, model artifact or deployment topology changes.
12. Pre-existing `.cargo/config.toml.example` and coordinating-worktree changes remain unmodified and uncommitted.

## ob-poc retirement boundary

Delete the following active package surface:

```text
rust/crates/dsl-sage/
audits/surface/dsl-sage.txt
```

Update:

```text
rust/Cargo.toml       # remove the orphan workspace member
rust/Cargo.lock       # remove the orphan workspace package entry
```

No module is moved or compatibility-re-exported because metadata and repository search prove there is no consumer. `rust/crates/ob-poc-sage` and all inline application REPL code are preserved.

## BPMN compatibility-classifier structure

Target module: `bpmn-lite-server-designer/src/rest.rs`.

Normative skeleton:

```rust
#[derive(Deserialize)]
struct UtteranceRequest {
    utterance: String,
    _current_dsl: String,
    #[serde(default)]
    target_node_id: Option<String>,
    #[serde(default)]
    unresolved_verb: Option<String>,
}

struct DesignerUtteranceContext<'a> {
    target_node_id: Option<&'a str>,
    unresolved_verb: Option<&'a str>,
}

async fn designer_utterance_compat_endpoint(
    Json(body): Json<UtteranceRequest>,
) -> impl IntoResponse;

fn classify_designer_utterance(
    utterance: &str,
    context: DesignerUtteranceContext<'_>,
) -> UtteranceResponse;

fn explicit_import_candidate(
    utterance: &str,
    unresolved_verb: Option<&str>,
) -> Option<(String, String)>;
```

The route remains `POST /api/dsl/sage/utter` solely as a wire-compatibility alias. Implementation names and comments state that this is a local deterministic classifier.

## Response policy

| Input | Context | Result |
|---|---|---|
| escape/deploy phrase | none required | unchanged compatibility response |
| retry/wrap phrase | non-empty `target_node_id` | `apply_macro` with that exact ID |
| retry/wrap phrase | no target | `none`, request a selected BPMN node |
| `import <verb>` | explicit token | `AddVerbStub`; unqualified token is BPMN-qualified |
| unknown-verb phrase | non-empty `unresolved_verb` | `AddVerbStub` for that exact candidate |
| unknown-verb phrase | no verb | `none`, request the unresolved verb identity |
| anything else | any | unchanged editing-mode response |

## Verification design

- baseline and post-change Cargo metadata prove `dsl-sage` has no external reverse dependency;
- the ob-poc workspace checks without `dsl-sage`, and repository search finds no active package reference;
- direct classifier tests cover injected retry targets, missing retry targets, explicit BPMN imports, qualified external imports, injected unresolved verbs and missing unresolved-verb context;
- route tests prove the old path and response schema still work;
- source search proves no `create-cbu`, `ob-poc:cbu.create` or default `ob-poc` value remains in the classifier production block;
- focused check/test/Clippy gates run before full repository gates;
- exact-revision gates run with ignored local development patches temporarily disabled and restored.

## Expected commits

1. ob-poc: `cleanup: retire orphan dsl-sage crate`.
2. BPMN: `fix(designer): remove ob-poc utterance fallbacks`.

The shared DSL repository receives no Phase 6 commit. The phase stops after its receipt; Phase 2 does not begin in the same gate.
