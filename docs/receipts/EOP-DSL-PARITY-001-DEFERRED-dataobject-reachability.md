# Deferred — DataObject reachability vs. Stage-0 emission DFS

**Status:** DEFERRED (Adam, 2026-08-14). Needs its own design thinking;
explicitly not to be forgotten between tranches. Not scheduled against any
tranche in `EOP-PLAN-DSL-PARITY-001` — DataObject is already out of that
programme's ruled scope ("P2 ... and DataObject are OUT — a future
programme with its own V&S").

**Surfaced by:** Gate D3 (MultiInstance), while building the REST endpoint
witness. Full root-cause writeup lives in
`docs/receipts/EOP-DSL-PARITY-001-D3-receipt.md`, section "Surfaced, NOT
decided — a pre-existing, cross-cutting gap this tranche made visible".
Summary kept here so the finding survives independently of D3's receipt.

## The gap

- `Operation::CreateDataObject` inserts `IRNode::DataObject` nodes with "no
  anchor, no edge" — permanently edgeless in the graph model.
- `emit_dsl`'s Stage-0 reachability check (flow-DFS from `Start`, dating to
  B1, unchanged through D1/D2/D3) has no exemption for structural
  declaration nodes — so any graph containing a `DataObject` unconditionally
  refuses DSL emission with `UnreachableNode`, regardless of what
  legitimately references it.
- G7.4 (verifier.rs) requires `IRNode::MultiInstance.collection_flag_name`
  to reference a declared `DataObject` for graph admission.
- Net effect: a real, G7.4-admitted, designer-authored MultiInstance graph
  (built via the actual `/graph-edit` REST path) can never successfully
  DSL-emit. Only a hand-built `IRGraph` that bypasses the G7.4 admission
  gate (as D3's unit tests and B2 G16 fixture correctly do, matching G13-G15
  precedent) reaches the green MI-emits-to-DSL path.
- Confirmed via D3's blind review (independent git blame) to predate
  D1/D2/D3, dating to B1. Not a D3-introduced defect — D3 is the first
  tranche where a *core, emittable* node kind has a *mandatory* dependency
  on a *structurally unreachable* node kind, making the collision
  practically unavoidable rather than incidental.

## Candidates (not decided, not ranked as a ruling — this doc does not rule)

- (a) A Stage-0 reachability exemption for `DataObject` nodes specifically
  — structural declarations are not "flow", so "reachable from Start" may
  be the wrong test for them.
- (b) Rethink whether `:collection` should reference a `DataObject` at all
  on the DSL path.

## Why deferred rather than fixed inline at D3

DataObject is out of `EOP-PLAN-DSL-PARITY-001`'s ruled scope. Folding a
DataObject-reachability fix into a MultiInstance tranche would be scope
creep into a kind this programme explicitly excludes. It needs its own
V&S/design note, most likely opened as its own EOP once DataObject enters
scope, or as a design note preceding whichever future programme takes
DataObject on.

## Next action

None scheduled. Re-surface this doc when DataObject enters scope of a
future programme, or if a later parity tranche in this programme trips
over the same DFS-vs-structural-node collision for a different node kind
(the receipt recommends checking for that before any future DSL-parity
gate that similarly requires a DataObject reference).
