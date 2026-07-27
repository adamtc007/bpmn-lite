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

pub mod board_candidate;
pub mod ops;
pub mod productions;
pub mod schema;
