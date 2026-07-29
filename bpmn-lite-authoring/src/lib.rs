//! BPMN-Lite authoring.
//!
//! Owns the YAML / DTO authoring pipeline that turns a hand-written
//! workflow specification into a verified, lint-checked,
//! hash-stamped, BPMN-XML-exportable artefact ready for execution.
//!
//! Pipeline (per `publish_workflow`): parse YAML → validate DTO →
//! lint against contract registry (L1–L5) → DTO→IR → verify IR →
//! lower to bytecode → hash → extract task manifest → optional
//! BPMN-XML export → assemble `WorkflowTemplate`.
//!
//! **Real external callers today:** `diagnostics_executor` (autofix
//! actions for the designer's diagnostics-resolve endpoint) is live
//! production code, consumed by `bpmn-lite-server-designer`. `dto`,
//! `publish`, and `yaml` are consumed by `bpmn-lite-engine`'s test
//! suite. The stale "no external caller" claim this doc comment used
//! to carry (Phase 0 audit findings) no longer holds — it predates
//! the designer crate's diagnostics-resolve wiring.
//!
//! Phase 2.6 (2026-05-14) migrated all eleven sub-modules
//! (contracts, dto, dto_to_ir, export_bpmn, ir_to_dto, lints,
//! publish, registry, store_postgres_templates (feature-gated),
//! validate, yaml) from `bpmn-lite-core/src/authoring/`.
//!
//! All submodules are private — every external consumer reaches the
//! curated vocabulary flat (`bpmn_lite_authoring::Foo`) via the
//! prelude `pub use`s below.

mod contracts;
mod diagnostics_executor;
mod dto;
mod dto_to_ir;
mod export_bpmn;
mod importer;
mod ir_to_dto;
mod lints;
#[cfg(test)]
mod oracle_boundary_tests;
mod publish;
mod registry;
#[cfg(feature = "postgres")]
pub(crate) mod store_postgres_templates;
mod validate;
mod yaml;

pub use diagnostics_executor::{execute_autofix, FixAction};
pub use dto::{
    EdgeDto, ErrorEdge, FlagCondition, FlagOp, FlagValue, NodeDto, RaceArm, RaceArmKind,
    TemplateMeta, WorkflowGraphDto,
};
pub use importer::import_zeebe_bpmn;
pub use publish::{compile_and_publish, compile_program_from_dto, PublishOptions, PublishResult};
pub use yaml::parse_workflow_yaml;
