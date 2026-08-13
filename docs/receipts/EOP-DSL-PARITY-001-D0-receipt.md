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
`emit.rs`'s frozen canonical rule exactly.

**Corrected after blind review — the first draft's claim "the projection
is now content-canonical" was overbroad and empirically refuted:** the
reviewer built two verifier-clean, `ir_graphs_equivalent` DAGs with two
error guards on one host attached in opposite orders and got different
plan bytes — `guards_by_host`'s per-host Vec was still `node_indices()`
(edit) order. Same defect class, unexercised path. **Fixed in this
tranche** (not claim-narrowed): each host's `GuardExecSpec`s now sort by
guard id, with its own cement test
(`project_ir_guard_order_is_content_canonical`) and mutation red trace
(guard sort removed → test fails → restored → green). Guard order feeds
lowering's guard arms, so this was a determinism fix, not cosmetics.

Remaining precise scope of the canonicality claim: node order
(`BTreeMap`-keyed), flow order (edge-id-sorted), and guard order
(guard-id-sorted) are content-canonical; **edge ids themselves remain
part of the content** — `ir_graphs_equivalent` deliberately excludes
edge ids from its comparison, so two "equivalent" graphs whose split
arms carry *different edge ids* still project different flow orders.
The canonical claim is therefore: same nodes + same edges *including
ids* → byte-identical plan, regardless of edit order. (The
cement tests assert exactly this; the reviewer's edge-id nuance is
hereby recorded rather than papered.)

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

- **Blind peer-review findings and dispositions:** an independent
  reviewer (no prior context) re-derived the entire impact table —
  finding all three `blake3(plan_json)` write sites and confirming no
  fourth exists, confirming `store_plan` is idempotent-no-op on existing
  keys in BOTH store impls (memory `or_insert_with`; postgres
  `ON CONFLICT DO NOTHING`), that `template_plan_hash` has zero
  recompute-compare consumers, that no test pins a plan-hash constant,
  and that G6 replay compares graph equivalence, not plan bytes.
  Reproduced the cement test's mutation red trace (confirming g3/g4/g6 +
  cement are exactly the tests that depend on the sort) and the full
  workspace suite. Verdict: A/C/D/E verified; **B refuted as stated** —
  findings and dispositions:
  1. **"The projection is now content-canonical" was overbroad**:
     `guards_by_host` order was still edit-order-derived, empirically
     reproduced with two error guards on one host (verifier-legal).
     Disposed by FIXING (guard-id sort + cement test + red trace), not
     by narrowing — and the work-item-4 determinism note now genuinely
     holds, since guard order feeds lowering's guard arms.
  2. **Edge-id nuance**: `ir_graphs_equivalent` excludes edge ids, so
     "equivalent graphs → identical bytes" needed the precise statement
     now in work item 1 (same edges *including ids* → identical bytes).
     Recorded, not papered.
  3. Confirmed the re-publish behaviour post-D0 (new canonical hash →
     new plan row via idempotent insert; old template versions keep
     their old pointers) is harmless duplication, as the impact table
     claimed.

- **STOP-gate decision: blocked — awaiting peer review of this receipt.**

Per Gate D0's own text: cement test green (with red trace), impact table
above, B2 tightened, all suites green. D1 (boundary guards) does not
start until this gate is accepted.
