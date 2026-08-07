//! Private, explicit inputs for deterministic BPMN gameboard enumeration.

use designer_graph::schema::{DesignerDag, NodeKey};
use semantic_decision_contracts::SemanticDecisionBoard;

use crate::bpmn_board::BpmnBoardError;

/// A borrowed, side-effect-free view of the authoritative graph and admitted
/// semantic board. It owns no clock, identity source, model, database or server.
pub(crate) struct BpmnGameState<'a> {
    dag: &'a DesignerDag,
    board: &'a SemanticDecisionBoard,
}

impl<'a> BpmnGameState<'a> {
    pub(crate) fn new(
        dag: &'a DesignerDag,
        board: &'a SemanticDecisionBoard,
    ) -> Result<Self, BpmnBoardError> {
        if let Some(anchor) = board.position.anchor.as_deref() {
            if dag.key_for_bpmn_id(anchor).is_none() {
                return Err(BpmnBoardError::StaleBoardAnchor {
                    bpmn_id: anchor.to_string(),
                });
            }
        }
        Ok(Self { dag, board })
    }

    pub(crate) fn dag(&self) -> &'a DesignerDag {
        self.dag
    }

    pub(crate) fn board(&self) -> &'a SemanticDecisionBoard {
        self.board
    }

    /// Enumerate concrete anchors in canonical BPMN-id order. An explicitly
    /// anchored semantic board stays at that anchor; a whole-graph board expands
    /// over every graph element instead of deduplicating away position.
    pub(crate) fn anchors(&self) -> Result<Vec<(NodeKey, String)>, BpmnBoardError> {
        if let Some(anchor) = self.board.position.anchor.as_deref() {
            let key = self.dag.key_for_bpmn_id(anchor).ok_or_else(|| {
                BpmnBoardError::StaleBoardAnchor {
                    bpmn_id: anchor.to_string(),
                }
            })?;
            return Ok(vec![(key, anchor.to_string())]);
        }

        let ir = self
            .dag
            .to_ir()
            .map_err(|error| BpmnBoardError::GraphProjection(error.to_string()))?;
        let mut anchors = ir
            .node_weights()
            .map(|node| node.id().to_string())
            .map(|bpmn_id| {
                self.dag
                    .key_for_bpmn_id(&bpmn_id)
                    .map(|key| (key, bpmn_id.clone()))
                    .ok_or(BpmnBoardError::StaleBoardAnchor { bpmn_id })
            })
            .collect::<Result<Vec<_>, _>>()?;
        anchors.sort_by(|left, right| left.1.cmp(&right.1));
        Ok(anchors)
    }
}
