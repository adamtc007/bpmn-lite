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

use serde::{Deserialize, Serialize};

use crate::policy::DecisionRecord;

/// Charter-mandated dataset separation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum DatasetClass {
    Evaluation,
    Training,
    Audit,
}

/// One captured interaction: the full I28 closure plus the raw
/// utterance (permitted-fields/redaction rules are charter deliverables
/// applied at the sink when capture goes live).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CaptureEvent {
    pub raw_utterance: String,
    pub record: DecisionRecord,
    pub dataset: DatasetClass,
}

/// What happened to a capture call — the caller always learns the
/// truth; suppression is visible, never silent success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CaptureOutcome {
    /// Switch OFF (no ratified charter): event DROPPED by design.
    SuppressedNoCharter,
    /// Stored under the named dataset class.
    Stored(DatasetClass),
}

/// The pipeline. `off()` is the only zero-argument constructor; turning
/// capture ON requires the ratified charter's reference — a mechanism
/// gate, not a boolean. Note: even under `q9-capture`, this type alone
/// does not make capture live anywhere — no callsite in this workspace
/// calls `on_under_charter` (grep before assuming otherwise).
pub(crate) struct CapturePipeline {
    /// `None` = switch OFF. `Some(charter_ref)` = ON under that charter.
    charter: Option<String>,
    /// Physically separate sinks (in-memory v1; durable impls land with
    /// live capture and inherit the same separation).
    sinks: BTreeMap<DatasetClass, Vec<CaptureEvent>>,
}

impl CapturePipeline {
    /// The default and only pre-charter state.
    pub(crate) fn off() -> Self {
        CapturePipeline {
            charter: None,
            sinks: BTreeMap::new(),
        }
    }

    /// Turning capture on REQUIRES the ratified charter reference
    /// (D17). Empty/whitespace refs are refused — the gate cannot be
    /// satisfied by a placeholder.
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
        })
    }

    pub(crate) fn charter_ref(&self) -> Option<&str> {
        self.charter.as_deref()
    }

    /// Capture one interaction. OFF → the event is dropped and the
    /// caller told so. ON → stored under exactly one dataset class.
    pub(crate) fn capture(&mut self, event: CaptureEvent) -> CaptureOutcome {
        match &self.charter {
            None => CaptureOutcome::SuppressedNoCharter,
            Some(_) => {
                let class = event.dataset;
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
