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
//! **Audit status (carried over from Phase 0 findings):** the
//! entire authoring pipeline below is architecturally complete but
//! has no external caller — the gRPC service exposes only
//! `Compile(bpmn_xml: string)`. There is no `PublishWorkflow` /
//! `CompileYaml` / `Validate` / `Lint` / `Template*` RPC, no xtask
//! CLI surface, and `compile_and_publish` is not wired to persist
//! into the TemplateStore. The Phase 2.6 migration relocates the
//! code as-is; making this crate user-facing is a separate
//! downstream slice.
//!
//! Phase 2.6 (2026-05-14) migrated all eleven sub-modules
//! (contracts, dto, dto_to_ir, export_bpmn, ir_to_dto, lints,
//! publish, registry, store_postgres_templates (feature-gated),
//! validate, yaml) from `bpmn-lite-core/src/authoring/`.

pub mod contracts;
pub mod dto;
pub mod dto_to_ir;
pub mod export_bpmn;
pub mod ir_to_dto;
pub mod lints;
pub mod publish;
pub mod registry;
#[cfg(feature = "postgres")]
pub mod store_postgres_templates;
pub mod validate;
pub mod yaml;
