//! WS-A `designer-graph` (EOP-PLAN-BPMN-DESIGN-003 v0.2).
//!
//! Invariants inherited from EOP-VS-BPMN-DESIGN-003 v0.6, binding on every
//! module in this crate:
//! - I16: structural derivation (pairing, regions, merge identity) is
//!   consumed from `bpmn_lite_compiler::{compute_post_dominators,
//!   compute_region_map, gateway_pairs}` — never reimplemented here.
//! - I23: no operation or production may introduce a backward edge; the
//!   compiler's cyclicity gate is the admission backstop, never the
//!   working mechanism. Acyclicity pre-gating before calling the oracle
//!   is THIS crate's responsibility (oracle precondition).
//! - I24: staging is refused where a mandatory declaration (MI max) is
//!   absent; declarations never route through the DTO surface (C7).
//! - P9: models select; this crate's deterministic builders construct.
//!
//! **Module-boundary note (pub-scope audit, 2026-07-29):** the 5 `pub mod`
//! submodules below are deliberate, addressed-by-name API surface, not a
//! facade leak — each maps to a distinct conceptual sub-domain
//! (`board_candidate`: legality oracle trait; `ops`: staging operations;
//! `positional`: positional-legality implementation; `productions`:
//! production appliers; `schema`: the DAG type itself), and every real
//! consumer (`bpmn-lite-server-designer`, `utterance-engine`) already
//! addresses them by module-qualified path as its chosen idiom. Same
//! judgment as `bpmn-lite-store`'s `store`/`pending`/`store_memory` split —
//! do not flatten this into a crate-root facade in a future pass without
//! re-litigating that call.

pub mod board_candidate;
mod g2_receipts;
pub mod ops;
pub mod positional;
pub mod productions;
pub mod runbook;
pub mod schema;
