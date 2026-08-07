//! Interpretable listwise conditional-logit baseline over complete move evidence.

use std::collections::{BTreeMap, BTreeSet};

use semantic_decision_contracts::{
    EvidenceLane, FiniteScore, GraphContentHash, HarmClass, LegalMoveId, MoveEvidence,
};
#[cfg(feature = "q9-capture")]
use semantic_decision_contracts::{GameTurnAdjudication, GameTurnRecord};
use serde::Serialize;
use sha2::{Digest, Sha256};

const MODEL_SCHEMA_VERSION: u32 = 1;
const CALIBRATION_SCHEMA_VERSION: u32 = 1;

/// Stable board-size strata used for Phase 6 calibration reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoardSizeBucket {
    Small,
    Medium,
    Large,
}

impl BoardSizeBucket {
    pub fn for_size(board_size: usize) -> anyhow::Result<Self> {
        match board_size {
            0 => anyhow::bail!("cannot calibrate an empty move board"),
            1..=4 => Ok(Self::Small),
            5..=12 => Ok(Self::Medium),
            _ => Ok(Self::Large),
        }
    }
}

/// Deterministic bounded optimiser configuration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructuredChoiceFitConfig {
    iterations: u32,
    learning_rate: FiniteScore,
    l2_penalty: FiniteScore,
    maximum_absolute_weight: FiniteScore,
}

impl StructuredChoiceFitConfig {
    pub fn new(
        iterations: u32,
        learning_rate: f64,
        l2_penalty: f64,
        maximum_absolute_weight: f64,
    ) -> anyhow::Result<Self> {
        if !(1..=10_000).contains(&iterations) {
            anyhow::bail!("structured-choice iterations must be within 1..=10000");
        }
        if !(0.0..=1.0).contains(&learning_rate) || learning_rate == 0.0 {
            anyhow::bail!("structured-choice learning rate must be within (0, 1]");
        }
        if !(0.0..=1.0).contains(&l2_penalty) {
            anyhow::bail!("structured-choice L2 penalty must be within [0, 1]");
        }
        if !(0.01..=32.0).contains(&maximum_absolute_weight) {
            anyhow::bail!("structured-choice weight bound must be within [0.01, 32]");
        }
        Ok(Self {
            iterations,
            learning_rate: FiniteScore::new(learning_rate)?,
            l2_penalty: FiniteScore::new(l2_penalty)?,
            maximum_absolute_weight: FiniteScore::new(maximum_absolute_weight)?,
        })
    }

    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    pub fn learning_rate(&self) -> FiniteScore {
        self.learning_rate
    }

    pub fn l2_penalty(&self) -> FiniteScore {
        self.l2_penalty
    }

    pub fn maximum_absolute_weight(&self) -> FiniteScore {
        self.maximum_absolute_weight
    }
}

/// One explicitly adjudicated listwise training observation.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredChoiceObservation {
    evidence: Vec<MoveEvidence>,
    chosen_move: LegalMoveId,
    risk_class: HarmClass,
}

impl StructuredChoiceObservation {
    /// Construct a positive observation only from the shared adjudication gate.
    /// Exploratory and accidental interactions return `Ok(None)` and cannot become
    /// positive labels through this API.
    #[cfg(feature = "q9-capture")]
    fn from_adjudicated(
        record: &GameTurnRecord,
        adjudication: &GameTurnAdjudication,
    ) -> anyhow::Result<Option<Self>> {
        adjudication.validate_for_record(record)?;
        let Some(chosen_move) = adjudication.positive_label().cloned() else {
            return Ok(None);
        };
        Self::admit(record.evidence().to_vec(), chosen_move, record.risk_class()).map(Some)
    }

    /// Construct a positive fit row only when the record is assigned to the frozen
    /// training split. The split proof is part of the public admission path.
    #[cfg(feature = "q9-capture")]
    pub fn from_training(
        record: &GameTurnRecord,
        adjudication: &GameTurnAdjudication,
        assignment: &crate::funnel::RealTurnSplitAssignment,
    ) -> anyhow::Result<Option<Self>> {
        if assignment.record_hash() != record.record_hash().as_str()
            || assignment.split() != crate::funnel::RealTurnSplit::Training
        {
            anyhow::bail!("structured-choice fitting requires the frozen training split");
        }
        Self::from_adjudicated(record, adjudication)
    }

    #[cfg(any(feature = "q9-capture", test))]
    fn admit(
        mut evidence: Vec<MoveEvidence>,
        chosen_move: LegalMoveId,
        risk_class: HarmClass,
    ) -> anyhow::Result<Self> {
        if evidence.is_empty() {
            anyhow::bail!("structured-choice observation has an empty move board");
        }
        evidence.sort_by(|left, right| left.move_id().cmp(right.move_id()));
        if evidence
            .windows(2)
            .any(|pair| pair[0].move_id() == pair[1].move_id())
        {
            anyhow::bail!("structured-choice observation contains duplicate moves");
        }
        if !evidence.iter().any(|item| item.move_id() == &chosen_move) {
            anyhow::bail!("structured-choice positive label is absent from the move board");
        }
        for item in &evidence {
            let lanes = item
                .lanes()
                .iter()
                .map(|lane| lane.lane)
                .collect::<BTreeSet<_>>();
            if lanes != EvidenceLane::ALL.into_iter().collect() {
                anyhow::bail!("structured-choice observation requires every Phase 3 lane");
            }
        }
        Ok(Self {
            evidence,
            chosen_move,
            risk_class,
        })
    }

    pub fn evidence(&self) -> &[MoveEvidence] {
        &self.evidence
    }

    pub fn chosen_move(&self) -> &LegalMoveId {
        &self.chosen_move
    }

    pub fn risk_class(&self) -> HarmClass {
        self.risk_class
    }
}

/// One full-board structured-choice score.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructuredChoiceScore {
    move_id: LegalMoveId,
    utility: FiniteScore,
    probability: FiniteScore,
}

/// One positive adjudicated observation proven to belong to the frozen validation split.
#[derive(Clone, Debug, PartialEq)]
pub struct StructuredChoiceCalibrationObservation {
    record_hash: String,
    observation: StructuredChoiceObservation,
}

impl StructuredChoiceCalibrationObservation {
    #[cfg(feature = "q9-capture")]
    pub fn from_validation(
        record: &GameTurnRecord,
        adjudication: &GameTurnAdjudication,
        assignment: &crate::funnel::RealTurnSplitAssignment,
    ) -> anyhow::Result<Option<Self>> {
        if assignment.record_hash() != record.record_hash().as_str()
            || assignment.split() != crate::funnel::RealTurnSplit::Validation
        {
            anyhow::bail!("calibration observations must match the frozen validation split");
        }
        let Some(observation) =
            StructuredChoiceObservation::from_adjudicated(record, adjudication)?
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            record_hash: record.record_hash().as_str().to_string(),
            observation,
        }))
    }

    #[cfg(test)]
    fn admit(
        record_hash: String,
        observation: StructuredChoiceObservation,
    ) -> anyhow::Result<Self> {
        GraphContentHash::new(record_hash.clone())?;
        Ok(Self {
            record_hash,
            observation,
        })
    }

    pub fn record_hash(&self) -> &str {
        &self.record_hash
    }

    pub fn observation(&self) -> &StructuredChoiceObservation {
        &self.observation
    }
}

/// One fitted temperature for an exact risk/board-size stratum.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructuredChoiceCalibrationCell {
    risk_class: HarmClass,
    board_size: BoardSizeBucket,
    observations: usize,
    temperature: FiniteScore,
    mean_negative_log_likelihood: FiniteScore,
}

impl StructuredChoiceCalibrationCell {
    pub fn risk_class(&self) -> HarmClass {
        self.risk_class
    }

    pub fn board_size(&self) -> BoardSizeBucket {
        self.board_size
    }

    pub fn observations(&self) -> usize {
        self.observations
    }

    pub fn temperature(&self) -> FiniteScore {
        self.temperature
    }

    pub fn mean_negative_log_likelihood(&self) -> FiniteScore {
        self.mean_negative_log_likelihood
    }
}

/// Validation-only calibration bound to one structured-choice model identity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructuredChoiceCalibration {
    schema_version: u32,
    model_hash: GraphContentHash,
    validation_data_hash: GraphContentHash,
    cells: Vec<StructuredChoiceCalibrationCell>,
    calibration_hash: GraphContentHash,
}

impl StructuredChoiceCalibration {
    /// Fit deterministic temperatures independently by risk class and board-size bucket.
    pub fn fit_validation(
        model: &StructuredChoiceModel,
        observations: &[StructuredChoiceCalibrationObservation],
    ) -> anyhow::Result<Self> {
        if observations.is_empty() {
            anyhow::bail!("structured-choice calibration requires validation observations");
        }
        let mut canonical = observations.to_vec();
        canonical.sort_by(|left, right| left.record_hash.cmp(&right.record_hash));
        if canonical
            .windows(2)
            .any(|pair| pair[0].record_hash == pair[1].record_hash)
        {
            anyhow::bail!("structured-choice calibration received a duplicate record");
        }
        let validation_data_hash = GraphContentHash::new(hash_strings(
            "semantic-gameboard-structured-choice-validation-data-v1",
            &canonical
                .iter()
                .map(|item| {
                    format!(
                        "{}:{}",
                        item.record_hash,
                        observation_hash(&item.observation)
                    )
                })
                .collect::<Vec<_>>(),
        ))?;
        let mut groups =
            BTreeMap::<(u8, BoardSizeBucket), Vec<&StructuredChoiceObservation>>::new();
        for item in &canonical {
            groups
                .entry((
                    risk_index(item.observation.risk_class),
                    BoardSizeBucket::for_size(item.observation.evidence.len())?,
                ))
                .or_default()
                .push(&item.observation);
        }
        let mut cells = Vec::with_capacity(groups.len());
        for ((risk, board_size), group) in groups {
            let (temperature, mean_negative_log_likelihood) = fit_temperature(model, &group)?;
            cells.push(StructuredChoiceCalibrationCell {
                risk_class: risk_from_index(risk),
                board_size,
                observations: group.len(),
                temperature: FiniteScore::new(temperature)?,
                mean_negative_log_likelihood: FiniteScore::new(mean_negative_log_likelihood)?,
            });
        }
        let mut calibration = Self {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            model_hash: model.model_hash.clone(),
            validation_data_hash,
            cells,
            calibration_hash: GraphContentHash::new("0".repeat(64))?,
        };
        calibration.calibration_hash = calibration.canonical_hash()?;
        Ok(calibration)
    }

    /// Apply only the exact admitted risk/size cell; an absent stratum fails closed.
    pub fn calibrate(
        &self,
        model: &StructuredChoiceModel,
        risk_class: HarmClass,
        scores: &[StructuredChoiceScore],
    ) -> anyhow::Result<Vec<StructuredChoiceScore>> {
        if self.model_hash != model.model_hash {
            anyhow::bail!("structured-choice calibration belongs to a different model");
        }
        let board_size = BoardSizeBucket::for_size(scores.len())?;
        let cell = self
            .cells
            .iter()
            .find(|cell| cell.risk_class == risk_class && cell.board_size == board_size)
            .ok_or_else(|| {
                anyhow::anyhow!("no validation calibration exists for this risk/size stratum")
            })?;
        let probabilities = softmax(
            &scores
                .iter()
                .map(|score| score.utility.get() / cell.temperature.get())
                .collect::<Vec<_>>(),
        )?;
        let mut calibrated = scores
            .iter()
            .zip(probabilities)
            .map(|(score, probability)| {
                Ok(StructuredChoiceScore {
                    move_id: score.move_id.clone(),
                    utility: score.utility,
                    probability: FiniteScore::new(probability)?,
                })
            })
            .collect::<Result<Vec<_>, semantic_decision_contracts::DecisionBoardError>>()?;
        calibrated.sort_by(|left, right| {
            right
                .probability
                .get()
                .total_cmp(&left.probability.get())
                .then_with(|| left.move_id.cmp(&right.move_id))
        });
        Ok(calibrated)
    }

    fn canonical_hash(&self) -> anyhow::Result<GraphContentHash> {
        #[derive(Serialize)]
        struct Preimage<'a> {
            schema_version: u32,
            model_hash: &'a GraphContentHash,
            validation_data_hash: &'a GraphContentHash,
            cells: &'a [StructuredChoiceCalibrationCell],
        }
        Ok(GraphContentHash::new(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&Preimage {
                schema_version: self.schema_version,
                model_hash: &self.model_hash,
                validation_data_hash: &self.validation_data_hash,
                cells: &self.cells,
            })?)
        ))?)
    }

    pub fn model_hash(&self) -> &GraphContentHash {
        &self.model_hash
    }

    pub fn validation_data_hash(&self) -> &GraphContentHash {
        &self.validation_data_hash
    }

    pub fn cells(&self) -> &[StructuredChoiceCalibrationCell] {
        &self.cells
    }

    pub fn calibration_hash(&self) -> &GraphContentHash {
        &self.calibration_hash
    }
}

impl StructuredChoiceScore {
    pub fn move_id(&self) -> &LegalMoveId {
        &self.move_id
    }

    pub fn utility(&self) -> FiniteScore {
        self.utility
    }

    pub fn probability(&self) -> FiniteScore {
        self.probability
    }
}

/// Interpretable conditional-logit weights over the closed evidence-lane set.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StructuredChoiceModel {
    schema_version: u32,
    weights: BTreeMap<EvidenceLane, FiniteScore>,
    fit_config: StructuredChoiceFitConfig,
    training_observations: usize,
    training_data_hash: GraphContentHash,
    model_hash: GraphContentHash,
}

impl StructuredChoiceModel {
    /// Fit deterministic full-board conditional log likelihood with bounded batch
    /// gradient ascent and L2 regularisation.
    pub fn fit(
        observations: &[StructuredChoiceObservation],
        config: StructuredChoiceFitConfig,
    ) -> anyhow::Result<Self> {
        if observations.is_empty() {
            anyhow::bail!("structured-choice fitting requires adjudicated observations");
        }
        let mut canonical = observations.to_vec();
        canonical.sort_by(|left, right| {
            left.chosen_move
                .cmp(&right.chosen_move)
                .then_with(|| observation_hash(left).cmp(&observation_hash(right)))
        });
        let mut weights = EvidenceLane::ALL
            .into_iter()
            .map(|lane| (lane, 0.0_f64))
            .collect::<BTreeMap<_, _>>();
        for _ in 0..config.iterations {
            let mut gradient = EvidenceLane::ALL
                .into_iter()
                .map(|lane| (lane, 0.0_f64))
                .collect::<BTreeMap<_, _>>();
            for observation in &canonical {
                let utilities = observation
                    .evidence
                    .iter()
                    .map(|item| utility(&weights, item))
                    .collect::<Vec<_>>();
                let probabilities = softmax(&utilities)?;
                for (item, probability) in observation.evidence.iter().zip(probabilities) {
                    let target = f64::from(item.move_id() == &observation.chosen_move);
                    for lane in item.lanes() {
                        gradient.entry(lane.lane).and_modify(|value| {
                            *value += (target - probability) * lane.score.get()
                        });
                    }
                }
            }
            let scale = 1.0 / canonical.len() as f64;
            for lane in EvidenceLane::ALL {
                let current = weights[&lane];
                let update = config.learning_rate.get()
                    * (gradient[&lane] * scale - config.l2_penalty.get() * current);
                weights.insert(
                    lane,
                    (current + update).clamp(
                        -config.maximum_absolute_weight.get(),
                        config.maximum_absolute_weight.get(),
                    ),
                );
            }
        }
        let weights = weights
            .into_iter()
            .map(|(lane, weight)| Ok((lane, FiniteScore::new(weight)?)))
            .collect::<Result<BTreeMap<_, _>, semantic_decision_contracts::DecisionBoardError>>()?;
        let observation_hashes = canonical.iter().map(observation_hash).collect::<Vec<_>>();
        let training_data_hash = GraphContentHash::new(hash_strings(
            "semantic-gameboard-structured-choice-training-data-v1",
            &observation_hashes,
        ))?;
        let model_hash = model_hash(&weights, &config, canonical.len(), &training_data_hash)?;
        Ok(Self {
            schema_version: MODEL_SCHEMA_VERSION,
            weights,
            fit_config: config,
            training_observations: canonical.len(),
            training_data_hash,
            model_hash,
        })
    }

    /// Score every supplied move exactly once. Candidate ordering cannot change the
    /// returned canonical probability ranking.
    pub fn score(&self, evidence: &[MoveEvidence]) -> anyhow::Result<Vec<StructuredChoiceScore>> {
        if evidence.is_empty() {
            anyhow::bail!("structured-choice scoring requires a non-empty move board");
        }
        let mut canonical = evidence.to_vec();
        canonical.sort_by(|left, right| left.move_id().cmp(right.move_id()));
        if canonical
            .windows(2)
            .any(|pair| pair[0].move_id() == pair[1].move_id())
        {
            anyhow::bail!("structured-choice scoring received duplicate moves");
        }
        for item in &canonical {
            let lanes = item
                .lanes()
                .iter()
                .map(|lane| lane.lane)
                .collect::<BTreeSet<_>>();
            if lanes != EvidenceLane::ALL.into_iter().collect() {
                anyhow::bail!("structured-choice scoring requires every Phase 3 lane");
            }
        }
        let utilities = canonical
            .iter()
            .map(|item| utility_finite(&self.weights, item))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let probabilities = softmax(
            &utilities
                .iter()
                .map(|value| value.get())
                .collect::<Vec<_>>(),
        )?;
        let mut scores = canonical
            .iter()
            .zip(utilities)
            .zip(probabilities)
            .map(|((item, utility), probability)| {
                Ok(StructuredChoiceScore {
                    move_id: item.move_id().clone(),
                    utility,
                    probability: FiniteScore::new(probability)?,
                })
            })
            .collect::<Result<Vec<_>, semantic_decision_contracts::DecisionBoardError>>()?;
        scores.sort_by(|left, right| {
            right
                .probability
                .get()
                .total_cmp(&left.probability.get())
                .then_with(|| left.move_id.cmp(&right.move_id))
        });
        Ok(scores)
    }

    pub fn weights(&self) -> &BTreeMap<EvidenceLane, FiniteScore> {
        &self.weights
    }

    pub fn fit_config(&self) -> &StructuredChoiceFitConfig {
        &self.fit_config
    }

    pub fn training_observations(&self) -> usize {
        self.training_observations
    }

    pub fn training_data_hash(&self) -> &GraphContentHash {
        &self.training_data_hash
    }

    pub fn model_hash(&self) -> &GraphContentHash {
        &self.model_hash
    }
}

fn utility(weights: &BTreeMap<EvidenceLane, f64>, evidence: &MoveEvidence) -> f64 {
    evidence
        .lanes()
        .iter()
        .map(|lane| weights[&lane.lane] * lane.score.get())
        .sum()
}

fn utility_finite(
    weights: &BTreeMap<EvidenceLane, FiniteScore>,
    evidence: &MoveEvidence,
) -> anyhow::Result<FiniteScore> {
    Ok(FiniteScore::new(
        evidence
            .lanes()
            .iter()
            .map(|lane| weights[&lane.lane].get() * lane.score.get())
            .sum(),
    )?)
}

fn softmax(utilities: &[f64]) -> anyhow::Result<Vec<f64>> {
    if utilities.is_empty() || utilities.iter().any(|value| !value.is_finite()) {
        anyhow::bail!("structured-choice softmax requires finite non-empty utilities");
    }
    let maximum = utilities.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let exponentials = utilities
        .iter()
        .map(|utility| (utility - maximum).exp())
        .collect::<Vec<_>>();
    let sum = exponentials.iter().sum::<f64>();
    if !sum.is_finite() || sum <= 0.0 {
        anyhow::bail!("structured-choice softmax normalization failed");
    }
    Ok(exponentials.into_iter().map(|value| value / sum).collect())
}

fn fit_temperature(
    model: &StructuredChoiceModel,
    observations: &[&StructuredChoiceObservation],
) -> anyhow::Result<(f64, f64)> {
    let mut best = None::<(f64, f64)>;
    for step in 0..=75 {
        let temperature = 0.25 + f64::from(step) * 0.05;
        let loss = observations
            .iter()
            .map(|observation| {
                let scores = model.score(observation.evidence())?;
                let probabilities = softmax(
                    &scores
                        .iter()
                        .map(|score| score.utility.get() / temperature)
                        .collect::<Vec<_>>(),
                )?;
                let probability = scores
                    .iter()
                    .zip(probabilities)
                    .find(|(score, _)| score.move_id() == observation.chosen_move())
                    .map(|(_, probability)| probability)
                    .ok_or_else(|| anyhow::anyhow!("calibration label is absent from its board"))?;
                Ok(-probability.max(f64::MIN_POSITIVE).ln())
            })
            .collect::<anyhow::Result<Vec<_>>>()?
            .into_iter()
            .sum::<f64>()
            / observations.len() as f64;
        if best.is_none_or(|(best_loss, _)| loss < best_loss) {
            best = Some((loss, temperature));
        }
    }
    let (loss, temperature) = best.expect("the fixed temperature grid is non-empty");
    Ok((temperature, loss))
}

fn risk_index(risk: HarmClass) -> u8 {
    match risk {
        HarmClass::ReadOnly => 0,
        HarmClass::Reversible => 1,
        HarmClass::Irreversible => 2,
        HarmClass::Destructive => 3,
    }
}

fn risk_from_index(index: u8) -> HarmClass {
    match index {
        0 => HarmClass::ReadOnly,
        1 => HarmClass::Reversible,
        2 => HarmClass::Irreversible,
        3 => HarmClass::Destructive,
        _ => unreachable!("risk indices are constructed by risk_index"),
    }
}

fn observation_hash(observation: &StructuredChoiceObservation) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"semantic-gameboard-structured-choice-observation-v1");
    hasher.update(observation.chosen_move.as_str().as_bytes());
    hasher.update(format!("{:?}", observation.risk_class).as_bytes());
    for item in &observation.evidence {
        hasher.update(item.evidence_hash().as_str().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn model_hash(
    weights: &BTreeMap<EvidenceLane, FiniteScore>,
    config: &StructuredChoiceFitConfig,
    observations: usize,
    training_data_hash: &GraphContentHash,
) -> anyhow::Result<GraphContentHash> {
    #[derive(Serialize)]
    struct Preimage<'a> {
        schema_version: u32,
        weights: &'a BTreeMap<EvidenceLane, FiniteScore>,
        config: &'a StructuredChoiceFitConfig,
        observations: usize,
        training_data_hash: &'a GraphContentHash,
    }
    Ok(GraphContentHash::new(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&Preimage {
            schema_version: MODEL_SCHEMA_VERSION,
            weights,
            config,
            observations,
            training_data_hash,
        })?)
    ))?)
}

fn hash_strings(domain: &str, values: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    for value in values {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use semantic_decision_contracts::{LaneScore, ProducerIdentity, GAMEBOARD_SCHEMA_VERSION};

    use super::*;

    fn evidence(move_byte: char, exact: f64) -> MoveEvidence {
        MoveEvidence::new(
            GAMEBOARD_SCHEMA_VERSION,
            LegalMoveId::new(move_byte.to_string().repeat(64)).unwrap(),
            EvidenceLane::ALL
                .into_iter()
                .map(|lane| LaneScore {
                    lane,
                    score: FiniteScore::new(if lane == EvidenceLane::GovernedExact {
                        exact
                    } else {
                        0.0
                    })
                    .unwrap(),
                })
                .collect(),
            FiniteScore::new(exact).unwrap(),
            FiniteScore::new(0.5).unwrap(),
            Vec::new(),
            ProducerIdentity::new("test.phase3-evidence.v1").unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn conditional_logit_learns_an_interpretable_lane_and_scores_full_board() {
        let chosen = LegalMoveId::new("a".repeat(64)).unwrap();
        let observation = StructuredChoiceObservation::admit(
            vec![evidence('a', 1.0), evidence('b', 0.0)],
            chosen.clone(),
            HarmClass::Reversible,
        )
        .unwrap();
        let config = StructuredChoiceFitConfig::new(200, 0.2, 0.01, 8.0).unwrap();
        let model = StructuredChoiceModel::fit(std::slice::from_ref(&observation), config).unwrap();
        assert!(model.weights()[&EvidenceLane::GovernedExact].get() > 0.0);
        assert_eq!(model.training_observations(), 1);
        let scores = model.score(observation.evidence()).unwrap();
        assert_eq!(scores.len(), 2);
        assert_eq!(scores[0].move_id(), &chosen);
        assert!(
            (scores
                .iter()
                .map(|score| score.probability().get())
                .sum::<f64>()
                - 1.0)
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn fitting_and_scoring_are_candidate_and_observation_order_invariant() {
        let first = StructuredChoiceObservation::admit(
            vec![evidence('a', 1.0), evidence('b', 0.0)],
            LegalMoveId::new("a".repeat(64)).unwrap(),
            HarmClass::Reversible,
        )
        .unwrap();
        let second = StructuredChoiceObservation::admit(
            vec![evidence('a', 0.0), evidence('b', 1.0)],
            LegalMoveId::new("b".repeat(64)).unwrap(),
            HarmClass::Reversible,
        )
        .unwrap();
        let config = StructuredChoiceFitConfig::new(50, 0.1, 0.01, 8.0).unwrap();
        let model =
            StructuredChoiceModel::fit(&[first.clone(), second.clone()], config.clone()).unwrap();
        let replay = StructuredChoiceModel::fit(&[second, first.clone()], config).unwrap();
        assert_eq!(model, replay);
        let mut reversed = first.evidence().to_vec();
        reversed.reverse();
        assert_eq!(
            model.score(first.evidence()).unwrap(),
            model.score(&reversed).unwrap()
        );
    }

    #[test]
    fn calibration_is_validation_content_addressed_and_stratum_bound() {
        let training = StructuredChoiceObservation::admit(
            vec![evidence('a', 1.0), evidence('b', 0.0)],
            LegalMoveId::new("a".repeat(64)).unwrap(),
            HarmClass::Reversible,
        )
        .unwrap();
        let model = StructuredChoiceModel::fit(
            std::slice::from_ref(&training),
            StructuredChoiceFitConfig::new(50, 0.1, 0.01, 8.0).unwrap(),
        )
        .unwrap();
        let validation =
            StructuredChoiceCalibrationObservation::admit("c".repeat(64), training.clone())
                .unwrap();
        let calibration =
            StructuredChoiceCalibration::fit_validation(&model, std::slice::from_ref(&validation))
                .unwrap();
        let scores = model.score(training.evidence()).unwrap();
        let calibrated = calibration
            .calibrate(&model, HarmClass::Reversible, &scores)
            .unwrap();
        assert_eq!(calibrated.len(), scores.len());
        assert_eq!(calibration.cells().len(), 1);
        assert!(calibration
            .calibrate(&model, HarmClass::Destructive, &scores)
            .is_err());
        assert_eq!(
            calibration,
            StructuredChoiceCalibration::fit_validation(&model, &[validation]).unwrap()
        );
    }
}
