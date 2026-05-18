//! Static analysis for compiled bpmn-lite programs.
//!
//! Analogous to `dmn-lite-analysis` for the decision vocabulary. Analyses
//! a `CompiledProgram` and returns a list of findings without modifying any state.
//!
//! # Findings
//!
//! - [`FindingKind::ConstantCondition`]: a branch condition is a compile-time
//!   literal (`PushBool(v) BrIf/BrIfNot`) — the branch is always or never taken.
//!
//! - [`FindingKind::UnwrittenFlagCondition`]: a flag is used as a branch condition
//!   (`LoadFlag {key} BrIf/BrIfNot`) but is never written by any `StoreFlag {key}`
//!   in the program. The flag always holds its initial value (false/0), so the branch
//!   is effectively constant.
//!
//! - [`FindingKind::FfiTemplatePinned`]: informational — lists every FFI template_id
//!   referenced in the program (for coverage cross-checking at startup).

#![forbid(unsafe_code)]

use std::collections::HashSet;

use bpmn_lite_types::{CompiledProgram, FlagKey, Instr};
use serde::{Deserialize, Serialize};

/// Severity of an analysis finding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Likely a bug or authoring error — should be fixed.
    Warning,
    /// Informational; useful for tooling, not necessarily a problem.
    Info,
}

/// One static analysis finding on a compiled BPMN program.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub kind: FindingKind,
    /// BPMN element id at or near the finding (from debug_map), if available.
    pub element_id: Option<String>,
    /// Human-readable description.
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum FindingKind {
    /// `PushBool(v) + BrIf/BrIfNot` — branch is always or never taken.
    ConstantCondition {
        /// Bytecode address of the BrIf/BrIfNot instruction.
        branch_addr: u32,
        /// Whether the branch is always taken (true) or never taken (false).
        always_taken: bool,
    },
    /// `LoadFlag {key} + BrIf/BrIfNot` where `key` is never written by `StoreFlag`
    /// anywhere in the program. Flag always reads as its initial value (false/0).
    UnwrittenFlagCondition {
        branch_addr: u32,
        flag_key: FlagKey,
        /// Symbolic name from flag_symbol_table, if available.
        flag_name: Option<String>,
    },
    /// Informational: an ExecFfi instruction references this template_id.
    FfiTemplatePinned {
        template_id_hex: String,
        ffi_task_addr: u32,
    },
}

/// Result of analysing a compiled program.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub findings: Vec<Finding>,
}

impl AnalysisReport {
    pub fn warnings(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
    }

    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }
}

/// Analyse a compiled BPMN program. Returns a report with all findings.
pub fn analyse(program: &CompiledProgram) -> AnalysisReport {
    let mut findings = Vec::new();

    // Collect all FlagKeys that are ever written in the program.
    let written_flags: HashSet<FlagKey> = program
        .program
        .iter()
        .filter_map(|instr| match instr {
            Instr::StoreFlag { key } => Some(*key),
            _ => None,
        })
        .collect();

    // Walk the bytecode looking for branch patterns.
    let instrs = &program.program;
    for (i, instr) in instrs.iter().enumerate() {
        let next = instrs.get(i + 1);
        let branch_addr = (i + 1) as u32;
        let element_id = program
            .debug_map
            .get(&(branch_addr))
            .or_else(|| program.debug_map.get(&(i as u32)))
            .cloned();

        match instr {
            // Pattern: PushBool(v) followed by BrIf or BrIfNot.
            Instr::PushBool(v) => {
                if let Some(branch) = next {
                    match branch {
                        Instr::BrIf { .. } => {
                            let always_taken = *v;
                            findings.push(Finding {
                                severity: Severity::Warning,
                                kind: FindingKind::ConstantCondition {
                                    branch_addr,
                                    always_taken,
                                },
                                element_id,
                                message: format!(
                                    "Branch at addr {} is {} (literal bool condition '{}')",
                                    branch_addr,
                                    if always_taken {
                                        "always taken"
                                    } else {
                                        "never taken"
                                    },
                                    v
                                ),
                            });
                        }
                        Instr::BrIfNot { .. } => {
                            let always_taken = !v;
                            findings.push(Finding {
                                severity: Severity::Warning,
                                kind: FindingKind::ConstantCondition {
                                    branch_addr,
                                    always_taken,
                                },
                                element_id,
                                message: format!(
                                    "Branch at addr {} is {} (literal bool condition '!{}')",
                                    branch_addr,
                                    if always_taken {
                                        "always taken"
                                    } else {
                                        "never taken"
                                    },
                                    v
                                ),
                            });
                        }
                        _ => {}
                    }
                }
            }

            // Pattern: LoadFlag {key} followed by BrIf or BrIfNot, where key is never written.
            Instr::LoadFlag { key } => {
                if !written_flags.contains(key) {
                    if let Some(branch) = next {
                        if matches!(branch, Instr::BrIf { .. } | Instr::BrIfNot { .. }) {
                            let flag_name = program.flag_symbol_table.get(key).cloned();
                            let name_str = flag_name.as_deref().unwrap_or("<unnamed>");
                            findings.push(Finding {
                                severity: Severity::Warning,
                                kind: FindingKind::UnwrittenFlagCondition {
                                    branch_addr,
                                    flag_key: *key,
                                    flag_name: flag_name.clone(),
                                },
                                element_id,
                                message: format!(
                                    "Flag '{}' (key={}) used as branch condition at addr {} but never written in this process; branch always evaluates against initial value (false/0)",
                                    name_str, key, branch_addr
                                ),
                            });
                        }
                    }
                }
            }

            _ => {}
        }
    }

    // Informational: list FFI template pins.
    for (addr, decl) in &program.ffi_task_decls {
        let template_id_hex: String = decl
            .template_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let element_id = program.debug_map.get(addr).cloned();
        findings.push(Finding {
            severity: Severity::Info,
            kind: FindingKind::FfiTemplatePinned {
                template_id_hex: template_id_hex.clone(),
                ffi_task_addr: *addr as u32,
            },
            element_id,
            message: format!(
                "ExecFfi at addr {} pins template {}",
                addr,
                &template_id_hex[..16]
            ),
        });
    }

    AnalysisReport { findings }
}

/// Produce a summary string suitable for a log line or diagnostic message.
pub fn summarise(report: &AnalysisReport) -> String {
    let warnings = report.warning_count();
    let infos = report.findings.len() - warnings;
    format!("{} warning(s), {} info(s)", warnings, infos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bpmn_lite_types::{CompiledProgram, Instr};
    use std::collections::BTreeMap;

    fn empty_program(instrs: Vec<Instr>) -> CompiledProgram {
        CompiledProgram {
            bytecode_version: [0u8; 32],
            program: instrs,
            debug_map: BTreeMap::new(),
            join_plan: BTreeMap::new(),
            wait_plan: BTreeMap::new(),
            message_name_map: BTreeMap::new(),
            race_plan: BTreeMap::new(),
            boundary_map: BTreeMap::new(),
            write_set: BTreeMap::new(),
            task_manifest: vec![],
            error_route_map: BTreeMap::new(),
            flag_symbol_table: BTreeMap::new(),
            data_objects: BTreeMap::new(),
            ffi_task_decls: BTreeMap::new(),
        }
    }

    #[test]
    fn detects_constant_false_condition() {
        let prog = empty_program(vec![Instr::PushBool(false), Instr::BrIf { target: 10 }]);
        let report = analyse(&prog);
        assert_eq!(report.warning_count(), 1);
        assert!(matches!(
            report.findings[0].kind,
            FindingKind::ConstantCondition {
                always_taken: false,
                ..
            }
        ));
    }

    #[test]
    fn detects_constant_true_condition_inverted() {
        let prog = empty_program(vec![Instr::PushBool(true), Instr::BrIfNot { target: 10 }]);
        let report = analyse(&prog);
        // PushBool(true) + BrIfNot → never taken
        assert_eq!(report.warning_count(), 1);
        assert!(matches!(
            report.findings[0].kind,
            FindingKind::ConstantCondition {
                always_taken: false,
                ..
            }
        ));
    }

    #[test]
    fn detects_unwritten_flag_condition() {
        let prog = empty_program(vec![Instr::LoadFlag { key: 7 }, Instr::BrIf { target: 10 }]);
        let report = analyse(&prog);
        assert_eq!(report.warning_count(), 1);
        assert!(matches!(
            report.findings[0].kind,
            FindingKind::UnwrittenFlagCondition { flag_key: 7, .. }
        ));
    }

    #[test]
    fn no_warning_when_flag_is_written() {
        let prog = empty_program(vec![
            Instr::PushBool(true),
            Instr::StoreFlag { key: 7 },
            Instr::LoadFlag { key: 7 },
            Instr::BrIf { target: 10 },
        ]);
        let report = analyse(&prog);
        assert_eq!(report.warning_count(), 0);
    }

    #[test]
    fn no_warning_for_clean_process() {
        // Typical pattern: LoadFlag on a written flag, no literal branches
        let prog = empty_program(vec![
            Instr::PushBool(true),
            Instr::StoreFlag { key: 0 },
            Instr::LoadFlag { key: 0 },
            Instr::BrIf { target: 5 },
            Instr::Jump { target: 6 },
        ]);
        let report = analyse(&prog);
        assert_eq!(report.warning_count(), 0);
    }
}
