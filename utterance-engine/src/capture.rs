//! Q9-GATED USER CAPTURE — feature `q9-capture` only. This entire module
//! is compiled ONLY when that feature is enabled at build time (see
//! `lib.rs`'s `#[cfg(feature = "q9-capture")] pub mod capture;`), which
//! is off by default and absent from every documented build/release
//! command in this repo (`scripts/check-q9-capture-gate.sh` enforces
//! that mechanically — DIR-004 Phase 1.2).
//!
//! DIR-004 Phase 1 ruling (structural separation, Option B): a
//! pre-charter build must not be ABLE to contain a live user-capture
//! path, not merely configured to leave it off at runtime. Before this
//! split, `CapturePipeline::off()` was reachable in every build — a
//! single application-code edit (swap `off()` for `on_under_charter()`)
//! would have silently activated real capture with no build-system
//! signal. Now that edit additionally requires `--features q9-capture`,
//! a visible, CI-checked build flag — the gate is a compile-time fact,
//! not a runtime convention. `utterance_engine::dev_capture` (always
//! compiled, unconditionally available to Adam's own testing) is the
//! deliberately DISTINCT module this split produces — no shared record
//! type, no shared store, per Adam's ruling.
//!
//! Two record streams exist and must not be conflated:
//! - The SESSION event log (T1 store) is operational data — the
//!   designer's own dialogue/undo history. Not this module.
//! - CAPTURE is corpus accrual for evaluation/training/audit — this
//!   module — and is gated by the Q9 charter (D17): until the charter
//!   is ratified and its reference is supplied, every capture call is
//!   SUPPRESSED (dropped, unrecoverable, deliberately).
//!
//! Dataset separation (charter deliverable): evaluation, training, and
//! audit are PHYSICALLY distinct sinks — one event goes to exactly one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy::DecisionRecord;

/// The ratified charter reference (EOP-GOV-Q9-CHARTER-001, ratified by
/// Adam 2026-08-06). `under_ratified_charter` accepts EXACTLY this
/// string — a stale or amended reference is refused, per the charter's
/// §10.4 (capture under a stale reference is a defect). Amending the
/// charter means bumping this constant in the same change.
pub const RATIFIED_CHARTER_REF: &str = "Q9-CHARTER-001@v1.0";

/// Charter-mandated dataset separation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DatasetClass {
    Evaluation,
    Training,
    Audit,
}

/// One captured interaction: the full I28 closure plus the raw
/// utterance (permitted-fields/redaction rules are charter deliverables
/// applied at the sink when capture goes live).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub raw_utterance: String,
    pub record: DecisionRecord,
    pub dataset: DatasetClass,
}

/// What happened to a capture call — the caller always learns the
/// truth; suppression is visible, never silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Switch OFF (no ratified charter): event DROPPED by design.
    SuppressedNoCharter,
    /// Stored under the named dataset class.
    Stored(DatasetClass),
    /// Durable persistence failed: the event was NOT stored anywhere
    /// (memory included) and the caller is told so — a persist failure
    /// is never a silent success and never a silent drop.
    PersistFailed(DatasetClass),
}

/// The pipeline. `off()` is the only zero-argument constructor; turning
/// capture ON requires the ratified charter's reference — a mechanism
/// gate, not a boolean. Note: even under `q9-capture`, this type alone
/// does not make capture live anywhere — no callsite in this workspace
/// calls `on_under_charter` (grep before assuming otherwise).
pub struct CapturePipeline {
    /// `None` = switch OFF. `Some(charter_ref)` = ON under that charter.
    charter: Option<String>,
    /// Physically separate sinks (in-memory view; when `durable_dir` is
    /// set, every stored event is ALSO appended to that class's JSONL
    /// file before the in-memory push — persist-first, fail closed).
    sinks: BTreeMap<DatasetClass, Vec<CaptureEvent>>,
    /// `Some` = live charter-governed capture writes one append-only
    /// JSONL file per dataset class under this directory
    /// (`evaluation.jsonl` / `training.jsonl` / `audit.jsonl`) —
    /// physical separation on disk, mirroring the in-memory sinks.
    durable_dir: Option<PathBuf>,
}

/// One durable JSONL line: the event plus its charter lineage
/// (charter §7 — every stored event names the charter it was captured
/// under).
#[derive(Serialize, Deserialize)]
struct DurableCaptureLine {
    charter: String,
    captured_at_epoch_s: u64,
    event: CaptureEvent,
}

impl CapturePipeline {
    /// The default and only pre-charter state.
    pub fn off() -> Self {
        CapturePipeline {
            charter: None,
            sinks: BTreeMap::new(),
            durable_dir: None,
        }
    }

    /// Live, durable, charter-governed capture — the ONLY public way to
    /// turn capture on (EOP-GOV-Q9-CHARTER-001 §10.3, ratified
    /// 2026-08-06). The reference must equal [`RATIFIED_CHARTER_REF`]
    /// exactly: an empty, stale, or amended reference is refused (§10.4
    /// — a mechanism gate, not a string-presence check). The capture
    /// directory is created up front; failure to create it refuses
    /// construction (fail closed at startup, not at first event).
    pub fn under_ratified_charter(charter_ref: &str, capture_dir: &Path) -> anyhow::Result<Self> {
        let r = charter_ref.trim();
        if r != RATIFIED_CHARTER_REF {
            return Err(anyhow::anyhow!(
                "capture refused: charter reference {r:?} is not the ratified \
                 {RATIFIED_CHARTER_REF:?} (EOP-GOV-Q9-CHARTER-001 §10.4)"
            ));
        }
        std::fs::create_dir_all(capture_dir).map_err(|e| {
            anyhow::anyhow!(
                "capture refused: cannot create capture directory {}: {e}",
                capture_dir.display()
            )
        })?;
        Ok(CapturePipeline {
            charter: Some(r.to_owned()),
            sinks: BTreeMap::new(),
            durable_dir: Some(capture_dir.to_owned()),
        })
    }

    /// Turning capture on REQUIRES the ratified charter reference
    /// (D17). Empty/whitespace refs are refused — the gate cannot be
    /// satisfied by a placeholder.
    ///
    /// Deliberately `pub(crate)`, narrower than `off()`/`capture()`:
    /// no callsite outside this crate's own tests may flip capture on.
    /// External consumers (`bpmn-lite-server-designer`) construct only
    /// via `off()` — see this module's doc comment.
    pub(crate) fn on_under_charter(charter_ref: &str) -> anyhow::Result<Self> {
        let r = charter_ref.trim();
        if r.is_empty() {
            return Err(anyhow::anyhow!(
                "capture cannot be enabled without a ratified Q9 charter reference (D17)"
            ));
        }
        Ok(CapturePipeline {
            charter: Some(r.to_owned()),
            sinks: BTreeMap::new(),
            durable_dir: None,
        })
    }

    pub(crate) fn charter_ref(&self) -> Option<&str> {
        self.charter.as_deref()
    }

    /// Capture one interaction. OFF → the event is dropped and the
    /// caller told so. ON → stored under exactly one dataset class.
    pub fn capture(&mut self, event: CaptureEvent) -> CaptureOutcome {
        match self.charter.clone() {
            None => CaptureOutcome::SuppressedNoCharter,
            Some(charter) => {
                let class = event.dataset;
                // Persist FIRST: an event that cannot be durably written
                // is not stored anywhere and the caller is told so.
                if let Some(dir) = &self.durable_dir {
                    if let Err(e) = append_durable(dir, &charter, &event) {
                        tracing::error!(
                            class = ?class,
                            "q9 capture persist failed (event NOT stored): {e}"
                        );
                        return CaptureOutcome::PersistFailed(class);
                    }
                }
                self.sinks.entry(class).or_default().push(event);
                CaptureOutcome::Stored(class)
            }
        }
    }

    /// Read one dataset — the separation surface (a Training reader can
    /// never see Evaluation events and vice versa).
    pub(crate) fn dataset(&self, class: DatasetClass) -> &[CaptureEvent] {
        self.sinks.get(&class).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Per-class file name — the on-disk half of the physical dataset
/// separation. One class, one file; never a shared stream.
fn class_file_name(class: DatasetClass) -> &'static str {
    match class {
        DatasetClass::Evaluation => "evaluation.jsonl",
        DatasetClass::Training => "training.jsonl",
        DatasetClass::Audit => "audit.jsonl",
    }
}

fn append_durable(dir: &Path, charter: &str, event: &CaptureEvent) -> anyhow::Result<()> {
    let line = DurableCaptureLine {
        charter: charter.to_owned(),
        captured_at_epoch_s: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        event: event.clone(),
    };
    let mut serialized = serde_json::to_string(&line)?;
    serialized.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(class_file_name(event.dataset)))?;
    file.write_all(serialized.as_bytes())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::policy::decide;
    use crate::policy::DispositionConfig;
    use crate::retrieval::{LexicalTier0, Tier0Retriever};
    use designer_graph::board_candidate::{LegalityOracle, OperationKind, ProductionId};

    struct AllLegal;
    impl LegalityOracle for AllLegal {
        type NodeKey = ();
        fn legal_operations(&self, _: Option<&()>) -> Vec<OperationKind> {
            OperationKind::ALL.to_vec()
        }
        fn legal_productions(&self, _: Option<&()>) -> Vec<ProductionId> {
            ProductionId::ALL.to_vec()
        }
    }

    fn one_event() -> CaptureEvent {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let ev = LexicalTier0.retrieve("connect the nodes", &board).unwrap();
        let (_, record) =
            decide(&DispositionConfig::shadow_v1(), &board, &ev, &crate::context::minimal("pack.none", "g-test")).unwrap();
        CaptureEvent {
            raw_utterance: "connect the nodes".into(),
            record,
            dataset: DatasetClass::Evaluation,
        }
    }

    /// D17 red: switch OFF drops the event — visibly, unrecoverably.
    /// Charter gate red: empty ref refused.
    #[test]
    fn off_drops_and_charterless_on_is_refused() {
        let mut p = CapturePipeline::off();
        assert_eq!(p.capture(one_event()), CaptureOutcome::SuppressedNoCharter);
        assert!(p.dataset(DatasetClass::Evaluation).is_empty(), "nothing may persist");
        assert!(CapturePipeline::on_under_charter("   ").is_err());
    }

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("q9cap-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// §10.4 red/green: the public constructor is an exact-reference
    /// mechanism gate — stale, amended, or arbitrary non-empty strings
    /// are refused; only the ratified reference constructs.
    #[test]
    fn only_the_exact_ratified_charter_reference_constructs() {
        let dir = scratch_dir("ref");
        assert!(CapturePipeline::under_ratified_charter("", &dir).is_err());
        assert!(CapturePipeline::under_ratified_charter("Q9-CHARTER-001@v0.9", &dir).is_err());
        assert!(
            CapturePipeline::under_ratified_charter("some ratified charter", &dir).is_err(),
            "non-empty is not enough — the gate is the exact reference, not string presence"
        );
        assert!(CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Charter §3/§5/§7: a stored event lands in EXACTLY one class file,
    /// carries its charter lineage, and round-trips; the other class
    /// files do not even exist.
    #[test]
    fn durable_capture_writes_exactly_one_class_file_with_lineage() {
        let dir = scratch_dir("durable");
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        let event = one_event();
        assert_eq!(p.capture(event.clone()), CaptureOutcome::Stored(DatasetClass::Evaluation));

        let eval = std::fs::read_to_string(dir.join("evaluation.jsonl")).unwrap();
        let lines: Vec<&str> = eval.lines().collect();
        assert_eq!(lines.len(), 1);
        let parsed: DurableCaptureLine = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.charter, RATIFIED_CHARTER_REF);
        assert_eq!(parsed.event.raw_utterance, event.raw_utterance);
        assert_eq!(parsed.event.dataset, DatasetClass::Evaluation);

        assert!(!dir.join("training.jsonl").exists(), "training sink must not exist");
        assert!(!dir.join("audit.jsonl").exists(), "audit sink must not exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fail closed: when durable persistence cannot happen, the event is
    /// stored NOWHERE (memory included) and the caller is told.
    #[test]
    fn persist_failure_stores_nothing_and_is_visible() {
        let dir = scratch_dir("fail");
        let mut p = CapturePipeline::under_ratified_charter(RATIFIED_CHARTER_REF, &dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        // Make the path un-creatable: a FILE where the directory was.
        std::fs::write(&dir, b"not a directory").unwrap();
        assert_eq!(
            p.capture(one_event()),
            CaptureOutcome::PersistFailed(DatasetClass::Evaluation)
        );
        assert!(p.dataset(DatasetClass::Evaluation).is_empty(), "nothing may be half-stored");
        let _ = std::fs::remove_file(&dir);
    }

    /// Green under a charter ref + physical dataset separation.
    #[test]
    fn on_stores_with_dataset_separation() {
        let mut p = CapturePipeline::on_under_charter("Q9-CHARTER-TEST-REF").unwrap();
        let mut train = one_event();
        train.dataset = DatasetClass::Training;
        assert_eq!(p.capture(one_event()), CaptureOutcome::Stored(DatasetClass::Evaluation));
        assert_eq!(p.capture(train), CaptureOutcome::Stored(DatasetClass::Training));
        assert_eq!(p.dataset(DatasetClass::Evaluation).len(), 1);
        assert_eq!(p.dataset(DatasetClass::Training).len(), 1);
        assert!(p.dataset(DatasetClass::Audit).is_empty());
    }
}
