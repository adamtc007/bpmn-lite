//! Identical-board comparison packets for Phase 6 evidence producers.

use std::collections::{BTreeMap, BTreeSet};

use semantic_decision_contracts::{
    DesignPosition, DesignStateId, FiniteScore, GameTurnAdjudication, GameTurnRecord,
    GraphContentHash, HarmClass, LegalMoveId, MoveSetHash,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Product bound for any statistical full-board request.
pub const MAX_RESOLVER_BOARD_SIZE: usize = 256;
/// Maximum UTF-8 bytes admitted for the offline prompt/turn envelope.
pub const MAX_OFFLINE_PROMPT_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes admitted for one governed candidate description.
pub const MAX_OFFLINE_CANDIDATE_BYTES: usize = 4 * 1024;
/// Maximum tokenizer-observed tokens admitted for one offline full-board request.
pub const MAX_OFFLINE_TOKEN_BUDGET: usize = 8 * 1024;

/// The four Phase 6 resolver configurations compared on identical move boards.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolverVariant {
    DeterministicFusion,
    DeterministicFusionWithCandle,
    StructuredChoice,
    BoundedPromptOffline,
}

impl ResolverVariant {
    pub const ALL: [Self; 4] = [
        Self::DeterministicFusion,
        Self::DeterministicFusionWithCandle,
        Self::StructuredChoice,
        Self::BoundedPromptOffline,
    ];
}

/// Typed, bounded refusal at a statistical producer boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ResolverBoundaryRefusal {
    #[error("the legal move board is empty")]
    EmptyBoard,
    #[error("the legal move board exceeds the product candidate limit")]
    BoardTooLarge,
    #[error("the producer omitted one or more legal moves")]
    IncompleteBoard,
    #[error("the producer emitted a duplicate move")]
    DuplicateMove,
    #[error("the producer emitted a move outside the legal board")]
    ForeignMove,
    #[error("the producer emitted a non-finite score")]
    NonFiniteScore,
    #[error("the model refused the bounded request")]
    ModelRefusal,
    #[error("the request exceeded its declared token budget")]
    TokenBudgetExceeded,
    #[error("the prompt/turn envelope exceeded its declared memory bound")]
    OversizedPromptText,
    #[error("candidate text was empty")]
    EmptyCandidateText,
    #[error("candidate text exceeded its declared bound")]
    OversizedCandidateText,
    #[error("the model bundle and card identities did not match")]
    BundleCardMismatch,
    #[error("the producer failed without a ranking")]
    ProducerFailure,
}

/// One candidate in a bounded offline prompt-ranker request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BoundedOfflineCandidateText {
    move_id: LegalMoveId,
    text: String,
    observed_tokens: usize,
}

impl BoundedOfflineCandidateText {
    pub fn move_id(&self) -> &LegalMoveId {
        &self.move_id
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn observed_tokens(&self) -> usize {
        self.observed_tokens
    }
}

/// Canonical, resource-bounded request for an offline prompt evidence producer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BoundedOfflineRankerRequest {
    position_id: DesignStateId,
    move_set_hash: MoveSetHash,
    prompt_text: String,
    prompt_observed_tokens: usize,
    token_budget: usize,
    candidates: Vec<BoundedOfflineCandidateText>,
    model_bundle_hash: GraphContentHash,
    request_hash: GraphContentHash,
}

impl BoundedOfflineRankerRequest {
    /// Admit a full-board prompt request using token counts reported by the model's
    /// own tokenizer. Text remains valid UTF-8 and is otherwise language-agnostic.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        position: &DesignPosition,
        prompt_text: String,
        prompt_observed_tokens: usize,
        raw_candidates: Vec<(LegalMoveId, String, usize)>,
        token_budget: usize,
        model_bundle_hash: GraphContentHash,
        card_bundle_hash: GraphContentHash,
    ) -> Result<Self, ResolverBoundaryRefusal> {
        validate_board_bound(position)?;
        if model_bundle_hash != card_bundle_hash {
            return Err(ResolverBoundaryRefusal::BundleCardMismatch);
        }
        if prompt_text.len() > MAX_OFFLINE_PROMPT_BYTES {
            return Err(ResolverBoundaryRefusal::OversizedPromptText);
        }
        if token_budget == 0 || token_budget > MAX_OFFLINE_TOKEN_BUDGET {
            return Err(ResolverBoundaryRefusal::TokenBudgetExceeded);
        }
        let legal = position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut candidates = Vec::with_capacity(raw_candidates.len());
        let mut observed_tokens = prompt_observed_tokens;
        for (move_id, text, candidate_tokens) in raw_candidates {
            if !legal.contains(&move_id) {
                return Err(ResolverBoundaryRefusal::ForeignMove);
            }
            if !seen.insert(move_id.clone()) {
                return Err(ResolverBoundaryRefusal::DuplicateMove);
            }
            if text.trim().is_empty() || candidate_tokens == 0 {
                return Err(ResolverBoundaryRefusal::EmptyCandidateText);
            }
            if text.len() > MAX_OFFLINE_CANDIDATE_BYTES {
                return Err(ResolverBoundaryRefusal::OversizedCandidateText);
            }
            observed_tokens = observed_tokens
                .checked_add(candidate_tokens)
                .ok_or(ResolverBoundaryRefusal::TokenBudgetExceeded)?;
            candidates.push(BoundedOfflineCandidateText {
                move_id,
                text,
                observed_tokens: candidate_tokens,
            });
        }
        if seen != legal {
            return Err(ResolverBoundaryRefusal::IncompleteBoard);
        }
        if observed_tokens > token_budget {
            return Err(ResolverBoundaryRefusal::TokenBudgetExceeded);
        }
        candidates.sort_by(|left, right| left.move_id.cmp(&right.move_id));
        let mut request = Self {
            position_id: position.state_id().clone(),
            move_set_hash: position.move_set_hash().clone(),
            prompt_text,
            prompt_observed_tokens,
            token_budget,
            candidates,
            model_bundle_hash,
            request_hash: GraphContentHash::new("0".repeat(64))
                .map_err(|_| ResolverBoundaryRefusal::ProducerFailure)?,
        };
        request.request_hash = request
            .canonical_hash()
            .map_err(|_| ResolverBoundaryRefusal::ProducerFailure)?;
        Ok(request)
    }

    fn canonical_hash(&self) -> anyhow::Result<GraphContentHash> {
        #[derive(Serialize)]
        struct Preimage<'a> {
            schema: &'static str,
            position_id: &'a DesignStateId,
            move_set_hash: &'a MoveSetHash,
            prompt_text: &'a str,
            prompt_observed_tokens: usize,
            token_budget: usize,
            candidates: &'a [BoundedOfflineCandidateText],
            model_bundle_hash: &'a GraphContentHash,
        }
        Ok(GraphContentHash::new(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&Preimage {
                schema: "semantic-gameboard-bounded-offline-ranker-request-v1",
                position_id: &self.position_id,
                move_set_hash: &self.move_set_hash,
                prompt_text: &self.prompt_text,
                prompt_observed_tokens: self.prompt_observed_tokens,
                token_budget: self.token_budget,
                candidates: &self.candidates,
                model_bundle_hash: &self.model_bundle_hash,
            })?)
        ))?)
    }

    pub fn position_id(&self) -> &DesignStateId {
        &self.position_id
    }

    pub fn move_set_hash(&self) -> &MoveSetHash {
        &self.move_set_hash
    }

    pub fn prompt_text(&self) -> &str {
        &self.prompt_text
    }

    pub fn prompt_observed_tokens(&self) -> usize {
        self.prompt_observed_tokens
    }

    pub fn token_budget(&self) -> usize {
        self.token_budget
    }

    pub fn candidates(&self) -> &[BoundedOfflineCandidateText] {
        &self.candidates
    }

    pub fn model_bundle_hash(&self) -> &GraphContentHash {
        &self.model_bundle_hash
    }

    pub fn request_hash(&self) -> &GraphContentHash {
        &self.request_hash
    }
}

/// One admitted move score. Construction is only available through a full-board packet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolverCandidateScore {
    move_id: LegalMoveId,
    score: FiniteScore,
}

impl ResolverCandidateScore {
    pub fn move_id(&self) -> &LegalMoveId {
        &self.move_id
    }

    pub fn score(&self) -> FiniteScore {
        self.score
    }
}

/// Position-bound output from one evidence producer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolverBoardPacket {
    variant: ResolverVariant,
    position_id: DesignStateId,
    move_set_hash: MoveSetHash,
    board_size: usize,
    scores: Vec<ResolverCandidateScore>,
    refusal: Option<ResolverBoundaryRefusal>,
    packet_hash: GraphContentHash,
}

impl ResolverBoardPacket {
    /// Admit raw scores only when they cover the complete legal move set exactly once.
    pub fn ranked(
        position: &DesignPosition,
        variant: ResolverVariant,
        raw_scores: Vec<(LegalMoveId, f64)>,
    ) -> Result<Self, ResolverBoundaryRefusal> {
        validate_board_bound(position)?;
        let legal = position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut scores = Vec::with_capacity(raw_scores.len());
        for (move_id, raw_score) in raw_scores {
            if !legal.contains(&move_id) {
                return Err(ResolverBoundaryRefusal::ForeignMove);
            }
            if !seen.insert(move_id.clone()) {
                return Err(ResolverBoundaryRefusal::DuplicateMove);
            }
            let score =
                FiniteScore::new(raw_score).map_err(|_| ResolverBoundaryRefusal::NonFiniteScore)?;
            scores.push(ResolverCandidateScore { move_id, score });
        }
        if seen != legal {
            return Err(ResolverBoundaryRefusal::IncompleteBoard);
        }
        scores.sort_by(|left, right| left.move_id.cmp(&right.move_id));
        Self::admit(position, variant, scores, None)
            .map_err(|_| ResolverBoundaryRefusal::ProducerFailure)
    }

    /// Preserve a typed refusal without manufacturing or truncating a ranking.
    pub fn refused(
        position: &DesignPosition,
        variant: ResolverVariant,
        refusal: ResolverBoundaryRefusal,
    ) -> anyhow::Result<Self> {
        if matches!(
            refusal,
            ResolverBoundaryRefusal::IncompleteBoard
                | ResolverBoundaryRefusal::DuplicateMove
                | ResolverBoundaryRefusal::ForeignMove
                | ResolverBoundaryRefusal::NonFiniteScore
        ) {
            anyhow::bail!("malformed output must be rejected, not recorded as a model refusal");
        }
        Self::admit(position, variant, Vec::new(), Some(refusal))
    }

    fn admit(
        position: &DesignPosition,
        variant: ResolverVariant,
        scores: Vec<ResolverCandidateScore>,
        refusal: Option<ResolverBoundaryRefusal>,
    ) -> anyhow::Result<Self> {
        if scores.is_empty() == refusal.is_none() {
            anyhow::bail!("a resolver packet must contain either a full ranking or one refusal");
        }
        let mut packet = Self {
            variant,
            position_id: position.state_id().clone(),
            move_set_hash: position.move_set_hash().clone(),
            board_size: position.legal_moves().len(),
            scores,
            refusal,
            packet_hash: GraphContentHash::new("0".repeat(64))?,
        };
        packet.packet_hash = packet.canonical_hash()?;
        Ok(packet)
    }

    fn canonical_hash(&self) -> anyhow::Result<GraphContentHash> {
        #[derive(Serialize)]
        struct Preimage<'a> {
            schema: &'static str,
            variant: ResolverVariant,
            position_id: &'a DesignStateId,
            move_set_hash: &'a MoveSetHash,
            board_size: usize,
            scores: &'a [ResolverCandidateScore],
            refusal: Option<ResolverBoundaryRefusal>,
        }
        Ok(GraphContentHash::new(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&Preimage {
                schema: "semantic-gameboard-resolver-packet-v1",
                variant: self.variant,
                position_id: &self.position_id,
                move_set_hash: &self.move_set_hash,
                board_size: self.board_size,
                scores: &self.scores,
                refusal: self.refusal,
            })?)
        ))?)
    }

    pub fn variant(&self) -> ResolverVariant {
        self.variant
    }

    pub fn position_id(&self) -> &DesignStateId {
        &self.position_id
    }

    pub fn move_set_hash(&self) -> &MoveSetHash {
        &self.move_set_hash
    }

    pub fn board_size(&self) -> usize {
        self.board_size
    }

    pub fn scores(&self) -> &[ResolverCandidateScore] {
        &self.scores
    }

    pub fn refusal(&self) -> Option<ResolverBoundaryRefusal> {
        self.refusal
    }

    pub fn packet_hash(&self) -> &GraphContentHash {
        &self.packet_hash
    }
}

fn validate_board_bound(position: &DesignPosition) -> Result<(), ResolverBoundaryRefusal> {
    match position.legal_moves().len() {
        0 => Err(ResolverBoundaryRefusal::EmptyBoard),
        size if size > MAX_RESOLVER_BOARD_SIZE => Err(ResolverBoundaryRefusal::BoardTooLarge),
        _ => Ok(()),
    }
}

/// One adjudicated turn with exactly one output from every Phase 6 resolver.
#[derive(Clone, Debug)]
pub struct ResolverComparisonCase {
    record_hash: String,
    risk_class: HarmClass,
    intended_move: Option<LegalMoveId>,
    packets: Vec<ResolverBoardPacket>,
}

impl ResolverComparisonCase {
    pub fn new(
        record: &GameTurnRecord,
        adjudication: &GameTurnAdjudication,
        mut packets: Vec<ResolverBoardPacket>,
    ) -> anyhow::Result<Self> {
        adjudication.validate_for_record(record)?;
        packets.sort_by_key(ResolverBoardPacket::variant);
        if packets.len() != ResolverVariant::ALL.len()
            || packets
                .iter()
                .map(ResolverBoardPacket::variant)
                .ne(ResolverVariant::ALL)
        {
            anyhow::bail!("comparison requires exactly one packet for each resolver variant");
        }
        if packets.iter().any(|packet| {
            packet.position_id() != record.position().state_id()
                || packet.move_set_hash() != record.position().move_set_hash()
                || packet.board_size() != record.position().legal_moves().len()
        }) {
            anyhow::bail!("resolver comparison packets do not share the adjudicated move board");
        }
        Ok(Self {
            record_hash: record.record_hash().as_str().to_string(),
            risk_class: record.risk_class(),
            intended_move: adjudication.positive_label().cloned(),
            packets,
        })
    }
}

/// Wilson 95% interval for an observed proportion.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ProportionInterval {
    estimate: FiniteScore,
    lower: FiniteScore,
    upper: FiniteScore,
}

impl ProportionInterval {
    pub fn estimate(&self) -> FiniteScore {
        self.estimate
    }

    pub fn lower(&self) -> FiniteScore {
        self.lower
    }

    pub fn upper(&self) -> FiniteScore {
        self.upper
    }
}

/// Ranking results for one resolver, optionally restricted to one risk class.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolverMetrics {
    variant: ResolverVariant,
    risk_class: Option<HarmClass>,
    labelled_turns: usize,
    unlabelled_turns: usize,
    ranked_turns: usize,
    refused_turns: usize,
    top_one: usize,
    top_three: usize,
    top_one_interval: Option<ProportionInterval>,
    top_three_interval: Option<ProportionInterval>,
    mean_reciprocal_rank: Option<FiniteScore>,
    mean_negative_log_likelihood: Option<FiniteScore>,
}

impl ResolverMetrics {
    pub fn variant(&self) -> ResolverVariant {
        self.variant
    }

    pub fn risk_class(&self) -> Option<HarmClass> {
        self.risk_class
    }

    pub fn labelled_turns(&self) -> usize {
        self.labelled_turns
    }

    pub fn unlabelled_turns(&self) -> usize {
        self.unlabelled_turns
    }

    pub fn ranked_turns(&self) -> usize {
        self.ranked_turns
    }

    pub fn refused_turns(&self) -> usize {
        self.refused_turns
    }

    pub fn top_one(&self) -> usize {
        self.top_one
    }

    pub fn top_three(&self) -> usize {
        self.top_three
    }

    pub fn top_one_interval(&self) -> Option<ProportionInterval> {
        self.top_one_interval
    }

    pub fn top_three_interval(&self) -> Option<ProportionInterval> {
        self.top_three_interval
    }

    pub fn mean_reciprocal_rank(&self) -> Option<FiniteScore> {
        self.mean_reciprocal_rank
    }

    pub fn mean_negative_log_likelihood(&self) -> Option<FiniteScore> {
        self.mean_negative_log_likelihood
    }
}

/// Content-addressed identical-board comparison report.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ResolverComparisonReport {
    case_count: usize,
    metrics: Vec<ResolverMetrics>,
    report_hash: GraphContentHash,
}

impl ResolverComparisonReport {
    pub fn case_count(&self) -> usize {
        self.case_count
    }

    pub fn metrics(&self) -> &[ResolverMetrics] {
        &self.metrics
    }

    pub fn report_hash(&self) -> &GraphContentHash {
        &self.report_hash
    }
}

#[derive(Default)]
struct MetricAccumulator {
    labelled: usize,
    unlabelled: usize,
    ranked: usize,
    refused: usize,
    top_one: usize,
    top_three: usize,
    reciprocal_rank_sum: f64,
    negative_log_likelihood_sum: f64,
}

/// Compare all four evidence producers. Candidate and case order cannot affect the report.
pub fn compare_resolvers(
    cases: &[ResolverComparisonCase],
) -> anyhow::Result<ResolverComparisonReport> {
    if cases.is_empty() {
        anyhow::bail!("resolver comparison requires adjudicated cases");
    }
    let mut canonical = cases.to_vec();
    canonical.sort_by(|left, right| left.record_hash.cmp(&right.record_hash));
    if canonical
        .windows(2)
        .any(|pair| pair[0].record_hash == pair[1].record_hash)
    {
        anyhow::bail!("resolver comparison received a duplicate game-turn record");
    }
    let risks = [
        HarmClass::ReadOnly,
        HarmClass::Reversible,
        HarmClass::Irreversible,
        HarmClass::Destructive,
    ];
    let mut accumulators = BTreeMap::<(ResolverVariant, u8), MetricAccumulator>::new();
    for case in &canonical {
        for packet in &case.packets {
            accumulate(
                accumulators.entry((packet.variant(), u8::MAX)).or_default(),
                case.intended_move.as_ref(),
                packet,
            )?;
            accumulate(
                accumulators
                    .entry((packet.variant(), risk_index(case.risk_class)))
                    .or_default(),
                case.intended_move.as_ref(),
                packet,
            )?;
        }
    }
    let mut metrics = Vec::new();
    for variant in ResolverVariant::ALL {
        metrics.push(finish_metrics(
            variant,
            None,
            accumulators.remove(&(variant, u8::MAX)).unwrap_or_default(),
        )?);
        for risk in risks {
            metrics.push(finish_metrics(
                variant,
                Some(risk),
                accumulators
                    .remove(&(variant, risk_index(risk)))
                    .unwrap_or_default(),
            )?);
        }
    }
    #[derive(Serialize)]
    struct Preimage<'a> {
        schema: &'static str,
        records: Vec<&'a str>,
        metrics: &'a [ResolverMetrics],
    }
    let report_hash = GraphContentHash::new(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&Preimage {
            schema: "semantic-gameboard-resolver-comparison-v1",
            records: canonical
                .iter()
                .map(|case| case.record_hash.as_str())
                .collect(),
            metrics: &metrics,
        })?)
    ))?;
    Ok(ResolverComparisonReport {
        case_count: canonical.len(),
        metrics,
        report_hash,
    })
}

fn accumulate(
    accumulator: &mut MetricAccumulator,
    intended_move: Option<&LegalMoveId>,
    packet: &ResolverBoardPacket,
) -> anyhow::Result<()> {
    let Some(intended_move) = intended_move else {
        accumulator.unlabelled += 1;
        return Ok(());
    };
    accumulator.labelled += 1;
    if packet.refusal().is_some() {
        accumulator.refused += 1;
        return Ok(());
    }
    accumulator.ranked += 1;
    let mut ranked = packet.scores().to_vec();
    ranked.sort_by(|left, right| {
        right
            .score
            .get()
            .total_cmp(&left.score.get())
            .then_with(|| left.move_id.cmp(&right.move_id))
    });
    let rank = ranked
        .iter()
        .position(|score| score.move_id() == intended_move)
        .ok_or_else(|| anyhow::anyhow!("admitted packet omitted the intended legal move"))?
        + 1;
    accumulator.top_one += usize::from(rank == 1);
    accumulator.top_three += usize::from(rank <= 3);
    accumulator.reciprocal_rank_sum += 1.0 / rank as f64;
    let probability = softmax_probability(&ranked, intended_move)?;
    accumulator.negative_log_likelihood_sum += -probability.max(f64::MIN_POSITIVE).ln();
    Ok(())
}

fn softmax_probability(
    scores: &[ResolverCandidateScore],
    intended_move: &LegalMoveId,
) -> anyhow::Result<f64> {
    let maximum = scores
        .iter()
        .map(|score| score.score.get())
        .fold(f64::NEG_INFINITY, f64::max);
    let denominator = scores
        .iter()
        .map(|score| (score.score.get() - maximum).exp())
        .sum::<f64>();
    let numerator = scores
        .iter()
        .find(|score| score.move_id() == intended_move)
        .map(|score| (score.score.get() - maximum).exp())
        .ok_or_else(|| anyhow::anyhow!("intended move missing from full-board scores"))?;
    if !denominator.is_finite() || denominator <= 0.0 {
        anyhow::bail!("resolver probability normalization failed");
    }
    Ok(numerator / denominator)
}

fn finish_metrics(
    variant: ResolverVariant,
    risk_class: Option<HarmClass>,
    value: MetricAccumulator,
) -> anyhow::Result<ResolverMetrics> {
    Ok(ResolverMetrics {
        variant,
        risk_class,
        labelled_turns: value.labelled,
        unlabelled_turns: value.unlabelled,
        ranked_turns: value.ranked,
        refused_turns: value.refused,
        top_one: value.top_one,
        top_three: value.top_three,
        top_one_interval: proportion_interval(value.top_one, value.ranked)?,
        top_three_interval: proportion_interval(value.top_three, value.ranked)?,
        mean_reciprocal_rank: mean(value.reciprocal_rank_sum, value.ranked)?,
        mean_negative_log_likelihood: mean(value.negative_log_likelihood_sum, value.ranked)?,
    })
}

fn mean(sum: f64, count: usize) -> anyhow::Result<Option<FiniteScore>> {
    (count > 0)
        .then(|| FiniteScore::new(sum / count as f64))
        .transpose()
        .map_err(Into::into)
}

fn proportion_interval(
    successes: usize,
    count: usize,
) -> anyhow::Result<Option<ProportionInterval>> {
    if count == 0 {
        return Ok(None);
    }
    let n = count as f64;
    let estimate = successes as f64 / n;
    let z = 1.959_963_984_540_054_f64;
    let denominator = 1.0 + z * z / n;
    let centre = (estimate + z * z / (2.0 * n)) / denominator;
    let spread =
        z * ((estimate * (1.0 - estimate) / n + z * z / (4.0 * n * n)).sqrt()) / denominator;
    Ok(Some(ProportionInterval {
        estimate: FiniteScore::new(estimate)?,
        lower: FiniteScore::new((centre - spread).clamp(0.0, 1.0))?,
        upper: FiniteScore::new((centre + spread).clamp(0.0, 1.0))?,
    }))
}

fn risk_index(risk: HarmClass) -> u8 {
    match risk {
        HarmClass::ReadOnly => 0,
        HarmClass::Reversible => 1,
        HarmClass::Irreversible => 2,
        HarmClass::Destructive => 3,
    }
}

#[cfg(test)]
mod tests {
    use semantic_decision_contracts::{
        BoardPath, CanonicalCandidateId, DesignBelief, DesignFocus, DesignTurnId, EvidenceLane,
        FocusAbsenceReason, GameDisposition, GameDomainId, GameSessionId, GameTurnAdjudication,
        GameTurnAnswer, GameTurnAnswerAbsenceReason, GameTurnAttempt, GameTurnCompilerResult,
        GameTurnJudgement, GameTurnRecord, GraphContentHash, GraphRevision, HistoryHash,
        IntendedMove, LaneScore, LegalMove, MoveEvidence, ProducerIdentity, SemanticFamilyId,
        SnapshotIdentity, GAMEBOARD_SCHEMA_VERSION,
    };

    use super::*;

    fn position() -> DesignPosition {
        let moves = ['a', 'b', 'c']
            .into_iter()
            .map(|value| {
                LegalMove::new(
                    GAMEBOARD_SCHEMA_VERSION,
                    CanonicalCandidateId::new(format!("candidate.{value}")).unwrap(),
                    GraphRevision::new("revision-1").unwrap(),
                    false,
                    None,
                    Vec::new(),
                    Vec::new(),
                    None,
                )
                .unwrap()
            })
            .collect();
        DesignPosition::new(
            GAMEBOARD_SCHEMA_VERSION,
            GameDomainId::new("domain.test").unwrap(),
            BoardPath::new(vec!["root".into()]).unwrap(),
            SnapshotIdentity::new("pack@sha256:test").unwrap(),
            GraphRevision::new("revision-1").unwrap(),
            GraphContentHash::new("d".repeat(64)).unwrap(),
            "compiler-test-v1",
            "policy-test-v1",
            None,
            DesignFocus::absent(FocusAbsenceReason::NotProvided),
            HistoryHash::new("f".repeat(64)).unwrap(),
            moves,
        )
        .unwrap()
    }

    fn comparison_case(turn: &str, ranked_first: usize) -> ResolverComparisonCase {
        let position = position();
        let intended = position.legal_moves()[0].move_id().clone();
        let evidence = position
            .legal_moves()
            .iter()
            .map(|legal_move| {
                MoveEvidence::new(
                    GAMEBOARD_SCHEMA_VERSION,
                    legal_move.move_id().clone(),
                    EvidenceLane::ALL
                        .into_iter()
                        .map(|lane| LaneScore {
                            lane,
                            score: FiniteScore::new(0.0).unwrap(),
                        })
                        .collect(),
                    FiniteScore::new(0.0).unwrap(),
                    FiniteScore::new(1.0 / position.legal_moves().len() as f64).unwrap(),
                    Vec::new(),
                    ProducerIdentity::new("test.comparison-evidence.v1").unwrap(),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let belief = DesignBelief::new(
            GAMEBOARD_SCHEMA_VERSION,
            position.state_id().clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            ProducerIdentity::new("test.comparison-belief.v1").unwrap(),
        )
        .unwrap();
        let disposition = GameDisposition::propose_move(&position, intended.clone()).unwrap();
        let record = GameTurnRecord::new(
            GAMEBOARD_SCHEMA_VERSION,
            GameSessionId::new("comparison-session").unwrap(),
            DesignTurnId::new(turn).unwrap(),
            1,
            100,
            SemanticFamilyId::new("family.comparison").unwrap(),
            HarmClass::Reversible,
            GraphContentHash::new("7".repeat(64)).unwrap(),
            position.clone(),
            evidence,
            belief,
            disposition,
            GameTurnAnswer::not_observed(GameTurnAnswerAbsenceReason::NotRequested),
            Some(intended.clone()),
            None,
            GameTurnAttempt::not_attempted(),
            GameTurnCompilerResult::not_requested(),
            Vec::new(),
        )
        .unwrap();
        let adjudication = GameTurnAdjudication::new(
            &record,
            "test-operator",
            101,
            GameTurnJudgement::AcceptedMove,
            IntendedMove::OnBoard { move_id: intended },
            None,
            Vec::new(),
            None,
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();
        let packets = ResolverVariant::ALL
            .into_iter()
            .map(|variant| {
                ResolverBoardPacket::ranked(
                    &position,
                    variant,
                    position
                        .legal_moves()
                        .iter()
                        .enumerate()
                        .map(|(index, legal_move)| {
                            (
                                legal_move.move_id().clone(),
                                f64::from(index == ranked_first),
                            )
                        })
                        .collect(),
                )
                .unwrap()
            })
            .collect();
        ResolverComparisonCase::new(&record, &adjudication, packets).unwrap()
    }

    #[test]
    fn packet_requires_every_legal_move_and_is_order_invariant() {
        let position = position();
        let ids = position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<Vec<_>>();
        assert_eq!(
            ResolverBoardPacket::ranked(
                &position,
                ResolverVariant::StructuredChoice,
                vec![(ids[0].clone(), 1.0)]
            ),
            Err(ResolverBoundaryRefusal::IncompleteBoard)
        );
        assert_eq!(
            ResolverBoardPacket::ranked(
                &position,
                ResolverVariant::StructuredChoice,
                vec![
                    (ids[0].clone(), 0.0),
                    (ids[1].clone(), f64::NAN),
                    (ids[2].clone(), 1.0),
                ],
            ),
            Err(ResolverBoundaryRefusal::NonFiniteScore)
        );
        let first = ResolverBoardPacket::ranked(
            &position,
            ResolverVariant::StructuredChoice,
            vec![
                (ids[0].clone(), 0.0),
                (ids[1].clone(), 0.5),
                (ids[2].clone(), 1.0),
            ],
        )
        .unwrap();
        let replay = ResolverBoardPacket::ranked(
            &position,
            ResolverVariant::StructuredChoice,
            vec![
                (ids[2].clone(), 1.0),
                (ids[0].clone(), 0.0),
                (ids[1].clone(), 0.5),
            ],
        )
        .unwrap();
        assert_eq!(first, replay);
    }

    #[test]
    fn malformed_output_cannot_be_disguised_as_a_refusal() {
        let position = position();
        assert!(ResolverBoardPacket::refused(
            &position,
            ResolverVariant::BoundedPromptOffline,
            ResolverBoundaryRefusal::IncompleteBoard,
        )
        .is_err());
        let refused = ResolverBoardPacket::refused(
            &position,
            ResolverVariant::BoundedPromptOffline,
            ResolverBoundaryRefusal::ModelRefusal,
        )
        .unwrap();
        assert_eq!(refused.scores(), &[]);
        assert_eq!(
            refused.refusal(),
            Some(ResolverBoundaryRefusal::ModelRefusal)
        );
    }

    #[test]
    fn offline_request_is_full_board_bounded_unicode_safe_and_bundle_bound() {
        let position = position();
        let ids = position
            .legal_moves()
            .iter()
            .map(|legal_move| legal_move.move_id().clone())
            .collect::<Vec<_>>();
        let bundle = GraphContentHash::new("9".repeat(64)).unwrap();
        let candidates = ids
            .iter()
            .enumerate()
            .map(|(index, move_id)| (move_id.clone(), format!("候補 {index} 🧭"), 3))
            .collect::<Vec<_>>();
        let request = BoundedOfflineRankerRequest::new(
            &position,
            "ユーザーの意図 🧭".into(),
            4,
            candidates.clone(),
            32,
            bundle.clone(),
            bundle.clone(),
        )
        .unwrap();
        let mut reversed = candidates;
        reversed.reverse();
        let replay = BoundedOfflineRankerRequest::new(
            &position,
            "ユーザーの意図 🧭".into(),
            4,
            reversed,
            32,
            bundle.clone(),
            bundle,
        )
        .unwrap();
        assert_eq!(request, replay);
        assert_eq!(request.candidates().len(), ids.len());
        assert_eq!(
            BoundedOfflineRankerRequest::new(
                &position,
                "prompt".into(),
                1,
                vec![(ids[0].clone(), String::new(), 0)],
                32,
                GraphContentHash::new("9".repeat(64)).unwrap(),
                GraphContentHash::new("9".repeat(64)).unwrap(),
            ),
            Err(ResolverBoundaryRefusal::EmptyCandidateText)
        );
        assert_eq!(
            BoundedOfflineRankerRequest::new(
                &position,
                "prompt".into(),
                1,
                ids.into_iter()
                    .map(|move_id| (move_id, "candidate".into(), 1))
                    .collect(),
                32,
                GraphContentHash::new("8".repeat(64)).unwrap(),
                GraphContentHash::new("9".repeat(64)).unwrap(),
            ),
            Err(ResolverBoundaryRefusal::BundleCardMismatch)
        );
    }

    #[test]
    fn confidence_intervals_are_finite_and_bounded() {
        let interval = proportion_interval(7, 10).unwrap().unwrap();
        assert_eq!(interval.estimate().get(), 0.7);
        assert!(interval.lower().get() < 0.7);
        assert!(interval.upper().get() > 0.7);
    }

    #[test]
    fn four_resolver_report_is_case_order_invariant_and_risk_stratified() {
        let first = comparison_case("comparison-turn-a", 0);
        let second = comparison_case("comparison-turn-b", 1);
        let report = compare_resolvers(&[first.clone(), second.clone()]).unwrap();
        let replay = compare_resolvers(&[second, first]).unwrap();
        assert_eq!(report, replay);
        assert_eq!(report.case_count(), 2);
        let overall = report
            .metrics()
            .iter()
            .find(|metrics| {
                metrics.variant() == ResolverVariant::StructuredChoice
                    && metrics.risk_class().is_none()
            })
            .unwrap();
        assert_eq!(overall.labelled_turns(), 2);
        assert_eq!(overall.ranked_turns(), 2);
        assert_eq!(overall.top_one(), 1);
        assert_eq!(overall.top_three(), 2);
    }
}
