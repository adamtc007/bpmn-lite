# EOP-PLAN-DSL-PARITY-001 — D0 receipt

Baseline: plan accepted at `1b3056e` (branch
`codex/bpmn-gameboard-refactor`). **Tier: CAREFUL — touches a shipped
projection's stored bytes.**

- **Scope delivered:** all four D0 work items — the sort, the evidenced
  impact check, the B2 tightening (multiset amendment reversed), and the
  kernel-side determinism note.

## Work item 1 — the sort

`project_ir`'s diverging-gateway arm now collects outgoing edges and
sorts by sequence-flow id before building `SplitExecNode.flows`
(ir_plan.rs, replacing the raw `edges_directed` iteration). This matches
`emit.rs`'s frozen canonical rule exactly — the projection is now
content-canonical: two edit orders building `ir_graphs_equivalent`
graphs project byte-identical plans.

## Work item 2 — impact check (evidenced, per-consumer)

The STOP condition ("any consumer pins byte-identity of already-stored
artifacts across re-projection") was checked against every consumer
class found by direct search, and none pins:

| Consumer | Mechanism | Verdict |
| --- | --- | --- |
| Plan store (`store_plan`/`load_plan`, memory + postgres) | `plan_hash = blake3(plan_json)` computed at WRITE time; lookups are by the stored hash — content addressing over immutable entries | Old entries untouched; a re-projection of the same graph now mints a different (canonical) hash → a NEW entry. Nothing dangles. |
| Instances (`plan_hash` on instance records) | Stored pointer into the plan store, written at spawn | Running/historic instances keep their original plan bytes verbatim. |
| Templates (`template_plan_hash` column; template catalog dual-write) | Stored pointer written at publish; read back as a field. Grepped every non-store usage: **no site recomputes a plan hash and compares it against a stored one** | No recompute-compare exists anywhere. |
| `spawn_template_instance_endpoint` → `reconstruct_plan_from_template` | Re-projects fresh via `dto_to_ir → verify → project_ir`, compiles fresh, starts the instance from the fresh plan — never compares against `template_plan_hash` | A template spawned after D0 gets canonical flow order; before/after spawns of the same template may differ in flow order, which is outcome-equivalent (B2's kernel tracing: count-based barrier, no branch indexing). |
| G6 replay artifacts | Replay-equivalence compares route/graph equivalence, not plan bytes across projection versions; full workspace suite green confirms | Unaffected; D0 is what makes the *forward* replay-tape determinism claim available. |

## Work item 3 — B2 tightening

- `normalize_plan` no longer sorts flows — strict ordered equality is
  restored; the B2-era multiset amendment is **reversed**, recorded in
  the harness doc comment with its history.
- New cement test `project_ir_flow_order_is_content_canonical`: the same
  And-block content built in two opposite branch-insertion orders is
  `ir_graphs_equivalent` and projects **byte-identical** serialized
  plans. **Red trace:** with the sort deliberately removed (working-tree
  mutation), this test fails; restored, 9/9 harness tests green. (The
  mutation-restore step used `git restore`, which also reverted the
  not-yet-committed D0 sort itself — re-applied and re-verified before
  committing; noted for honesty since the sequence appears in the
  session log.)

## Work item 4 — determinism note

`V2Fork` target order, fiber-ID assignment, and event-tape ordering are
now content-derived for graph-authored parallel blocks: same design
content → same lowered fork order, regardless of edit order. This
discharges the B2 standalone observation and removes the
outcome-equivalent-but-not-replay-tape-identical caveat for plans
projected after D0.

## Verification

- `cargo test -p designer-graph --lib`: 81/0 (72 + 9 harness incl. the
  new cement).
- `cargo test -p bpmn-lite-compiler --lib`: 198/0.
- `cargo test -p bpmn-lite-server-designer --lib`: 97/0 (1 ignored).
- `cargo test --workspace` (full): no failures.
- `cargo check --workspace --all-targets`: clean (same 2 pre-existing
  unrelated warnings).
- Public API diff: none (behaviour change inside an existing fn).

- **Refusal catalogue delta vs B0's frozen list: none.**

- **Known deviations or explicitly parked work:** none — the tranche is
  exactly its four work items.

- **Blind peer-review findings and dispositions:** pending — dispatched
  at this receipt's close.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate D0's own text: cement test green (with red trace), impact table
above, B2 tightened, all suites green. D1 (boundary guards) does not
start until this gate is accepted.
