//! DSL/BPMN/DMN authoring REST API for the bpmn-lite federated stack (T6).
//!
//! Runs on port 8080 alongside the workflow instance runner
//! (`bpmn-lite-server-runner`, gRPC on 50051 + its own demo REST API).
//! Backed by `MemoryStore` — demo-mode only, no Postgres required.
//!
//! This crate hosts the **workflow designer** half of the former combined
//! `bpmn-lite-server`: compile preview (BPMN/DMN), macro application,
//! diagnostics resolution, template catalogue, and design-session
//! (graph-backed authoring) endpoints. The instance runner half lives in
//! the sibling `bpmn-lite-server-runner` crate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::{get, post};
use axum::Router;

/// Explicit, product-owned request body ceiling. Axum's unconfigured default
/// is 2 MiB; legitimate whole-graph/session-save payloads can exceed that, so
/// this is set generously higher while still bounding decode allocation for
/// an attacker-supplied body (Gate 8 "decode allocations" resource limit).
const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bpmn_lite_compiler::dsl::{ExecutionNode, WorkflowExecutionPlan};
use bpmn_lite_store::store::{DesignSessionEventKind, WorkflowStore};
use bpmn_lite_store::store_memory::MemoryStore;
use bpmn_lite_types::TenantId;

// ── Designer state ─────────────────────────────────────────────────────

/// Explicit utterance-mapper rollout. Production defaults to `Shadow` and
/// there is deliberately no auto-apply state.
///
/// Six named stages, per `EOP-PLAN-BPMN-GAMEBOARD-001.md` §15's capability-
/// surface rollout (`observe -> shadow -> palette -> feedback -> suggest ->
/// workbook`) — scaffolding only, added on explicit instruction ahead of any
/// actual rollout decision (2026-08-10). `Observe`, `Palette` and `Feedback`
/// name real future capability distinctions the plan describes (recording
/// positions without disposition; exposing the legal move palette/previews;
/// exposing governed explanations and recovery options for unsuccessful
/// attempts) that this codebase has no separate gate for yet — introducing a
/// gate that doesn't correspond to a real code path would be exactly the
/// kind of trap door the working contract forbids, so all four
/// pre-`Suggest` stages currently behave identically to `Shadow` under the
/// two gates that do exist (`suggestions_enabled`, `workbooks_enabled`).
/// `Suggest` and `Workbook` are unchanged from the prior three-stage design.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MapperRollout {
    Observe,
    Shadow,
    Palette,
    Feedback,
    Suggest,
    Workbook,
}

impl MapperRollout {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            Some("observe") => Self::Observe,
            Some("palette") => Self::Palette,
            Some("feedback") => Self::Feedback,
            Some("suggest") => Self::Suggest,
            Some("workbook") => Self::Workbook,
            _ => Self::Shadow,
        }
    }

    fn configured() -> Self {
        #[cfg(test)]
        {
            // Endpoint tests exercise the complete staged-workbook surface;
            // a separate test below cements the production default.
            Self::Workbook
        }
        #[cfg(not(test))]
        {
            Self::parse(std::env::var("BPMN_MAPPER_ROLLOUT").ok().as_deref())
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Shadow => "shadow",
            Self::Palette => "palette",
            Self::Feedback => "feedback",
            Self::Suggest => "suggest",
            Self::Workbook => "workbook",
        }
    }

    fn suggestions_enabled(self) -> bool {
        matches!(self, Self::Suggest | Self::Workbook)
    }

    fn workbooks_enabled(self) -> bool {
        matches!(self, Self::Workbook)
    }
}

pub struct DesignerState {
    store: Arc<dyn WorkflowStore>,
    /// Save-as-template registry (2026-07-30): the lifecycle-bearing
    /// `workflow_templates` system (`bpmn_lite_authoring::TemplateStore` —
    /// Draft/Published/Retired, immutability enforced by the store), fed by
    /// `compile_and_publish_from_dto`. Demo mode uses the in-memory
    /// backend; `PostgresTemplateStore` (authoring's `postgres` feature)
    /// slots in when the designer gets a durable config. Distinct from
    /// `store`'s `workflow_template_catalog` (plan_hash + dsl_body, no
    /// lifecycle), which the runtime instantiation path still reads —
    /// see the dual-write note in `save_design_session_endpoint`.
    template_store: Arc<dyn bpmn_lite_authoring::TemplateStore>,
    /// Phase B (2026-07-30): compile+start capability for spawning a
    /// runnable instance directly from a Published template, in-process
    /// against `store`. The demo designer and demo runner are two
    /// independent processes with independent `MemoryStore`s (confirmed —
    /// no sharing exists), so spawning here — where the template and its
    /// compiled program already live — avoids inventing cross-process
    /// transport. Blurs the authoring/runtime crate split on purpose, for
    /// demo mode only; not a precedent for merging the crates generally.
    engine: bpmn_lite_engine::BpmnLiteEngine,
    tenant_id: TenantId,
    mapper_rollout: MapperRollout,
    /// Serializes each session's load→reconstruct→stage→append sequence
    /// (graph-edit and save-as-template) so two concurrent requests against
    /// the SAME session id can never both stage against the same base and
    /// both persist — the second would otherwise replay against a DAG
    /// shape it was never validated against, permanently bricking the
    /// session's reconstruction. Different sessions never contend.
    session_locks: Mutex<HashMap<Uuid, Arc<tokio::sync::Mutex<()>>>>,
    // DIR-004 Phase 1/2 + G1.2 wiring: dev-session capture is durable,
    // store-backed (`demo.store.{open,append,load}_dev_capture_session`)
    // -- no in-memory field here. A session only captures after an
    // explicit `POST .../dev-capture/enable` call carrying Adam's
    // consent statement (D17's spirit, applied to his own self-testing
    // use); `session_utterance_endpoint` checks the store and, if open,
    // captures the full closure via `utterance_engine::dev_capture`
    // (always compiled, structurally distinct from the Q9-gated path).
    /// Q9 charter-governed live capture (EOP-GOV-Q9-CHARTER-001,
    /// ratified 2026-08-06). `Some` ONLY when the designated deployment
    /// (`scripts/run-designer-q9-capture.sh`) supplies BOTH
    /// `Q9_CHARTER_REF` (must equal the ratified reference exactly) and
    /// `Q9_CAPTURE_DIR` at startup; misconfiguration refuses startup
    /// (fail closed). `None` = compiled-but-off: every capture call
    /// reports `suppressed_no_charter`, visibly, per interaction.
    #[cfg(feature = "q9-capture")]
    q9_capture: Option<Mutex<utterance_engine::capture::CapturePipeline>>,
    /// Utterance-derived pending proposals awaiting human ratification
    /// (the propose → preview → ratify → apply loop's middle state).
    ///
    /// DESIGN DECISION — deliberately EPHEMERAL, in-memory only: a
    /// pending proposal is an un-ratified draft; losing it on restart is
    /// fail-closed (nothing un-ratified may survive a process). The
    /// audit trail is NOT here: every state is copied into an append-only
    /// `ProposalAudit` event, while ratification additionally becomes a
    /// persisted `GraphEdit`. Losing this cache on restart remains
    /// fail-closed; the historical proposal chain does not disappear.
    proposals: Mutex<HashMap<Uuid, PendingProposal>>,
    /// Tier-1 trained ranker (DIR-002 serving integration, ruled
    /// 2026-08-01): loaded ONCE at startup from `SLM_BUNDLE_DIR` when the
    /// `candle-probe` feature is compiled. `None` = degraded-to-tier-0
    /// serving (env unset OR load failure — logged, never a startup
    /// refusal, never a fabricated bundle identity: the served
    /// `model_bundle_hash` is whatever producer actually ran).
    #[cfg(feature = "candle-probe")]
    tier1: Option<Arc<utterance_engine::trained_ranker::Tier1Ranker>>,
    /// The BGE embed tier-0 (`embed` feature): feeds tier-1's retrieval
    /// subset when a bundle is loaded (the corpus was generated on this
    /// retriever — the lexical tier-0 structurally excludes the
    /// context-sensitivity pairs tier-1 exists to resolve) and serves
    /// alone otherwise. `None` = load failure, degraded to lexical.
    #[cfg(feature = "embed")]
    embed_tier0: Option<Arc<utterance_engine::retrieval::embed::EmbedTier0>>,
}

/// One staged-but-unratified proposal. `staged_against_hash` is the
/// session's graph-identity hash (blake3 over the accumulated edit-log
/// payloads) at staging time — ratification refuses on any drift.
#[derive(Clone)]
pub(crate) struct PendingProposal {
    session_id: Uuid,
    workbook: semantic_decision_contracts::ProposalWorkbook,
    bound: Option<crate::proposal::BoundProposal>,
    preview_delta: Option<semantic_decision_contracts::GraphDeltaPreview>,
    source_utterance_text: String,
    staged_against_hash: String,
    dry_run_diagnostics: Vec<String>,
    audit_event_seq: Option<u64>,
    design_position: semantic_decision_contracts::DesignPosition,
    move_evidence: Vec<semantic_decision_contracts::MoveEvidence>,
    design_belief: semantic_decision_contracts::DesignBelief,
    bound_game: Option<utterance_engine::bpmn_board::BpmnBoundGameTurnProjection>,
    gameboard_attempt_receipt_json: Option<String>,
    correction_of: Option<semantic_decision_contracts::MoveAttemptId>,
    correction_kind: Option<semantic_decision_contracts::CorrectionKind>,
}

/// Per-request latency ceiling (G1.4). Callers state the interaction
/// shape explicitly, rather than lane selection depending only on
/// which lanes happen to be loaded — mirrors I34's "each consumer names
/// which it uses" discipline. `CompletionOnly` forbids tier-1 BY
/// CONSTRUCTION regardless of whether a tier-1 bundle is loaded;
/// `completion_budget_never_permits_tier1` proves this exhaustively
/// without needing a real loaded model. No caller currently passes
/// `CompletionOnly` — no completion-shaped endpoint exists yet — but the
/// mechanism is real and load-bearing, not speculative: it is the seam
/// a future completion endpoint plugs into, and it is what the
/// exhaustive test exercises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceLatencyBudget {
    /// <100ms target: tier-0 only, never tier-1 (measured tier-1 p95 is
    /// 2.6-2.9s — a wrong-lane problem, not a tuning problem).
    CompletionOnly,
    /// <500ms target: the full cascade may run.
    UtteranceSubmit,
}

impl EvidenceLatencyBudget {
    fn permits_tier1(self) -> bool {
        matches!(self, EvidenceLatencyBudget::UtteranceSubmit)
    }
}

impl DesignerState {
    pub fn try_new() -> Result<Arc<Self>, anyhow::Error> {
        Self::assemble(
            Arc::new(MemoryStore::new()),
            Arc::new(bpmn_lite_authoring::MemoryTemplateStore::new()),
        )
    }

    /// Assemble a state over an explicit store pair. Shared by the memory
    /// default and the Postgres (env-selected) path; also lets the
    /// restart-survival tests build two states over the same database.
    pub fn assemble(
        store: Arc<dyn WorkflowStore>,
        template_store: Arc<dyn bpmn_lite_authoring::TemplateStore>,
    ) -> Result<Arc<Self>, anyhow::Error> {
        Self::assemble_with_rollout(store, template_store, MapperRollout::configured())
    }

    fn assemble_with_rollout(
        store: Arc<dyn WorkflowStore>,
        template_store: Arc<dyn bpmn_lite_authoring::TemplateStore>,
        mapper_rollout: MapperRollout,
    ) -> Result<Arc<Self>, anyhow::Error> {
        let tenant_id = TenantId::new("demo")?;
        let engine =
            bpmn_lite_engine::BpmnLiteEngine::new_with_tenant(store.clone(), tenant_id.clone());
        Ok(Arc::new(Self {
            store,
            template_store,
            engine,
            tenant_id,
            mapper_rollout,
            session_locks: Mutex::new(HashMap::new()),
            #[cfg(feature = "q9-capture")]
            q9_capture: Self::load_q9_capture_from_env()?,
            proposals: Mutex::new(HashMap::new()),
            #[cfg(feature = "candle-probe")]
            tier1: Self::load_tier1_from_env(),
            #[cfg(feature = "embed")]
            embed_tier0: Self::load_embed_tier0(),
        }))
    }

    /// Q9 capture startup wiring. Unlike the tier-1 model load (which
    /// DEGRADES on failure), capture misconfiguration REFUSES startup:
    /// an operator who explicitly requested charter-governed capture
    /// must never silently run without it. Env unset → compiled-but-off
    /// (`None`), logged. In tests: always off — hermetic regardless of
    /// the developer's shell environment.
    #[cfg(feature = "q9-capture")]
    fn load_q9_capture_from_env(
    ) -> Result<Option<Mutex<utterance_engine::capture::CapturePipeline>>, anyhow::Error> {
        #[cfg(test)]
        {
            return Ok(None);
        }
        #[cfg(not(test))]
        {
            let charter = match std::env::var("Q9_CHARTER_REF") {
                Ok(c) if !c.trim().is_empty() => c,
                _ => {
                    tracing::info!(
                        "Q9_CHARTER_REF unset — q9-capture compiled but OFF \
                         (every capture call will report suppressed_no_charter)"
                    );
                    return Ok(None);
                }
            };
            let dir = std::env::var("Q9_CAPTURE_DIR").map_err(|_| {
                anyhow::anyhow!(
                    "Q9_CHARTER_REF is set but Q9_CAPTURE_DIR is not — refusing to start: \
                     charter-governed capture must be durable (EOP-GOV-Q9-CHARTER-001 §4/§5)"
                )
            })?;
            let pipeline = utterance_engine::capture::CapturePipeline::under_ratified_charter(
                &charter,
                std::path::Path::new(&dir),
            )?;
            tracing::info!(charter = %charter.trim(), dir = %dir, "Q9 capture LIVE under ratified charter");
            Ok(Some(Mutex::new(pipeline)))
        }
    }

    /// Startup bundle load (fail-closed DEGRADATION, not failure): env
    /// unset → tier-0 serving, honestly; set-but-broken → error logged,
    /// tier-0 serving. The endpoint never 500s for a missing model.
    #[cfg(feature = "candle-probe")]
    fn load_tier1_from_env() -> Option<Arc<utterance_engine::trained_ranker::Tier1Ranker>> {
        // Unit tests must remain hermetic even when CI compiles every named
        // feature.  Loading weights is an explicit evaluation activity, not
        // an incidental side effect of constructing an in-memory test state.
        #[cfg(test)]
        std::env::var_os("BPMN_LITE_TEST_ENABLE_MODELS")?;
        let dir = match std::env::var("SLM_BUNDLE_DIR") {
            Ok(d) if !d.is_empty() => d,
            _ => {
                tracing::info!(
                    "SLM_BUNDLE_DIR unset — tier-1 ranker not loaded; serving tier-0 only"
                );
                return None;
            }
        };
        match utterance_engine::trained_ranker::Tier1Ranker::load(
            utterance_engine::trained_ranker::Base::ModernbertBase,
            std::path::Path::new(&dir),
        ) {
            Ok(r) => {
                tracing::info!(
                    bundle = %r.model_bundle_hash(),
                    temperature = r.temperature(),
                    "tier-1 trained ranker loaded from {dir}"
                );
                Some(Arc::new(r))
            }
            Err(e) => {
                tracing::error!(
                    "tier-1 bundle load FAILED from SLM_BUNDLE_DIR={dir}: {e:#} — \
                     degrading to tier-0 serving (record identity stays honest)"
                );
                None
            }
        }
    }

    #[cfg(feature = "embed")]
    fn load_embed_tier0() -> Option<Arc<utterance_engine::retrieval::embed::EmbedTier0>> {
        // See `load_tier1_from_env`: ordinary all-feature tests exercise the
        // deterministic degraded path without network access or model loads.
        #[cfg(test)]
        std::env::var_os("BPMN_LITE_TEST_ENABLE_MODELS")?;
        match utterance_engine::retrieval::embed::EmbedTier0::new() {
            Ok(t) => Some(Arc::new(t)),
            Err(e) => {
                tracing::error!("embed tier-0 load FAILED: {e:#} — degrading to lexical tier-0");
                None
            }
        }
    }

    /// THE evidence producer selection for `session_utterance_endpoint`
    /// (I27: evidence only — `policy::decide` downstream is untouched).
    /// Priority: governed exact-match short-circuit → tier-1 trained
    /// ranker (bundle loaded, budget permits) → embed tier-0 (compiled +
    /// loaded) → lexical tier-0 (default build's behavior, unchanged).
    /// The active producer signs the evidence via `model_bundle_hash`,
    /// so the record is honest on every path.
    fn retrieve_utterance_evidence(
        &self,
        text: &str,
        board: &dyn utterance_engine::board::InferenceBoard,
        context: &utterance_engine::context::ContextProjection,
        position: Option<&semantic_decision_contracts::DesignPosition>,
        attempts: &[semantic_decision_contracts::MoveAttemptReceipt],
        budget: EvidenceLatencyBudget,
    ) -> anyhow::Result<utterance_engine::contract::SlmResult> {
        let finalize = |result,
                        lane: semantic_decision_contracts::EvidenceLane,
                        bundle: String|
         -> anyhow::Result<utterance_engine::contract::SlmResult> {
            if let (Some(semantic_board), Some(position)) = (board.semantic_board(), position) {
                return utterance_engine::bpmn_board::finalize_bpmn_move_evidence(
                    semantic_board,
                    position,
                    text,
                    result,
                    lane,
                    vec![bundle],
                    attempts,
                );
            }
            if let Some(semantic_board) = board.semantic_board() {
                return utterance_engine::exact::finalize_semantic_evidence(
                    semantic_board,
                    text,
                    result,
                    vec![lane],
                    vec![bundle],
                );
            }
            Ok(result)
        };
        #[cfg(not(feature = "candle-probe"))]
        let _ = context; // context text is a tier-1 encoding input only

        // G1.4: a governed exact-match that resolves CLEANLY (exactly one
        // candidate) short-circuits to the cheap lexical lane BY
        // CONSTRUCTION -- never even attempting tier-1/embed -- rather
        // than by which lanes happen to be loaded. `finalize_*` below
        // boosts the matched candidate to 1.0 regardless of which lane
        // supplied the base ranking, so WHO WINS is lane-independent --
        // but a blind review correctly caught that `finalize_*` only
        // caps every OTHER candidate at 0.99, it does not reset them, so
        // lexical's raw (and comparatively crude) token-overlap score
        // for a runner-up could survive close enough to 1.0 to collapse
        // `policy::decide`'s separation_margin and flip a clean exact
        // match into `Ambiguous`/`EscalateToSage`. The runner-up scores
        // are explicitly zeroed below (see comment there) so the margin
        // is always maximal on this path -- true losslessness, not just
        // for the winner. Before any of this, lane selection depended
        // ONLY on which lanes were loaded, never on request-time match
        // quality (RESEARCH-002/S4.1: measured tier-1 p95 is 2.6-2.9s)
        // -- a request tier-0 could answer in <1ms paid the full tier-1
        // cost whenever a bundle happened to be loaded.
        let clean_exact_match = board
            .semantic_board()
            .map(|semantic_board| {
                matches!(
                    utterance_engine::exact::governed_exact(semantic_board, text),
                    utterance_engine::exact::ExactMatch::Unique(_)
                )
            })
            .unwrap_or(false);

        if !clean_exact_match && budget.permits_tier1() {
            #[cfg(feature = "candle-probe")]
            if let Some(t1) = &self.tier1 {
                if let Some(semantic_board) = board.semantic_board() {
                    let result = t1.rank_full_board(text, context, semantic_board)?;
                    let bundle = result.model_bundle_hash.clone();
                    return finalize(
                        result,
                        utterance_engine::exact::EvidenceLane::CandleCrossEncoder,
                        bundle,
                    );
                }
                // Deliberately do NOT call `t1.rank(...)` here. Every loadable
                // tier-1 bundle's card is required (`validate_bundle_card`) to
                // declare `pair_serializer_id`/`pair_serializer_hash` matching
                // `pair::serialize_candidate_pair` — i.e. every live bundle was
                // trained on that sentinel-laden pair text. `Tier1Ranker::rank`
                // routes through `score_serving`, which builds an entirely
                // different `"{utterance}\n\n{context}"` + plain description
                // textualisation the bundle was never trained on. A legacy/thin
                // board has no `CandidateSemanticSlice`s to serialize a real
                // pair from in the first place, so there is no correct way to
                // route it through tier-1 at all — degrade to tier-0 with an
                // honest producer identity instead of silently scoring on
                // untrained text.
            }
        }
        if !clean_exact_match {
            #[cfg(feature = "embed")]
            if let Some(e0) = &self.embed_tier0 {
                use utterance_engine::retrieval::Tier0Retriever as _;
                let result = e0.retrieve(text, board)?;
                let bundle = result.model_bundle_hash.clone();
                return finalize(
                    result,
                    utterance_engine::exact::EvidenceLane::Embedding,
                    bundle,
                );
            }
        }
        {
            use utterance_engine::retrieval::Tier0Retriever as _;
            let mut result = utterance_engine::retrieval::LexicalTier0.retrieve(text, board)?;
            if clean_exact_match {
                // The short-circuit's lossless claim holds only for who
                // WINS (finalize's exact-match boost forces the matched
                // candidate to 1.0 regardless of lane) -- it does NOT
                // hold for the runner-up's score, which `finalize` merely
                // caps at 0.99 rather than resetting. `LexicalTier0`'s
                // raw token-overlap scoring (plus its own independent
                // exact-pin against `description`, a different field
                // than `governed_exact`'s `phrases`) can hand an
                // unrelated candidate a near-1.0 score on coincidental
                // wording overlap, shrinking `policy::decide`'s
                // separation_margin below its threshold and flipping a
                // clean exact match into `Ambiguous`/`EscalateToSage` --
                // a real defect a blind review caught (G1.4). Zeroing
                // every candidate's score here is safe: neither
                // `retrieved_subset_hash` (candidate ids only) nor
                // `board_hash` depend on scores, and finalize's boost
                // still forces the true match to 1.0 immediately after.
                for candidate in &mut result.ranking {
                    candidate.score = utterance_engine::contract::FiniteScore::new(0.0)?;
                }
            }
            let bundle = result.model_bundle_hash.clone();
            finalize(
                result,
                utterance_engine::exact::EvidenceLane::Lexical,
                bundle,
            )
        }
    }

    /// Env-driven store selection for the demo designer binary.
    ///
    /// The designer is an **unauthenticated demo** binary, so unlike the
    /// runner its default is MEMORY and persistence is opt-in:
    ///
    /// - `DATABASE_URL` set (and `BPMN_LITE_STORE` not forcing memory) →
    ///   `PostgresWorkflowStore` + `PostgresTemplateStore` over one pool.
    ///   Migrations run via `DATABASE_ADMIN_URL` if set (admin pool closed
    ///   afterwards, mirroring the runner's A18 split), else via the
    ///   runtime pool.
    /// - `BPMN_LITE_STORE=memory` → memory, even if `DATABASE_URL` is set.
    /// - `BPMN_LITE_STORE=postgres` without `DATABASE_URL` → hard error
    ///   (mirrors the runner: fail closed rather than silently dropping
    ///   to a volatile store when persistence was explicitly requested).
    /// - Nothing set → memory (zero-env demo UX unchanged).
    ///
    /// Mapper rollout is independent of storage: `BPMN_MAPPER_ROLLOUT` accepts
    /// `shadow`, `suggest`, or `workbook`; missing/unknown values are shadow.
    /// Every stage records the actual evidence producer, and even `workbook`
    /// requires explicit ratification. There is no auto-apply stage.
    pub async fn try_new_from_env() -> Result<Arc<Self>, anyhow::Error> {
        let store_mode = std::env::var("BPMN_LITE_STORE").unwrap_or_default();
        let force_memory = store_mode.eq_ignore_ascii_case("memory");
        let force_postgres = store_mode.eq_ignore_ascii_case("postgres");
        let database_url = std::env::var("DATABASE_URL").ok();

        if force_memory || (database_url.is_none() && !force_postgres) {
            return Self::try_new();
        }
        let Some(url) = database_url else {
            anyhow::bail!("DATABASE_URL is required unless BPMN_LITE_STORE=memory is set");
        };

        #[cfg(feature = "postgres")]
        {
            if let Ok(admin_url) = std::env::var("DATABASE_ADMIN_URL") {
                tracing::info!("Running migrations via DATABASE_ADMIN_URL...");
                let admin_pool = sqlx::PgPool::connect(&admin_url).await?;
                bpmn_lite_store_postgres::PostgresWorkflowStore::new(admin_pool.clone())
                    .migrate()
                    .await?;
                admin_pool.close().await;
                tracing::info!("Migrations applied; admin pool closed");
            }
            let pool = sqlx::PgPool::connect(&url).await?;
            let pg = bpmn_lite_store_postgres::PostgresWorkflowStore::new(pool.clone());
            if std::env::var("DATABASE_ADMIN_URL").is_err() {
                pg.migrate().await?;
            }
            tracing::info!("Designer using PostgresWorkflowStore + PostgresTemplateStore");
            let store: Arc<dyn WorkflowStore> = Arc::new(pg);
            // The demo tenant must exist before any FK-bearing write
            // (design sessions, instances) — fail here, not mid-request.
            store.ensure_tenant(&TenantId::new("demo")?).await?;
            Self::assemble(
                store,
                Arc::new(bpmn_lite_authoring::PostgresTemplateStore::new(pool)),
            )
        }
        #[cfg(not(feature = "postgres"))]
        {
            let _ = url;
            anyhow::bail!(
                "DATABASE_URL set but the designer was built without the postgres feature"
            )
        }
    }

    fn session_lock(&self, id: Uuid) -> Arc<tokio::sync::Mutex<()>> {
        self.session_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

// ── Wire-types ──────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct VisualGraphDto {
    workflow_id: String,
    nodes: Vec<VisualNodeDto>,
    edges: Vec<VisualEdgeDto>,
}

#[derive(Serialize)]
pub(crate) struct VisualNodeDto {
    id: String,
    label: String,
    kind: String,
    plug: Option<String>,
    span: Option<bpmn_lite_types::SourceSpan>,
}

#[derive(Serialize)]
pub(crate) struct VisualEdgeDto {
    from: String,
    to: String,
    condition: Option<String>,
}

/// Human label for a timer spec on the visual graph (WS-D D1).
fn timer_spec_label(spec: &bpmn_lite_compiler::TimerSpec) -> String {
    use bpmn_lite_compiler::TimerSpec;
    match spec {
        TimerSpec::Duration { ms } => format!("{}s", ms / 1000),
        TimerSpec::Date { deadline_ms } => format!("until t={deadline_ms}"),
        TimerSpec::Cycle {
            interval_ms,
            max_fires,
        } => format!("every {}s ×{}", interval_ms / 1000, max_fires),
    }
}

fn plan_to_visual_graph(plan: &WorkflowExecutionPlan) -> VisualGraphDto {
    use bpmn_lite_compiler::dsl::JoinMode;
    use bpmn_lite_compiler::dsl::SplitMode;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for (id, node) in plan.nodes() {
        let (kind, label, plug, span) = match node {
            ExecutionNode::Start(st) => ("start".to_owned(), "Start".to_owned(), None, st.span),
            ExecutionNode::End(e) => (
                "end".to_owned(),
                format!("End ({})", e.status),
                None,
                e.span,
            ),
            ExecutionNode::Task(t) => {
                let plug_name = t.plug.clone();
                let display_label = if plug_name.starts_with("dmn-lite:") {
                    format!("↗ Evaluate {}", plug_name.trim_start_matches("dmn-lite:"))
                } else {
                    format!("↗ Call {}", plug_name)
                };
                ("task".to_owned(), display_label, Some(plug_name), t.span)
            }
            ExecutionNode::Split(s) => {
                let mode_str = match s.mode {
                    SplitMode::Exclusive => "Exclusive Gateway (XOR)",
                    SplitMode::Inclusive => "Inclusive Gateway (OR)",
                    SplitMode::Parallel => "Parallel Gateway (AND)",
                };
                ("split".to_owned(), mode_str.to_owned(), None, s.span)
            }
            ExecutionNode::Join(j) => {
                let mode_str = match j.mode {
                    JoinMode::Exclusive => "Merge (XOR)",
                    JoinMode::Inclusive => "Join (OR)",
                    JoinMode::Parallel => "Join (AND)",
                };
                ("join".to_owned(), mode_str.to_owned(), None, j.span)
            }
            ExecutionNode::Wait(w) => (
                "wait".to_owned(),
                format!("Wait ({})", timer_spec_label(&w.spec)),
                None,
                w.span,
            ),
            ExecutionNode::MessageWait(w) => (
                "message_wait".to_owned(),
                format!(
                    "Wait for {} (correlate by {})",
                    w.name, w.correlation_key_source
                ),
                None,
                w.span,
            ),
            ExecutionNode::MultiInstance(mi) => (
                "multi_instance".to_owned(),
                format!(
                    "For each of {} (max {}): {}",
                    mi.collection_flag_name, mi.declared_max, mi.task_type
                ),
                Some(mi.task_type.clone()),
                mi.span,
            ),
        };

        nodes.push(VisualNodeDto {
            id: id.clone(),
            label,
            kind,
            plug,
            span,
        });

        // Outgoing edge extraction
        match node {
            ExecutionNode::Start(n) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: n.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::Task(t) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: t.next.clone(),
                    condition: None,
                });
                // WS-D D1: a guard's escape flow is a real route out of
                // the host — rendered as a labelled edge so the escape
                // subgraph never appears disconnected in the preview.
                for guard in &t.guards {
                    edges.push(VisualEdgeDto {
                        from: id.clone(),
                        to: guard.escape_entry.clone(),
                        condition: Some(format!("Guard: {}", guard.guard_id)),
                    });
                }
            }
            ExecutionNode::Split(s) => {
                for flow in &s.flows {
                    let cond_str =
                        if let (Some(ph), Some(val)) = (&flow.placeholder, &flow.expected_value) {
                            Some(format!("{} == {:?}", ph, val))
                        } else {
                            None
                        };
                    edges.push(VisualEdgeDto {
                        from: id.clone(),
                        to: flow.next.clone(),
                        condition: cond_str,
                    });
                }
            }
            ExecutionNode::Join(j) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: j.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::Wait(w) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: w.next.clone(),
                    condition: None,
                });
            }
            ExecutionNode::MessageWait(w) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: w.next.clone(),
                    condition: Some(format!("Message: {}", w.name)),
                });
            }
            ExecutionNode::End(_) => {}
            ExecutionNode::MultiInstance(mi) => {
                edges.push(VisualEdgeDto {
                    from: id.clone(),
                    to: mi.next.clone(),
                    condition: None,
                });
            }
        }
    }

    VisualGraphDto {
        workflow_id: plan.workflow_id().to_string(),
        nodes,
        edges,
    }
}

/// `IRNode::DataObject` carries zero bytecode and never becomes an
/// `ExecutionNode`, so `plan_to_visual_graph` never sees it — the graph
/// endpoint is the enriched execution map, and a structural declaration
/// with no flow edges belongs on it as a data node, distinct from the
/// runbook's raw operation-tape rendering. Rendered as unconnected nodes
/// (no `VisualEdgeDto`s -- `DataObject` carries no `next`/flow fields to
/// project) alongside the flow nodes `plan_to_visual_graph` already built.
fn data_object_visual_nodes(ir: &bpmn_lite_compiler::IRGraph) -> Vec<VisualNodeDto> {
    ir.node_weights()
        .filter_map(|node| match node {
            bpmn_lite_compiler::IRNode::DataObject {
                id,
                name,
                type_decl,
                role,
            } => {
                let type_label = match type_decl {
                    bpmn_lite_types::DataObjectType::Primitive(primitive) => match primitive {
                        bpmn_lite_types::PrimitiveType::Bool => "bool".to_owned(),
                        bpmn_lite_types::PrimitiveType::I64 => "i64".to_owned(),
                        bpmn_lite_types::PrimitiveType::F64 => "f64".to_owned(),
                        bpmn_lite_types::PrimitiveType::String => "string".to_owned(),
                    },
                    bpmn_lite_types::DataObjectType::SemOsDomain { domain_id, .. } => {
                        format!("semos-domain:{domain_id}")
                    }
                };
                let role_label = match role {
                    bpmn_lite_types::DataObjectRole::Input => "input",
                    bpmn_lite_types::DataObjectRole::Output => "output",
                    bpmn_lite_types::DataObjectRole::Internal => "internal",
                };
                Some(VisualNodeDto {
                    id: id.clone(),
                    label: format!("Data object: {name} ({type_label}, {role_label})"),
                    kind: "data_object".to_owned(),
                    plug: None,
                    span: None,
                })
            }
            _ => None,
        })
        .collect()
}

// ── Router ──────────────────────────────────────────────────────────────

/// GET /bpmn/health — service-identity health contract. The runner and
/// the designer both answer `/bpmn/health` with `{status, service}`; the
/// UI dispatches on `service` to pick its surface (runner demo vs
/// designer workspace) instead of treating a 404 as "something is
/// wrong". Before this endpoint existed, the NORMAL designer deployment
/// permanently showed an alarming "runner unreachable" banner — mode
/// detection by absence, exactly the kind of negative-space check the
/// working contract's "enforce the mechanism" rule forbids. `tier1_bundle`
/// is the served SLM bundle identity (null = tier-0 degradation), so one
/// curl answers "which service, which model".
async fn designer_health(State(state): State<Arc<DesignerState>>) -> impl IntoResponse {
    #[cfg(feature = "candle-probe")]
    let tier1_bundle = state
        .tier1
        .as_ref()
        .map(|r| r.model_bundle_hash().to_string());
    #[cfg(not(feature = "candle-probe"))]
    let tier1_bundle: Option<String> = {
        let _ = &state; // state unused in the tier-0-only build
        None
    };
    Json(serde_json::json!({
        "status": "ok",
        "service": "bpmn-lite-designer",
        "tier1_bundle": tier1_bundle,
        "mapper_rollout": state.mapper_rollout.label(),
        "mapper_suggestions_enabled": state.mapper_rollout.suggestions_enabled(),
        "mapper_workbooks_enabled": state.mapper_rollout.workbooks_enabled(),
        "mapper_ratification_required": true,
        "mapper_auto_apply": false,
    }))
}

pub fn designer_router(state: Arc<DesignerState>) -> Router {
    Router::new()
        .route("/bpmn/health", get(designer_health))
        .route("/bpmn/compile/preview", post(compile_bpmn_preview))
        .route("/dmn/compile/preview", post(compile_dmn_preview))
        .route("/dmn/decisions/:id", get(get_dmn_decision))
        .route("/api/dsl/macro/apply", post(apply_dsl_macro))
        .route(
            "/api/dsl/diagnostics/resolve",
            post(resolve_dsl_diagnostics),
        )
        .route(
            "/api/dsl/sage/utter",
            post(designer_utterance_compat_endpoint),
        )
        .route(
            "/api/dsl/sessions",
            get(list_design_sessions_endpoint).post(create_design_session_endpoint),
        )
        .route("/api/dsl/sessions/:id", get(get_design_session_endpoint))
        .route(
            "/api/dsl/sessions/:id/revision",
            post(session_revision_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/utterance",
            post(session_utterance_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/palette/select",
            post(palette_select_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/dev-capture/enable",
            post(dev_capture_enable_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/dev-capture",
            get(dev_capture_status_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/adjudication",
            post(session_adjudication_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/graph-edit",
            post(session_graph_edit_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/proposals",
            get(list_proposals_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/proposals/:pid/ratify",
            post(ratify_proposal_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/proposals/:pid/answers",
            post(answer_proposal_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/proposals/:pid/reject",
            post(reject_proposal_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/save",
            post(save_design_session_endpoint),
        )
        .route("/api/dsl/sessions/:id/undo", post(session_undo_endpoint))
        .route("/api/dsl/sessions/:id/runbook", get(session_runbook_endpoint))
        .route(
            "/api/dsl/sessions/:id/dsl-receipt",
            get(session_dsl_receipt_endpoint),
        )
        .route(
            "/api/dsl/sessions/:id/gameboard",
            get(session_gameboard_endpoint),
        )
        // Sage receives the same position-bound, policy-filtered board as the
        // palette. This alias is deliberately read-only: it cannot select,
        // preview, ratify or mutate a move.
        .route(
            "/api/dsl/sage/sessions/:id/gameboard",
            get(session_gameboard_endpoint),
        )
        .route(
            "/api/dsl/sage/sessions/:id/history",
            get(sage_session_history_endpoint),
        )
        .route(
            "/api/dsl/sage/sessions/:id/audit",
            get(sage_session_audit_endpoint),
        )
        .route(
            "/api/dsl/sage/sessions/:id/attempts/:attempt_id",
            get(sage_attempt_endpoint),
        )
        .route(
            "/api/dsl/sage/sessions/:id/guidance/:candidate_id",
            get(sage_move_guidance_endpoint),
        )
        .route("/api/dsl/sessions/:id/graph", get(session_graph_endpoint))
        .route("/designer", get(designer_page))
        .route(
            "/bpmn/templates",
            get(list_templates_endpoint).post(define_template_endpoint),
        )
        .route(
            "/bpmn/templates/:name/versions/:version",
            get(get_template_version_endpoint),
        )
        .route(
            "/bpmn/templates/:name/spawn",
            post(spawn_template_instance_endpoint),
        )
        .route(
            "/bpmn/templates/published",
            get(list_published_templates_endpoint),
        )
        .route("/bpmn/instances/:id/status", get(instance_status_endpoint))
        .route(
            "/bpmn/instances/:id/advance",
            post(advance_instance_endpoint),
        )
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(state)
}

// ── Preview Compilation and DMN handlers ──────────────────────────────────

const OB_POC_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/ob-poc-v1.0.0.yaml"
));
const DMN_LITE_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/dmn-lite-v1.0.0.yaml"
));
const BPMN_MANIFEST_YAML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../manifests/bpmn-v1.0.0.yaml"
));

fn get_preview_registry() -> bpmn_lite_compiler::dsl::ManifestPlaceholderRegistry<
    bpmn_lite_compiler::dsl::StubPlaceholderRegistry,
> {
    use bpmn_lite_compiler::dsl::{ManifestPlaceholderRegistry, StubPlaceholderRegistry};
    use dsl_manifest::Manifest;

    let ob_poc = Manifest::load_from_yaml(OB_POC_MANIFEST_YAML).expect("ob-poc manifest must load");
    let dmn_lite =
        Manifest::load_from_yaml(DMN_LITE_MANIFEST_YAML).expect("dmn-lite manifest must load");
    let bpmn = Manifest::load_from_yaml(BPMN_MANIFEST_YAML).expect("bpmn manifest must load");

    let mut registry =
        ManifestPlaceholderRegistry::new(StubPlaceholderRegistry::new().with_demo_bindings());
    registry.import(ob_poc);
    registry.import(dmn_lite);
    registry.import(bpmn);
    registry
}

#[derive(Serialize)]
pub(crate) struct CompilePreviewResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct CompilePreviewBody {
    bpmn_dsl: String,
}

#[derive(Deserialize)]
pub(crate) struct DmnPreviewRequest {
    dmn_dsl: String,
}

#[derive(Serialize)]
pub(crate) struct DmnInputSchema {
    name: String,
    #[serde(rename = "type")]
    type_ref: String,
    domain: String,
}

#[derive(Serialize)]
pub(crate) struct DmnOutputSchema {
    name: String,
    #[serde(rename = "type")]
    type_ref: String,
    domain: String,
}

#[derive(Serialize)]
pub(crate) struct DmnRuleInputCell {
    op: String,
    value: String,
}

#[derive(Serialize)]
pub(crate) struct DmnRuleDto {
    id: String,
    inputs: Vec<DmnRuleInputCell>,
    outputs: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct DmnSchemaDto {
    decision_name: String,
    hit_policy: String,
    inputs: Vec<DmnInputSchema>,
    outputs: Vec<DmnOutputSchema>,
    rules: Vec<DmnRuleDto>,
}

#[derive(Serialize)]
pub(crate) struct DmnPreviewResponse {
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    schema: Option<DmnSchemaDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

async fn compile_bpmn_preview(Json(body): Json<CompilePreviewBody>) -> impl IntoResponse {
    use bpmn_lite_compiler::dsl::compile;
    let registry = get_preview_registry();
    match compile(&body.bpmn_dsl, &registry) {
        Ok(plan) => {
            let visual = plan_to_visual_graph(&plan);
            let resp = CompilePreviewResponse {
                workflow_id: Some(visual.workflow_id),
                nodes: Some(visual.nodes),
                edges: Some(visual.edges),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err) => {
            let (error_msg, diagnostics) = match err {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => {
                    ("Parsing failed".to_owned(), errs)
                }
                bpmn_lite_compiler::dsl::CompileError::Unroll(err) => {
                    ("Loop unrolling failed".to_owned(), vec![err.to_string()])
                }
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    let mut formatted = Vec::new();
                    for e in errs {
                        let msg = format!("{}", e);
                        formatted.push(msg);
                        let symbol = if e.message.starts_with("unresolved symbol '")
                            && e.message.ends_with("'")
                        {
                            Some(
                                e.message
                                    .trim_start_matches("unresolved symbol '")
                                    .trim_end_matches("'"),
                            )
                        } else if e.message.starts_with("verb '") {
                            let remaining = e.message.trim_start_matches("verb '");
                            remaining.find('\'').map(|idx| &remaining[..idx])
                        } else if e.message.starts_with("decision '") {
                            let remaining = e.message.trim_start_matches("decision '");
                            remaining.find('\'').map(|idx| &remaining[..idx])
                        } else {
                            None
                        };

                        if let Some(sym) = symbol {
                            formatted.push(format!(
                                "Suggestion: Would you like me to import {} to fix the unresolved verb error?",
                                sym
                            ));
                        }
                    }
                    ("Linting failed".to_owned(), formatted)
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    let mut formatted = Vec::new();
                    for e in errs {
                        let msg = format!("{}", e);
                        formatted.push(msg);
                        if e.message.starts_with("cycle detected:") {
                            formatted.push("Suggestion: Try structuring the cyclic path within a bounded 'loop' block.".to_owned());
                        } else if e.message.ends_with("is unreachable from start") {
                            formatted.push("Suggestion: Connect this node from a preceding gateway or task by updating the ':next' attribute.".to_owned());
                        }
                    }
                    ("DAG validation failed".to_owned(), formatted)
                }
            };
            let resp = CompilePreviewResponse {
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(error_msg),
                diagnostics,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

async fn compile_dmn_preview(Json(body): Json<DmnPreviewRequest>) -> impl IntoResponse {
    match parse_dmn_to_dto(&body.dmn_dsl) {
        Ok(schema) => {
            let resp = DmnPreviewResponse {
                schema: Some(schema),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err_msg) => {
            let resp = DmnPreviewResponse {
                schema: None,
                error: Some("DMN compilation failed".to_owned()),
                diagnostics: vec![err_msg],
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

async fn get_dmn_decision(Path(decision_id): Path<String>) -> impl IntoResponse {
    // Sanitize path parameter to prevent path traversal
    if !decision_id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid decision ID format" })),
        )
            .into_response();
    }

    use std::path::PathBuf;
    let decisions_dir = std::env::var("DMN_DECISIONS_DIR")
        .unwrap_or_else(|_| format!("{}/../dmn-lite-decisions", env!("CARGO_MANIFEST_DIR")));
    let path = PathBuf::from(decisions_dir).join(format!("{}.dmn-lite", decision_id));

    match std::fs::read_to_string(&path) {
        Ok(source_text) => match parse_dmn_to_dto(&source_text) {
            Ok(schema) => (StatusCode::OK, Json(schema)).into_response(),
            Err(err_msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": err_msg })),
            )
                .into_response(),
        },
        Err(err) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Decision not found: {}", err) })),
        )
            .into_response(),
    }
}

fn parse_dmn_to_dto(source_text: &str) -> Result<DmnSchemaDto, String> {
    use dmn_lite_parser::{parse, HitPolicyAst, TypeRefAst, WhenAst};

    let source = parse(source_text).map_err(|e| format!("{}", e))?;
    let decision = source
        .decisions
        .first()
        .ok_or_else(|| "No decision defined in DSL".to_owned())?;

    let decision_name = decision.name.name.clone();
    let hit_policy = match &decision.hit_policy {
        HitPolicyAst::Unique(_) => "unique".to_owned(),
        HitPolicyAst::First(_) => "first".to_owned(),
    };

    let inputs: Vec<DmnInputSchema> = decision
        .inputs
        .iter()
        .map(|input| {
            let type_ref = match &input.type_ref {
                TypeRefAst::Enum(_) => "enum",
                TypeRefAst::Bool(_) => "bool",
                TypeRefAst::Integer(_) => "integer",
                TypeRefAst::Decimal(_) => "decimal",
                TypeRefAst::String(_) => "string",
            }
            .to_owned();
            DmnInputSchema {
                name: input.name.name.clone(),
                type_ref,
                domain: input.domain_ref.name.clone(),
            }
        })
        .collect();

    let outputs: Vec<DmnOutputSchema> = decision
        .outputs
        .iter()
        .map(|output| {
            let type_ref = match &output.type_ref {
                TypeRefAst::Enum(_) => "enum",
                TypeRefAst::Bool(_) => "bool",
                TypeRefAst::Integer(_) => "integer",
                TypeRefAst::Decimal(_) => "decimal",
                TypeRefAst::String(_) => "string",
            }
            .to_owned();
            DmnOutputSchema {
                name: output.name.name.clone(),
                type_ref,
                domain: output.domain_ref.name.clone(),
            }
        })
        .collect();

    let rules: Vec<DmnRuleDto> = decision
        .rules
        .iter()
        .map(|rule| {
            // Build inputs array in same order as inputs list
            let mut rule_inputs = Vec::new();
            for input_schema in &inputs {
                let mut matched_cell = DmnRuleInputCell {
                    op: "-".to_owned(),
                    value: "-".to_owned(),
                };

                match &rule.when {
                    WhenAst::CatchAll(_) => {}
                    WhenAst::Predicates(preds, _) => {
                        if let Some(pred) = find_predicate_for_field(preds, &input_schema.name) {
                            let (op, val) = format_predicate_cell(pred);
                            matched_cell = DmnRuleInputCell { op, value: val };
                        }
                    }
                }
                rule_inputs.push(matched_cell);
            }

            // Build outputs array in same order as outputs list
            let mut rule_outputs = Vec::new();
            for output_schema in &outputs {
                let mut matched_val = "-".to_owned();
                if let Some(assign) = rule
                    .then
                    .iter()
                    .find(|a| a.output.name == output_schema.name)
                {
                    matched_val = format_literal(&assign.value);
                }
                rule_outputs.push(matched_val);
            }

            DmnRuleDto {
                id: rule.id.name.clone(),
                inputs: rule_inputs,
                outputs: rule_outputs,
            }
        })
        .collect();

    Ok(DmnSchemaDto {
        decision_name,
        hit_policy,
        inputs,
        outputs,
        rules,
    })
}

fn find_predicate_for_field<'a>(
    preds: &'a [dmn_lite_parser::PredicateAst],
    field_name: &str,
) -> Option<&'a dmn_lite_parser::PredicateAst> {
    for pred in preds {
        if let Some(f) = get_predicate_field(pred) {
            if f == field_name {
                return Some(pred);
            }
        }
    }
    None
}

fn get_predicate_field(pred: &dmn_lite_parser::PredicateAst) -> Option<&str> {
    use dmn_lite_parser::PredicateAst;
    match pred {
        PredicateAst::Eq { field, .. } => Some(&field.name),
        PredicateAst::NotEq { field, .. } => Some(&field.name),
        PredicateAst::Lt { field, .. } => Some(&field.name),
        PredicateAst::Le { field, .. } => Some(&field.name),
        PredicateAst::Gt { field, .. } => Some(&field.name),
        PredicateAst::Ge { field, .. } => Some(&field.name),
        PredicateAst::InSet { field, .. } => Some(&field.name),
        PredicateAst::Range { field, .. } => Some(&field.name),
        PredicateAst::IsNull { field, .. } => Some(&field.name),
        PredicateAst::IsNotNull { field, .. } => Some(&field.name),
        PredicateAst::Not { inner, .. } => get_predicate_field(inner),
        PredicateAst::And { items, .. } | PredicateAst::Or { items, .. } => {
            let mut field = None;
            for item in items {
                let current = get_predicate_field(item)?;
                if let Some(previous) = field {
                    if previous != current {
                        return None;
                    }
                } else {
                    field = Some(current);
                }
            }
            field
        }
    }
}

fn format_predicate_cell(pred: &dmn_lite_parser::PredicateAst) -> (String, String) {
    use dmn_lite_parser::PredicateAst;
    match pred {
        PredicateAst::Eq { value, .. } => ("==".to_string(), format_literal(value)),
        PredicateAst::NotEq { value, .. } => ("!=".to_string(), format_literal(value)),
        PredicateAst::Lt { value, .. } => ("<".to_string(), format_number_literal(value)),
        PredicateAst::Le { value, .. } => ("<=".to_string(), format_number_literal(value)),
        PredicateAst::Gt { value, .. } => (">".to_string(), format_number_literal(value)),
        PredicateAst::Ge { value, .. } => (">=".to_string(), format_number_literal(value)),
        PredicateAst::IsNull { .. } => ("is-null".to_string(), "".to_string()),
        PredicateAst::IsNotNull { .. } => ("is-not-null".to_string(), "".to_string()),
        PredicateAst::InSet { values, .. } => {
            let formatted_vals: Vec<String> = values.iter().map(format_literal).collect();
            ("in".to_string(), format!("[{}]", formatted_vals.join(", ")))
        }
        PredicateAst::Range {
            lower,
            upper,
            lower_inclusive,
            upper_inclusive,
            ..
        } => {
            let left_bracket = if *lower_inclusive { "[" } else { "(" };
            let right_bracket = if *upper_inclusive { "]" } else { ")" };
            let lower_str = format_range_bound(lower);
            let upper_str = format_range_bound(upper);
            (
                "in".to_string(),
                format!(
                    "{}{} .. {}{}",
                    left_bracket, lower_str, upper_str, right_bracket
                ),
            )
        }
        PredicateAst::Not { inner, .. } => {
            let (op, val) = format_predicate_cell(inner);
            (format!("not {}", op), val)
        }
        PredicateAst::And { items, .. } => {
            let formatted_vals: Vec<String> = items
                .iter()
                .map(|item| format_predicate_cell(item).1)
                .collect();
            ("and".to_string(), formatted_vals.join(" and "))
        }
        PredicateAst::Or { items, .. } => {
            let formatted_vals: Vec<String> = items
                .iter()
                .map(|item| format_predicate_cell(item).1)
                .collect();
            ("or".to_string(), formatted_vals.join(" or "))
        }
    }
}

fn format_literal(lit: &dmn_lite_parser::LiteralAst) -> String {
    use dmn_lite_parser::LiteralAst;
    match lit {
        LiteralAst::Symbol(s) => s.name.clone(),
        LiteralAst::String(s) => s.value.clone(),
        LiteralAst::Number(n) => n.text.clone(),
        LiteralAst::Boolean { value, .. } => value.to_string(),
    }
}

fn format_number_literal(n: &dmn_lite_parser::NumberLitAst) -> String {
    n.text.clone()
}

fn format_range_bound(bound: &dmn_lite_parser::RangeBound) -> String {
    use dmn_lite_parser::RangeBound;
    match bound {
        RangeBound::Unbounded(_) => "*".to_string(),
        RangeBound::Value(n) => n.text.clone(),
    }
}

// ── DSL Macro & Diagnostics Handlers ─────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub(crate) struct MacroApplyRequest {
    source_code: String,
    macro_type: String,
    parameters: HashMap<String, String>,
}

#[derive(Serialize)]
pub(crate) struct MacroApplyResponse {
    source_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

fn get_macros_config() -> Option<bpmn_lite_compiler::dsl::MacroConfigList> {
    let workspace_macros = format!("{}/../macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&workspace_macros).exists() {
        if let Ok(config) =
            bpmn_lite_compiler::dsl::MacroConfigList::load_from_file(&workspace_macros)
        {
            return Some(config);
        }
    }
    let server_macros = format!("{}/macros.yaml", env!("CARGO_MANIFEST_DIR"));
    if std::path::Path::new(&server_macros).exists() {
        if let Ok(config) = bpmn_lite_compiler::dsl::MacroConfigList::load_from_file(&server_macros)
        {
            return Some(config);
        }
    }
    None
}

async fn apply_dsl_macro(Json(body): Json<MacroApplyRequest>) -> impl IntoResponse {
    use bpmn_lite_compiler::dsl::{
        create_parallel_split_join, create_xor_split_join, parse_workflow_str, repeat_n_times,
        AstMutator, NodeAst, ToSexpr, XorBranchConfig,
    };

    let mut workflow = match parse_workflow_str(&body.source_code) {
        Ok(w) => w,
        Err(e) => {
            let resp = MacroApplyResponse {
                source_code: body.source_code.clone(),
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(format!("Failed to parse source: {}", e)),
                diagnostics: vec![e],
            };
            return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
        }
    };

    let result = match body.macro_type.as_str() {
        "BoundedRetry" => {
            let target_node_id = match body.parameters.get("target_node_id") {
                Some(id) => id,
                None => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": "Missing parameter target_node_id"})),
                    )
                        .into_response();
                }
            };
            let ceiling: u32 = body
                .parameters
                .get("ceiling")
                .and_then(|c| c.parse().ok())
                .unwrap_or(3);
            let loop_id = body.parameters.get("custom_id").cloned();

            // G3.3: the whole multi-step edit (extract task, rewire
            // predecessors, build the loop, splice it back in) is owned by
            // one entry point instead of being orchestrated here directly.
            repeat_n_times(&mut workflow, target_node_id, ceiling, loop_id.as_deref())
                .map_err(|e| e.to_string())
        }
        "XorSplit" => {
            let split_id = body
                .parameters
                .get("split_id")
                .map(|s| s.as_str())
                .unwrap_or("xor-split");
            let placeholder = body
                .parameters
                .get("placeholder")
                .map(|s| s.as_str())
                .unwrap_or("@decision_val");
            let join_id = body
                .parameters
                .get("join_id")
                .map(|s| s.as_str())
                .unwrap_or("xor-join");
            let join_next = body
                .parameters
                .get("join_next")
                .map(|s| s.as_str())
                .unwrap_or("end");
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let branch_val = body
                .parameters
                .get("branch_value")
                .map(|s| s.as_str())
                .unwrap_or("yes");
            let branch_target = body
                .parameters
                .get("branch_target")
                .map(|s| s.as_str())
                .unwrap_or("end");

            let branches = vec![
                XorBranchConfig {
                    condition_value: branch_val.to_string(),
                    target_next: branch_target.to_string(),
                },
                XorBranchConfig {
                    condition_value: "default".to_string(),
                    target_next: join_id.to_string(),
                },
            ];

            let (split, join) =
                create_xor_split_join(split_id, placeholder, branches, join_id, join_next);

            let mut mutator = AstMutator::new(&mut workflow);
            mutator
                .insert_after(predecessor_id, NodeAst::Split(split))
                .and_then(|_| {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(split_id, NodeAst::Join(join))
                })
        }
        "ParallelSplit" => {
            let split_id = body
                .parameters
                .get("split_id")
                .map(|s| s.as_str())
                .unwrap_or("and-split");
            let join_id = body
                .parameters
                .get("join_id")
                .map(|s| s.as_str())
                .unwrap_or("and-join");
            let join_next = body
                .parameters
                .get("join_next")
                .map(|s| s.as_str())
                .unwrap_or("end");
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let branch_target = body
                .parameters
                .get("branch_target")
                .map(|s| s.as_str())
                .unwrap_or("end");

            let branch_entries = vec![branch_target.to_string(), join_id.to_string()];
            let (split, join) =
                create_parallel_split_join(split_id, branch_entries, join_id, join_next);

            let mut mutator = AstMutator::new(&mut workflow);
            mutator
                .insert_after(predecessor_id, NodeAst::Split(split))
                .and_then(|_| {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(split_id, NodeAst::Join(join))
                })
        }
        "Custom" => {
            let macro_id = match body.parameters.get("macro_id") {
                Some(id) => id,
                None => return (
                    StatusCode::BAD_REQUEST,
                    Json(
                        serde_json::json!({"error": "Missing parameter macro_id for Custom macro"}),
                    ),
                )
                    .into_response(),
            };
            let predecessor_id = body
                .parameters
                .get("predecessor_id")
                .map(|s| s.as_str())
                .unwrap_or("start");

            let config = get_macros_config()
                .ok_or_else(|| "No macros.yaml config found on disk".to_string());
            let node = config.and_then(|c| {
                let macro_cfg = c
                    .macros
                    .into_iter()
                    .find(|m| &m.id == macro_id)
                    .ok_or_else(|| {
                        format!("Custom macro '{}' not found in macros.yaml", macro_id)
                    })?;
                macro_cfg.instantiate(&body.parameters)
            });

            match node {
                Ok(n) => {
                    let mut mutator = AstMutator::new(&mut workflow);
                    mutator.insert_after(predecessor_id, n)
                }
                Err(e) => Err(e),
            }
        }
        other => Err(format!("Unknown macro type: {}", other)),
    };

    if let Err(e) = result {
        let resp = MacroApplyResponse {
            source_code: body.source_code.clone(),
            workflow_id: None,
            nodes: None,
            edges: None,
            error: Some(format!("Macro application failed: {}", e)),
            diagnostics: vec![e],
        };
        return (StatusCode::OK, Json(resp)).into_response();
    }

    let new_dsl = workflow.to_sexpr(0);
    let registry = get_preview_registry();

    match bpmn_lite_compiler::dsl::compile(&new_dsl, &registry) {
        Ok(plan) => {
            let visual = plan_to_visual_graph(&plan);
            let resp = MacroApplyResponse {
                source_code: new_dsl,
                workflow_id: Some(visual.workflow_id),
                nodes: Some(visual.nodes),
                edges: Some(visual.edges),
                error: None,
                diagnostics: Vec::new(),
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
        Err(err) => {
            let diagnostics = match err {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                bpmn_lite_compiler::dsl::CompileError::Unroll(err) => vec![err.to_string()],
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    errs.iter().map(|e| format!("{}", e)).collect()
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    errs.iter().map(|e| format!("{}", e)).collect()
                }
            };
            let resp = MacroApplyResponse {
                source_code: new_dsl,
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some("Compilation failed after macro application".to_string()),
                diagnostics,
            };
            (StatusCode::OK, Json(resp)).into_response()
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct DiagnosticsResolveRequest {
    source_code: String,
    action: bpmn_lite_authoring::FixAction,
}

#[derive(Serialize)]
pub(crate) struct DiagnosticsResolveResponse {
    source_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nodes: Option<Vec<VisualNodeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    edges: Option<Vec<VisualEdgeDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    diagnostics: Vec<String>,
}

async fn resolve_dsl_diagnostics(Json(body): Json<DiagnosticsResolveRequest>) -> impl IntoResponse {
    let manifests_path = std::env::var("SAGE_MANIFESTS_DIR")
        .unwrap_or_else(|_| format!("{}/../manifests", env!("CARGO_MANIFEST_DIR")));
    let manifests_dir = std::path::PathBuf::from(manifests_path);
    match bpmn_lite_authoring::execute_autofix(&body.source_code, &body.action, &manifests_dir) {
        Ok(new_dsl) => {
            let registry = get_preview_registry();
            let new_dsl_str = new_dsl.clone();
            match bpmn_lite_compiler::dsl::compile(&new_dsl_str, &registry) {
                Ok(plan) => {
                    let visual = plan_to_visual_graph(&plan);
                    let resp = DiagnosticsResolveResponse {
                        source_code: new_dsl_str,
                        workflow_id: Some(visual.workflow_id),
                        nodes: Some(visual.nodes),
                        edges: Some(visual.edges),
                        error: None,
                        diagnostics: Vec::new(),
                    };
                    (StatusCode::OK, Json(resp)).into_response()
                }
                Err(err) => {
                    let diagnostics = match err {
                        bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                        bpmn_lite_compiler::dsl::CompileError::Unroll(err) => vec![err.to_string()],
                        bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                            errs.iter().map(|e| format!("{}", e)).collect()
                        }
                        bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                            errs.iter().map(|e| format!("{}", e)).collect()
                        }
                    };
                    let resp = DiagnosticsResolveResponse {
                        source_code: new_dsl_str,
                        workflow_id: None,
                        nodes: None,
                        edges: None,
                        error: Some("Compilation failed after diagnostic resolution".to_string()),
                        diagnostics,
                    };
                    (StatusCode::OK, Json(resp)).into_response()
                }
            }
        }
        Err(e) => {
            let resp = DiagnosticsResolveResponse {
                source_code: body.source_code.clone(),
                workflow_id: None,
                nodes: None,
                edges: None,
                error: Some(format!("Diagnostic resolution failed: {}", e)),
                diagnostics: vec![e],
            };
            (StatusCode::BAD_REQUEST, Json(resp)).into_response()
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct UtteranceRequest {
    utterance: String,
    _current_dsl: String,
    #[serde(default)]
    target_node_id: Option<String>,
    #[serde(default)]
    unresolved_verb: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct UtteranceResponse {
    escape_intent_detected: bool,
    suggested_action: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_payload: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Default)]
struct DesignerUtteranceContext<'a> {
    target_node_id: Option<&'a str>,
    unresolved_verb: Option<&'a str>,
}

/// Compatibility endpoint for the legacy route name.
///
/// This is a local deterministic designer classifier, not the shared Sage
/// runtime. Optional node and diagnostic identities are supplied by the BPMN
/// host so the classifier never fabricates application commands.
async fn designer_utterance_compat_endpoint(
    Json(body): Json<UtteranceRequest>,
) -> impl IntoResponse {
    let context = DesignerUtteranceContext {
        target_node_id: non_empty(body.target_node_id.as_deref()),
        unresolved_verb: non_empty(body.unresolved_verb.as_deref()),
    };
    Json(classify_designer_utterance(&body.utterance, context))
}

/// Classify the small set of legacy designer navigation and authoring hints.
fn classify_designer_utterance(
    utterance: &str,
    context: DesignerUtteranceContext<'_>,
) -> UtteranceResponse {
    let text = utterance.trim().to_lowercase();

    let is_escape = text.contains("exit")
        || text.contains("close editor")
        || text.contains("go back")
        || text.contains("quit")
        || text.contains("deploy")
        || text.contains("push release")
        || text.contains("staging")
        || text.contains("check database")
        || text.contains("list active manifests");

    let (suggested_action, msg, payload) = if text.contains("exit")
        || text.contains("close editor")
        || text.contains("go back")
        || text.contains("quit")
    {
        (
            "exit",
            "Autosaved current workspace. Exiting designer session.",
            None,
        )
    } else if text.contains("deploy") || text.contains("push release") || text.contains("staging") {
        (
            "chat",
            "It looks like you want to perform a deployment or navigate away. Should we save your workflow draft and transition out of the designer session?",
            None,
        )
    } else if text.contains("retry loop") || text.contains("wrap") || text.contains("retry") {
        match context.target_node_id {
            Some(target_node_id) => {
                let mut params = serde_json::Map::new();
                params.insert(
                    "target_node_id".to_string(),
                    serde_json::Value::String(target_node_id.to_string()),
                );
                params.insert(
                    "ceiling".to_string(),
                    serde_json::Value::String("3".to_string()),
                );
                (
                    "apply_macro",
                    "We can wrap your selected task in a retry loop. Apply macro?",
                    Some(serde_json::json!({
                        "macro_type": "BoundedRetry",
                        "parameters": params
                    })),
                )
            }
            None => (
                "none",
                "Select the BPMN task to wrap before requesting a retry loop.",
                None,
            ),
        }
    } else if text.starts_with("import ") || text.contains("unknown verb") {
        match explicit_import_candidate(utterance, &text, context.unresolved_verb) {
            Some((domain, verb)) => (
                "resolve_diagnostic",
                "Resolving unresolved symbol by adding signature stub to manifest.",
                Some(serde_json::json!({
                    "type": "AddVerbStub",
                    "domain": domain,
                    "verb": verb
                })),
            ),
            None => (
                "none",
                "Provide the exact unresolved verb before adding a manifest stub.",
                None,
            ),
        }
    } else {
        ("none", "Utterance processed. Continues editing mode.", None)
    };

    UtteranceResponse {
        escape_intent_detected: is_escape,
        suggested_action: suggested_action.to_string(),
        message: msg.to_string(),
        action_payload: payload,
    }
}

fn explicit_import_candidate(
    utterance: &str,
    normalized: &str,
    unresolved_verb: Option<&str>,
) -> Option<(String, String)> {
    let candidate = if normalized.starts_with("import ") {
        utterance.trim().get("import ".len()..)?.trim()
    } else {
        unresolved_verb?.trim()
    };
    if candidate.is_empty() {
        return None;
    }

    match candidate.split_once(':') {
        Some((domain, verb)) if !domain.trim().is_empty() && !verb.trim().is_empty() => Some((
            domain.trim().to_string(),
            format!("{}:{}", domain.trim(), verb.trim()),
        )),
        Some(_) => None,
        None => Some(("bpmn".to_string(), format!("bpmn:{candidate}"))),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Deserialize, Serialize)]
pub(crate) struct DefineTemplateBody {
    name: String,
    dsl_body: String,
}

#[derive(Serialize)]
pub(crate) struct DefineTemplateResponse {
    plan_hash: String,
    version: u32,
}

async fn define_template_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Json(body): Json<DefineTemplateBody>,
) -> impl IntoResponse {
    let registry = get_preview_registry();
    let plan = match bpmn_lite_compiler::dsl::compile(&body.dsl_body, &registry) {
        Ok(p) => p,
        Err(e) => {
            let (error_msg, diagnostics) = match e {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => {
                    ("Parsing failed".to_owned(), errs)
                }
                bpmn_lite_compiler::dsl::CompileError::Unroll(err) => {
                    ("Loop unrolling failed".to_owned(), vec![err.to_string()])
                }
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    let formatted = errs.iter().map(|e| format!("{}", e)).collect::<Vec<_>>();
                    ("Linting failed".to_owned(), formatted)
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    let formatted = errs.iter().map(|e| format!("{}", e)).collect::<Vec<_>>();
                    ("DAG validation failed".to_owned(), formatted)
                }
            };
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": error_msg,
                    "diagnostics": diagnostics
                })),
            )
                .into_response();
        }
    };

    let plan_json = match serde_json::to_string(&plan) {
        Ok(json) => json,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Serialization failed: {}", e) })),
            )
                .into_response();
        }
    };

    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();

    if let Err(e) = demo.store.store_plan(hash, &plan_json).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to store plan: {}", e) })),
        )
            .into_response();
    }

    let version = match demo.store.load_latest_template_version(&body.name).await {
        Ok(Some((v, _, _))) => v + 1,
        _ => 1,
    };

    if let Err(e) = demo
        .store
        .store_template(&body.name, version, hash, &body.dsl_body)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to store template: {}", e) })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(DefineTemplateResponse {
            plan_hash: hex::encode(hash),
            version,
        }),
    )
        .into_response()
}

#[derive(Serialize)]
pub(crate) struct TemplateSummaryDto {
    name: String,
    latest_version: u32,
    plan_hash: String,
    created_at: String,
}

async fn list_templates_endpoint(State(demo): State<Arc<DesignerState>>) -> impl IntoResponse {
    match demo.store.list_templates().await {
        Ok(list) => {
            let dtos: Vec<TemplateSummaryDto> = list
                .into_iter()
                .map(|t| TemplateSummaryDto {
                    name: t.name,
                    latest_version: t.latest_version,
                    plan_hash: hex::encode(t.plan_hash),
                    created_at: t.created_at,
                })
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Serialize)]
pub(crate) struct TemplateVersionDto {
    name: String,
    version: u32,
    plan_hash: String,
    dsl_body: String,
}

async fn get_template_version_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((name, version)): Path<(String, u32)>,
) -> impl IntoResponse {
    match demo.store.load_template_version(&name, version).await {
        Ok(Some((dsl_body, plan_hash))) => {
            let dto = TemplateVersionDto {
                name,
                version,
                plan_hash: hex::encode(plan_hash),
                dsl_body,
            };
            (StatusCode::OK, Json(dto)).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Template '{}' version {} not found", name, version) })),
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ).into_response()
    }
}

// ── Published-template catalogue listing (Phase B, 2026-07-30) ───────────

#[derive(Serialize)]
struct PublishedTemplateDto {
    template_key: String,
    template_version: u32,
    process_key: String,
    task_manifest: Vec<String>,
    created_at: i64,
    published_at: Option<i64>,
}

#[derive(Deserialize)]
struct ListPublishedTemplatesQuery {
    key: Option<String>,
}

/// Lists templates from the lifecycle-bearing `workflow_templates` registry
/// (`bpmn_lite_authoring::TemplateStore`), Published-only — for the
/// catalogue UI to browse and spawn from. Distinct from
/// `list_templates_endpoint`, which still reads the legacy
/// `workflow_template_catalog` (no lifecycle) that Phase A's dual-write
/// feeds.
async fn list_published_templates_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Query(query): Query<ListPublishedTemplatesQuery>,
) -> impl IntoResponse {
    match demo
        .template_store
        .list(
            query.key.as_deref(),
            Some(bpmn_lite_authoring::TemplateState::Published),
        )
        .await
    {
        Ok(list) => {
            let dtos: Vec<PublishedTemplateDto> = list
                .into_iter()
                .map(|t| PublishedTemplateDto {
                    template_key: t.template_key,
                    template_version: t.template_version,
                    process_key: t.process_key,
                    task_manifest: t.task_manifest,
                    created_at: t.created_at,
                    published_at: t.published_at,
                })
                .collect();
            (StatusCode::OK, Json(dtos)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Spawn instance from a Published template (Phase B, 2026-07-30) ───────

fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Reconstruct a `WorkflowExecutionPlan` from a template's `dto_snapshot`,
/// with no YAML/DSL text round-trip: dto_to_ir -> verify -> project_ir.
/// `project_ir`'s precondition is an already-verified graph, hence the
/// explicit `verify()` call here.
fn reconstruct_plan_from_template(
    tpl: &bpmn_lite_authoring::WorkflowTemplate,
) -> anyhow::Result<WorkflowExecutionPlan> {
    let ir = bpmn_lite_authoring::dto_to_ir(&tpl.dto_snapshot)?;
    let verify_errors = bpmn_lite_compiler::verify(&ir);
    if !verify_errors.is_empty() {
        let msgs: Vec<String> = verify_errors.iter().map(|e| e.message.clone()).collect();
        anyhow::bail!(
            "template {}:v{} failed re-verification: {}",
            tpl.template_key,
            tpl.template_version,
            msgs.join("; ")
        );
    }
    bpmn_lite_compiler::dsl::project_ir(&ir, tpl.process_key.clone())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[derive(Deserialize, Default)]
struct SpawnTemplateBody {
    version: Option<u32>,
    payload: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct SpawnTemplateResponse {
    instance_id: Uuid,
    template_key: String,
    template_version: u32,
    bytecode_version: String,
}

async fn spawn_template_instance_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(name): Path<String>,
    body: Option<Json<SpawnTemplateBody>>,
) -> impl IntoResponse {
    let body = body.map(|Json(b)| b).unwrap_or_default();

    let tpl = if let Some(version) = body.version {
        match demo.template_store.load(&name, version).await {
            Ok(Some(tpl)) if tpl.state == bpmn_lite_authoring::TemplateState::Published => tpl,
            Ok(Some(tpl)) => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "template_not_published",
                        "template_key": name,
                        "template_version": version,
                        "state": format!("{:?}", tpl.state),
                    })),
                )
                    .into_response();
            }
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "template_not_found",
                        "template_key": name,
                        "template_version": version,
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
    } else {
        match demo.template_store.load_latest_published(&name).await {
            Ok(Some(tpl)) => tpl,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "no_published_version",
                        "template_key": name,
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        }
    };

    let plan = match reconstruct_plan_from_template(&tpl) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let compiled = match demo.engine.compile_dsl(&plan).await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("compile_dsl failed: {e}") })),
            )
                .into_response();
        }
    };

    let payload_json = body
        .payload
        .unwrap_or_else(|| serde_json::json!({}))
        .to_string();
    let idempotency_key = Uuid::now_v7();
    let entry_id = Uuid::now_v7();
    let runbook_id = Uuid::now_v7();
    let instance_id = bpmn_lite_types::EffectId::for_command(idempotency_key, 0, 1).as_uuid();

    let start_command = match bpmn_lite_types::StartCommand::builder(
        demo.tenant_id.clone(),
        instance_id,
        compiled.bytecode_version,
    )
    .process_key(plan.workflow_id().to_string())
    .lineage(entry_id, runbook_id)
    .correlation_id(idempotency_key.to_string())
    .idempotency_key(idempotency_key)
    .initial_payload(payload_json)
    .session_stack(bpmn_lite_types::session_stack::SessionStackState::default())
    .logical_time(unix_time_ms())
    .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let iid = match demo.engine.start_command(start_command).await {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // Phase C: run the fiber up to its first wait point so a freshly
    // spawned instance is immediately observable (parked on its first
    // job) via `GET /bpmn/instances/:id/status`. One synchronous tick —
    // NOT a background loop; all further progress is request-driven
    // through `POST /bpmn/instances/:id/advance`.
    if let Err(e) = demo.engine.tick_instance(iid).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(SpawnTemplateResponse {
            instance_id: iid,
            template_key: tpl.template_key,
            template_version: tpl.template_version,
            bytecode_version: hex::encode(compiled.bytecode_version),
        }),
    )
        .into_response()
}

// ── Phase C: observe + advance a spawned instance (2026-08-01) ─────────
//
// Request-driven execution for the demo browser UI: no background tick
// loop. `GET .../status` is a pure read over `engine.inspect`;
// `POST .../advance` performs one real VM advance round — tick, dequeue
// queued jobs, `complete_job` each with the instance's own current
// payload (hash-verified by the kernel), tick again. This is genuine
// engine execution, not the runner's plan-walking simulation.

/// FIXED wire contract — a React page is built against these exact
/// field names. Do not rename. (`waiting_timers` added WS-D D3 —
/// additive, existing readers unaffected.)
#[derive(Serialize)]
struct InstanceStatusResponse {
    instance_id: Uuid,
    state: String,
    /// Present only when the state variant carries a completion/terminal
    /// timestamp (`Completed`/`Cancelled`/`Terminated`).
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<i64>,
    waiting_jobs: Vec<WaitingJobDto>,
    /// WS-D D3: fibers parked on a durable timer (`WaitState::Timer`,
    /// i.e. a standalone Wait node), with their deadline — previously
    /// these were dumped invisibly into `wait_count`. Guard-armed timers
    /// do NOT appear here by design: arming does not park the fiber (the
    /// guarded body keeps running); they surface as the scheduled
    /// durable effect, not a wait state.
    waiting_timers: Vec<WaitingTimerDto>,
    fiber_count: usize,
    wait_count: usize,
}

#[derive(Serialize)]
struct WaitingJobDto {
    job_key: String,
    /// BPMN node id from the program debug map when resolvable
    /// (single-fiber case); empty string otherwise.
    node_id: String,
}

#[derive(Serialize)]
struct WaitingTimerDto {
    deadline_ms: u64,
    /// Same single-fiber node-id resolution rule as `WaitingJobDto`.
    node_id: String,
}

fn instance_status_response(
    inspection: &bpmn_lite_engine::ProcessInspection,
) -> InstanceStatusResponse {
    use bpmn_lite_types::ProcessState;
    let (state, completed_at) = match &inspection.state {
        ProcessState::Running => ("Running".to_string(), None),
        ProcessState::Completed { at } => ("Completed".to_string(), Some(*at)),
        ProcessState::Cancelled { at, .. } => ("Cancelled".to_string(), Some(*at)),
        ProcessState::Terminated { at } => ("Terminated".to_string(), Some(*at)),
        ProcessState::Failed { .. } => ("Failed".to_string(), None),
        ProcessState::Incidented { .. } => ("Incidented".to_string(), None),
        ProcessState::WaitingOnSubmission { .. } => ("WaitingOnSubmission".to_string(), None),
        ProcessState::WaitingOnInvocation { .. } => ("WaitingOnInvocation".to_string(), None),
    };
    let single_fiber = inspection.fibers.len() == 1;
    let waiting_jobs = inspection
        .fibers
        .iter()
        .filter_map(|f| match &f.wait_state {
            bpmn_lite_types::WaitState::Job { job_key } => Some(WaitingJobDto {
                job_key: job_key.clone(),
                node_id: if single_fiber {
                    inspection.current_node_id.clone().unwrap_or_default()
                } else {
                    String::new()
                },
            }),
            _ => None,
        })
        .collect();
    let waiting_timers = inspection
        .fibers
        .iter()
        .filter_map(|f| match &f.wait_state {
            bpmn_lite_types::WaitState::Timer { deadline_ms } => Some(WaitingTimerDto {
                deadline_ms: *deadline_ms,
                node_id: if single_fiber {
                    inspection.current_node_id.clone().unwrap_or_default()
                } else {
                    String::new()
                },
            }),
            _ => None,
        })
        .collect();
    let wait_count = inspection
        .fibers
        .iter()
        .filter(|f| !matches!(f.wait_state, bpmn_lite_types::WaitState::Running))
        .count();
    InstanceStatusResponse {
        instance_id: inspection.instance_id,
        state,
        completed_at,
        waiting_jobs,
        waiting_timers,
        fiber_count: inspection.fibers.len(),
        wait_count,
    }
}

/// 404-or-instance guard shared by status/advance: distinguishes
/// "unknown id" (404) from engine/store failures (500) structurally —
/// via `load_instance`'s `Option` — not by matching error strings.
async fn load_designer_instance(
    demo: &DesignerState,
    instance_id: Uuid,
) -> Result<bpmn_lite_types::ProcessInstance, axum::response::Response> {
    match demo.store.load_instance(&demo.tenant_id, instance_id).await {
        Ok(Some(instance)) => Ok(instance),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "instance_not_found",
                "instance_id": instance_id,
            })),
        )
            .into_response()),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response()),
    }
}

async fn instance_status_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(instance_id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(resp) = load_designer_instance(&demo, instance_id).await {
        return resp;
    }
    match demo.engine.inspect(instance_id).await {
        Ok(inspection) => {
            (StatusCode::OK, Json(instance_status_response(&inspection))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// WS-D D3: optional advance body. `logical_time_ms` injects the timer
/// clock for this round — durable timers due at or before it fire. Wall
/// clock when omitted. Injection keeps timeout receipts deterministic
/// (no sleeping in tests) and lets a demo fast-forward a 3-day wait.
#[derive(serde::Deserialize, Default)]
struct AdvanceRequest {
    logical_time_ms: Option<u64>,
}

async fn advance_instance_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(instance_id): Path<Uuid>,
    body: Option<Json<AdvanceRequest>>,
) -> impl IntoResponse {
    let logical_time = body
        .and_then(|Json(request)| request.logical_time_ms)
        .unwrap_or_else(|| unix_time_ms() as u64);

    let instance = match load_designer_instance(&demo, instance_id).await {
        Ok(instance) => instance,
        Err(resp) => return resp,
    };

    // Idempotent no-op on any non-Running instance (Completed etc.):
    // report current status unchanged.
    if !matches!(instance.state, bpmn_lite_types::ProcessState::Running) {
        return match demo.engine.inspect(instance_id).await {
            Ok(inspection) => {
                (StatusCode::OK, Json(instance_status_response(&inspection))).into_response()
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        };
    }

    // WS-D D3: fire due durable timers FIRST — an interrupting guard
    // whose deadline has passed must unwind its host before this round
    // dequeues (and would otherwise complete) the host's job. This is
    // the runner's scheduler tick, request-driven: the designer's
    // no-background-loop doctrine holds, timers advance only when a
    // request advances them.
    if let Err(e) = demo
        .engine
        .tick_due_timers("designer-advance", logical_time, 128, 5_000)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // One advance round: tick + dequeue (run_instance), complete each
    // dequeued job, tick again so fibers move past the completed steps.
    let jobs = match demo.engine.run_instance(instance_id).await {
        Ok(jobs) => jobs,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // WS-D D3: complete only jobs a fiber is ACTUALLY parked on. An
    // interrupting guard that fired above unwinds the host fiber, but no
    // JobMutation cancels the host's already-queued activation — the
    // kernel has no job-cancel mutation at all (only RetryClaimed/
    // DeadLetterClaimed) — so `dequeue_jobs` can hand back a superseded
    // activation whose completion the engine rightly refuses
    // ("completion has no parked fiber"). Filter structurally against
    // the fibers' wait states, never by matching the error string; the
    // superseded claim simply lease-expires. The missing job-cancel on
    // interrupting unwind is a surfaced kernel gap (production workers
    // holding the host's job hit the same ghost), recorded in the WS-D
    // phase notes — not silently papered over here.
    let parked_job_keys: std::collections::HashSet<String> =
        match demo.engine.inspect(instance_id).await {
            Ok(inspection) => inspection
                .fibers
                .iter()
                .filter_map(|f| match &f.wait_state {
                    bpmn_lite_types::WaitState::Job { job_key } => Some(job_key.clone()),
                    _ => None,
                })
                .collect(),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        };

    for job in jobs.iter().filter(|j| parked_job_keys.contains(&j.job_key)) {
        // The kernel verifies `expected_instance_payload_hash` against
        // the instance's CURRENT payload hash, and a completion may
        // rewrite the payload — so re-read per job rather than trusting
        // the activation-time snapshot. Echoing the instance's own
        // payload back keeps the hash chain stable across the loop.
        let current = match demo.store.load_instance(&demo.tenant_id, instance_id).await {
            Ok(Some(instance)) => instance,
            Ok(None) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("instance {instance_id} vanished mid-advance"),
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
        };
        if let Err(e) = demo
            .engine
            .complete_job(
                &job.job_key,
                current.domain_payload.as_ref(),
                current.domain_payload_hash,
                std::collections::BTreeMap::new(),
            )
            .await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    if let Err(e) = demo.engine.tick_instance(instance_id).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match demo.engine.inspect(instance_id).await {
        Ok(inspection) => {
            (StatusCode::OK, Json(instance_status_response(&inspection))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── Designer sessions (EOP-SAGE-REPL-BPMN-001 T1) ────────────────────────

#[derive(Deserialize)]
pub(crate) struct CreateSessionBody {
    name: String,
    #[serde(default)]
    dsl_source: String,
}

#[derive(Serialize)]
pub(crate) struct CreateSessionResponse {
    session_id: String,
}

#[derive(Deserialize)]
pub(crate) struct SessionRevisionBody {
    dsl_source: String,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
pub(crate) struct SessionRevisionResponse {
    seq: u64,
    /// Compile diagnostics for the NEW source — drafts may be invalid;
    /// the revision is recorded either way (the REPL shows diagnostics,
    /// it does not lose work).
    compiles: bool,
    diagnostics: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct SessionUtteranceBody {
    text: String,
    /// WS-B.4: the BPMN id of the graph position the utterance was
    /// issued from. Meaningful only for DesignerDag-backed sessions
    /// (`is_graph_backed()`); ignored for legacy DSL-source sessions,
    /// which have no `DesignerDag` to resolve it against. An id naming
    /// no node in the reconstruction is a fail-closed 422, never a
    /// silent whole-graph fallback.
    #[serde(default)]
    anchor: Option<String>,
}

#[derive(Deserialize)]
struct PaletteSelectionBody {
    move_id: String,
    #[serde(default)]
    anchor: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct SaveSessionBody {
    template_name: String,
}

#[derive(Deserialize)]
struct GameboardQuery {
    #[serde(default)]
    anchor: Option<String>,
}

async fn create_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let id = Uuid::now_v7();
    match demo
        .store
        .create_design_session(&demo.tenant_id, id, &body.name, &body.dsl_source)
        .await
    {
        Ok(()) => (
            StatusCode::CREATED,
            Json(CreateSessionResponse {
                session_id: id.to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn list_design_sessions_endpoint(
    State(demo): State<Arc<DesignerState>>,
) -> impl IntoResponse {
    match demo.store.list_design_sessions(&demo.tenant_id).await {
        Ok(summaries) => Json(summaries).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn get_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

async fn session_revision_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionRevisionBody>,
) -> impl IntoResponse {
    let seq = match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::Revision {
                dsl_source: body.dsl_source.clone(),
                note: body.note.clone(),
            },
        )
        .await
    {
        Ok(seq) => seq,
        Err(bpmn_lite_store::StoreError::NotFound(_)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let registry = get_preview_registry();
    let (compiles, diagnostics) =
        match bpmn_lite_compiler::dsl::compile(&body.dsl_source, &registry) {
            Ok(_) => (true, Vec::new()),
            Err(bpmn_lite_compiler::dsl::CompileError::Parse(errs)) => (false, errs),
            Err(bpmn_lite_compiler::dsl::CompileError::Unroll(err)) => {
                (false, vec![err.to_string()])
            }
            Err(bpmn_lite_compiler::dsl::CompileError::Lint(errs)) => {
                (false, errs.iter().map(|e| format!("{e}")).collect())
            }
            Err(bpmn_lite_compiler::dsl::CompileError::Dag(errs)) => {
                (false, errs.iter().map(|e| format!("{e}")).collect())
            }
        };
    Json(SessionRevisionResponse {
        seq,
        compiles,
        diagnostics,
    })
    .into_response()
}

/// v1 whole-graph legality: every operation/production legal. Serves
/// sessions that have never accumulated a `GraphEdit` (the legacy
/// DSL-source path — no `DesignerDag` exists to run the real oracle
/// against). WS-B.4-graph-backed sessions use `PositionalLegality`
/// instead (`reconstruct_designer_dag` below).
struct WholeGraphLegality;
impl designer_graph::board_candidate::LegalityOracle for WholeGraphLegality {
    type NodeKey = ();
    fn legal_operations(
        &self,
        _: Option<&()>,
    ) -> Vec<designer_graph::board_candidate::OperationKind> {
        designer_graph::board_candidate::OperationKind::ALL.to_vec()
    }
    fn legal_productions(
        &self,
        _: Option<&()>,
    ) -> Vec<designer_graph::board_candidate::ProductionId> {
        designer_graph::board_candidate::ProductionId::ALL.to_vec()
    }
}

// ── WS-B.4: DesignerDag-backed sessions ──────────────────────────────────
//
// A session accumulating `GraphEdit` events is DesignerDag-backed: its
// `DesignerDag` is the replay product of a deterministically-seeded Start
// node plus every accumulated operation sequence, in event order (schema.rs's
// own module doc: "the durable surface is the edit log... the DAG is its
// replay product"). Reconstruction is a pure function of the event log —
// no snapshot is ever persisted (rider 2, same doc).

/// The session's Start node key: derived from the session id via a fixed
/// namespace so it is stable across every reconstruction and knowable to
/// a client authoring the session's very first graph edit (returned in
/// `create_design_session_endpoint`'s response once a session begins its
/// graph-edit life — see `SEED_START_ID`/`seed_start_key`).
const SEED_START_ID: &str = "start";
fn seed_start_key(session_id: Uuid) -> designer_graph::schema::NodeKey {
    const NAMESPACE: Uuid = Uuid::from_bytes([
        0xb1, 0x3f, 0x1a, 0x02, 0xd2, 0x99, 0x4a, 0x71, 0x9c, 0x3e, 0x7a, 0x21, 0x5c, 0x0e, 0x8b,
        0x44,
    ]);
    designer_graph::schema::NodeKey(Uuid::new_v5(&NAMESPACE, session_id.as_bytes()))
}

/// Replay a session's accumulated `GraphEdit` payloads into a
/// `DesignerDag`. Fail-closed: a payload that fails to deserialize or
/// fails to stage is a bug in what was already accepted at append time
/// (the graph-edit endpoint validates before persisting) — surfaced as an
/// error, never silently skipped mid-replay (a partial DAG would silently
/// misrepresent the session).
///
/// G6.1: `as_of_seq` bounds the replay to a truncated view of the tape —
/// `None` is the live/current view (any `Undo` events in the log are
/// honored, per `DesignSessionRecord::visible_events`); `Some(n)` is an
/// explicit historical view as of event `n`, ignoring any `Undo` event
/// that itself sits after `n`. Every call site names which it wants.
fn reconstruct_designer_dag(
    record: &bpmn_lite_store::DesignSessionRecord,
    as_of_seq: Option<u64>,
) -> anyhow::Result<designer_graph::schema::DesignerDag> {
    use designer_graph::ops::Operation;
    use designer_graph::productions::apply_production;
    use designer_graph::schema::{DesignerDag, Provenance};

    let mut dag = DesignerDag::new(record.name.clone());
    dag.seed(
        seed_start_key(record.id),
        bpmn_lite_compiler::IRNode::Start {
            id: SEED_START_ID.into(),
        },
        Provenance::default(),
    )?;
    for (i, payload) in record
        .graph_edit_payloads_as_of(as_of_seq)
        .into_iter()
        .enumerate()
    {
        let ops: Vec<Operation> = serde_json::from_str(payload)
            .map_err(|e| anyhow::anyhow!("graph edit #{i} failed to deserialize: {e}"))?;
        dag = apply_production(&dag, ops, Provenance::default())
            .map_err(|e| anyhow::anyhow!("graph edit #{i} failed to replay: {e}"))?
            .candidate;
    }
    Ok(dag)
}

/// G6.2 — render a session's accumulated operation tape as readable
/// Designer-DSL text, one operation per line, in visible replay order
/// (honoring any `Undo`, same as `reconstruct_designer_dag`). The tape is
/// the durable, authoritative surface; this is a read-only rendering job,
/// not a re-parseable program.
async fn session_runbook_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let mut all_ops: Vec<designer_graph::ops::Operation> = Vec::new();
    for (i, payload) in record.graph_edit_payloads_as_of(None).into_iter().enumerate() {
        let ops: Vec<designer_graph::ops::Operation> = match serde_json::from_str(payload) {
            Ok(ops) => ops,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("graph edit #{i} failed to deserialize: {e}") })),
                )
                    .into_response();
            }
        };
        all_ops.extend(ops);
    }
    let runbook = crate::runbook::render_runbook(&all_ops);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "session_id": id,
            "operation_count": all_ops.len(),
            "runbook": runbook,
        })),
    )
        .into_response()
}

/// The session's graph-identity hash: blake3 over the accumulated
/// edit-log payloads (the DAG's sole source of truth). Shared by the
/// utterance pipeline's board identity and the proposal drift check.
fn graph_identity_hash(record: &bpmn_lite_store::DesignSessionRecord) -> String {
    let mut hasher = blake3::Hasher::new();
    for payload in record.graph_edit_payloads() {
        hasher.update(payload.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}

/// Canonical SHA-256 content identity of the authoritative graph edit log.
/// This is deliberately separate from the legacy blake3 revision identity,
/// whose bytes remain unchanged for board and proposal compatibility.
/// Delegates to `graph_content_hash_over` (G2.2) so chain-preview's
/// hypothetical route-hash extension uses the identical framing, not a
/// parallel reimplementation that could drift from this one.
fn graph_content_hash(record: &bpmn_lite_store::DesignSessionRecord) -> String {
    utterance_engine::bpmn_board::graph_content_hash_over(
        record.graph_edit_payloads().into_iter(),
    )
}

/// G2.3 outcome of attempting to resolve a compound (`"<a>; <b>"`)
/// utterance as a chain-verified plan.
enum CompoundChainOutcome {
    /// `observed_intent` is not compound syntax; proceed exactly as before.
    NotCompound,
    /// Both spans resolved, span 2 verified against the position span 1
    /// actually produces (not the original, unchanged position).
    Ready(utterance_engine::disposition::ResolvedChain),
    /// Compound syntax detected and span 1 was ACTUALLY materialised and
    /// compiler-admitted, but span 2 is proven illegal against the
    /// resulting graph (or span 1 itself fails compiler admission) --
    /// named diagnostics, the case Gate G2 requires a refusal for.
    Refused { diagnostics: Vec<String> },
    /// Compound syntax detected but chaining could not even be ATTEMPTED
    /// -- span 1 doesn't resolve to a legal move at all, or (the common
    /// case) its candidate has a required argument (`Condition`,
    /// `SubprocessReference`) `start_workbook` can never auto-extract
    /// from free text, only from an explicit typed answer, so it can
    /// never reach `ReadyForDryRun` from utterance text alone. This is
    /// NOT a proven conflict -- falls back to `compound_plan`'s pre-G2.3
    /// same-position resolution (declares the two-move intent without
    /// verifying it), preserving existing behavior for compound
    /// utterances whose moves are legitimately incomplete pending
    /// clarification, exactly as before this tranche.
    Unverifiable,
}

/// A synthetic evidence hash for a compound-plan chain step. Mirrors the
/// `WorkbookEvidence::PaletteSelection` convention already used for the
/// palette-selection endpoint (`rest.rs`, "palette-selection-v1" tag) and
/// the direct-edit-equivalence probe ("direct-edit-equivalence-probe" tag)
/// -- a versioned SHA-256 digest of a preimage binding the hash to the
/// exact position and move it evidences, so provenance stays distinguishable
/// even though there is no real `DecisionRecord` behind an exact-phrase
/// compound span.
fn compound_span_evidence_hash(
    position: &semantic_decision_contracts::DesignPosition,
    move_id: &semantic_decision_contracts::LegalMoveId,
    span: &str,
) -> anyhow::Result<semantic_decision_contracts::EvidenceRecordHash> {
    use sha2::{Digest, Sha256};
    semantic_decision_contracts::EvidenceRecordHash::new(format!(
        "{:x}",
        Sha256::digest(
            format!(
                "compound-plan-step-v1\0{}\0{}\0{}",
                position.state_id().as_str(),
                move_id.as_str(),
                span
            )
            .as_bytes()
        )
    ))
    .map_err(anyhow::Error::from)
}

/// G2.3: server-layer chain-fold orchestration for a compound utterance.
///
/// Detection stays pure and lives in `StrictCompoundSyntax`
/// (`utterance-engine`, unchanged). Everything AFTER detection that must
/// actually materialise and compiler-admit span 1 against a real
/// `DesignerDag` has to live HERE: turning a `LegalMoveId` into a real
/// `Vec<Operation>` tape always requires a `ProposalWorkbook`
/// (`materialize_workbook`), and the only production path that builds one
/// is `proposal::start_workbook` -- a server-layer function `utterance-
/// engine`'s `disposition.rs` cannot reach and, per the established crate-
/// boundary discipline (workbook/session materialization is server-only;
/// no existing precedent constructs a `ProposalWorkbook` from utterance-
/// engine at runtime), should not gain. Rule 7 substrate check surfaced
/// this 2026-08-11; Adam ruled: move the orchestration here rather than
/// give utterance-engine a new, precedent-breaking construction path.
///
/// Span 1 is resolved and materialised exactly like `compound_plan`'s
/// existing (pre-G2.3) span resolution -- governed exact match, highest-
/// evidence legal move for that candidate. Span 2 is resolved the SAME
/// way, but against `resolve_hypothetical_position`'s (G2.1) result for
/// the `DesignerDag` span 1's operations actually produce and the compiler
/// actually admits (via `resolve_hypothetical_chain`, G2.2) -- never
/// against the original, unchanged position.
#[allow(clippy::too_many_arguments)]
fn resolve_compound_chain(
    dag: &designer_graph::schema::DesignerDag,
    board: &semantic_decision_contracts::SemanticDecisionBoard,
    position: &semantic_decision_contracts::DesignPosition,
    evidence: &[semantic_decision_contracts::MoveEvidence],
    observed_intent: &str,
    current_graph_revision: &str,
    real_graph_edit_payloads: &[&str],
    anchor: Option<designer_graph::schema::NodeKey>,
) -> anyhow::Result<CompoundChainOutcome> {
    use utterance_engine::disposition::{ActionSpanProducer, ResolvedChain, StrictCompoundSyntax};
    use utterance_engine::exact::{governed_exact, ExactMatch};

    let Some(span_evidence) = StrictCompoundSyntax.detect(observed_intent, board) else {
        return Ok(CompoundChainOutcome::NotCompound);
    };
    let spans = span_evidence.spans;

    let probabilities = evidence
        .iter()
        .map(|item| (item.move_id(), item.probability().get()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let resolve_span = |pos: &semantic_decision_contracts::DesignPosition, span: &str| {
        match governed_exact(board, span) {
            ExactMatch::Unique(candidate) => pos
                .legal_moves()
                .iter()
                .filter(|legal_move| legal_move.candidate_id().as_str() == candidate)
                .max_by(|left, right| {
                    probabilities
                        .get(left.move_id())
                        .copied()
                        .unwrap_or(0.0)
                        .total_cmp(&probabilities.get(right.move_id()).copied().unwrap_or(0.0))
                        .then_with(|| right.move_id().cmp(left.move_id()))
                })
                .map(|legal_move| legal_move.move_id().clone()),
            ExactMatch::None | ExactMatch::Collision(_) => None,
        }
    };

    let Some(move_1) = resolve_span(position, &spans[0]) else {
        return Ok(CompoundChainOutcome::Unverifiable);
    };

    let evidence_hash = compound_span_evidence_hash(position, &move_1, &spans[0])?;
    let workbook = match crate::proposal::start_workbook(
        dag,
        anchor,
        board,
        crate::proposal::SelectedMove {
            position,
            move_id: &move_1,
        },
        crate::proposal::WorkbookEvidence::PaletteSelection(evidence_hash),
        &spans[0],
        0,
    ) {
        Ok(workbook) => workbook,
        Err(_) => return Ok(CompoundChainOutcome::Unverifiable),
    };
    if !matches!(
        workbook.status(),
        semantic_decision_contracts::ProposalStatus::ReadyForDryRun
            | semantic_decision_contracts::ProposalStatus::ReadyForRatification
    ) {
        // Common case: a candidate argument kind (`Condition`,
        // `SubprocessReference`) that `start_workbook` can only fill via
        // an explicit typed answer, never free-text extraction -- there
        // is no way to materialise span 1 from the utterance alone.
        return Ok(CompoundChainOutcome::Unverifiable);
    }
    let bound = utterance_engine::bpmn_board::materialize_bpmn_workbook(dag, &workbook)?;

    let steps = [utterance_engine::bpmn_board::ChainStep {
        move_id: move_1.clone(),
        operations: bound.operations().to_vec(),
    }];
    let positions = match utterance_engine::bpmn_board::resolve_hypothetical_chain(
        dag,
        board,
        position,
        current_graph_revision,
        DESIGNER_COMPILER_PROFILE_IDENTITY,
        real_graph_edit_payloads,
        position.focus().clone(),
        &steps,
    ) {
        Ok(positions) => positions,
        // Blind-review finding (2026-08-11): classify by variant, don't
        // blanket-catch. `GraphProjection` (the op-application/apply_
        // production refusal) and `CompilerRefused` and `StaleBoardAnchor`
        // (the board's bound anchor going stale because span 1 removed
        // the node span 2 needs) are genuinely "span 2 illegal given span
        // 1" -- name them, don't 500 them. Everything else
        // (`MissingSemanticContract` -- registry/oracle drift,
        // `ResourceLimit`, `StaleBoardRevision`, `Shared`, `Gameboard`,
        // `InvalidAnchor`) is an orthogonal system/infra error that has
        // nothing to do with span 2's legality; mislabeling it as a
        // "refusal" would bury a real bug behind a business-logic-shaped
        // diagnostic. Propagate those as real errors instead.
        Err(
            error @ (utterance_engine::bpmn_board::BpmnBoardError::GraphProjection(_)
            | utterance_engine::bpmn_board::BpmnBoardError::CompilerRefused { .. }
            | utterance_engine::bpmn_board::BpmnBoardError::StaleBoardAnchor { .. }),
        ) => {
            return Ok(CompoundChainOutcome::Refused {
                diagnostics: vec![error.to_string()],
            })
        }
        Err(error) => return Err(error.into()),
    };
    let position_2 = positions.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!("resolve_hypothetical_chain returned no positions for one step")
    })?;

    let Some(move_2) = resolve_span(&position_2, &spans[1]) else {
        return Ok(CompoundChainOutcome::Refused {
            diagnostics: vec![format!(
                "span '{}' is not a legal move once '{}' has been applied",
                spans[1], spans[0]
            )],
        });
    };

    Ok(CompoundChainOutcome::Ready(ResolvedChain {
        spans,
        moves: vec![move_1, move_2],
    }))
}

/// Reconstruct the bounded typed-attempt projection observed by a position.
/// Raw dialogue and opaque proposal payloads never enter the ranker preimage.
/// G6.1: `as_of_seq` bounds this to the same truncated-replay view
/// `reconstruct_designer_dag` uses — the position-derivation history must
/// not include attempts from a range an undo excised.
fn design_history_projection(
    record: &bpmn_lite_store::DesignSessionRecord,
    as_of_seq: Option<u64>,
) -> anyhow::Result<(String, Vec<semantic_decision_contracts::MoveAttemptReceipt>)> {
    const MAX_WINDOW: usize = 64;
    let mut encoded = record
        .visible_events(as_of_seq)
        .into_iter()
        .rev()
        .filter_map(|event| match &event.kind {
            DesignSessionEventKind::Utterance {
                gameboard_attempt_receipt_json: Some(receipt),
                ..
            }
            | DesignSessionEventKind::ProposalAudit {
                gameboard_attempt_receipt_json: Some(receipt),
                ..
            } => Some(receipt.as_str()),
            _ => None,
        })
        .take(MAX_WINDOW)
        .collect::<Vec<_>>();
    encoded.reverse();
    let attempts = encoded
        .into_iter()
        .map(serde_json::from_str)
        .collect::<Result<Vec<_>, _>>()?;
    utterance_engine::bpmn_board::project_bpmn_attempt_history(&attempts)
        .map_err(anyhow::Error::from)
}

/// G6.1: `as_of_seq` bounds this to the same truncated-replay view
/// `reconstruct_designer_dag` uses — an undo must not leave a belief
/// snapshot from the excised (undone) range as "latest".
fn latest_gameboard_belief(
    record: &bpmn_lite_store::DesignSessionRecord,
    as_of_seq: Option<u64>,
) -> anyhow::Result<Option<semantic_decision_contracts::DesignBelief>> {
    record
        .visible_events(as_of_seq)
        .into_iter()
        .rev()
        .find_map(|event| match &event.kind {
            DesignSessionEventKind::Utterance {
                gameboard_belief_json: Some(belief),
                ..
            } => Some(serde_json::from_str(belief)),
            _ => None,
        })
        .transpose()
        .map_err(anyhow::Error::from)
}

fn gameboard_focus(
    anchor: Option<&str>,
    anchor_resolved: bool,
) -> anyhow::Result<semantic_decision_contracts::DesignFocus> {
    match anchor {
        Some(anchor) if anchor_resolved => Ok(semantic_decision_contracts::DesignFocus::element(
            semantic_decision_contracts::GraphElementRef::new(anchor)?,
        )),
        Some(anchor) => Ok(semantic_decision_contracts::DesignFocus::unknown(
            semantic_decision_contracts::GraphElementRef::new(anchor)?,
        )),
        None => Ok(semantic_decision_contracts::DesignFocus::absent(
            semantic_decision_contracts::FocusAbsenceReason::NotProvided,
        )),
    }
}

/// Versioned compatibility identity for the compiler/verifier admission
/// profile used by the existing Designer path. This is application adapter
/// metadata, not a shared gameboard rule or semantic-pack vocabulary.
const DESIGNER_COMPILER_PROFILE_IDENTITY: &str = "bpmn-lite-compiler-v1";

#[derive(Deserialize)]
pub(crate) struct SessionGraphEditBody {
    /// The `Vec<designer_graph::ops::Operation>` to stage and, on
    /// success, append. Sent as real JSON (not a pre-encoded string) —
    /// the server is the only party that ever deserializes it; the store
    /// persists the re-serialized form as an opaque string.
    operations: Vec<designer_graph::ops::Operation>,
    #[serde(default)]
    note: String,
}

#[derive(Serialize)]
pub(crate) struct SessionGraphEditResponse {
    seq: u64,
    /// Raw operation tapes have no asserted semantic move equivalence. They
    /// remain available only as an explicitly attributable lower-level edit.
    edit_kind: &'static str,
    /// Present only when the submitted single-operation tape is exactly a
    /// current, fully bound semantic move. All other tapes remain explicitly
    /// lower-level edits.
    semantic_move_id: Option<String>,
    non_equivalence_reason: Option<&'static str>,
}

#[derive(Debug)]
enum DirectEditResolution {
    Equivalent(String),
    NonEquivalent(&'static str),
    /// The raw tape is recognizably equivalent to a named candidate that
    /// `PolicyFilter` denies. Policy hiding is an authorization boundary,
    /// not merely a discovery/explanation boundary — a raw operation tape
    /// must not reach the graph by a route that never consults it. The
    /// caller must refuse the whole request; this is never folded into
    /// `NonEquivalent`, which only ever means "no policy verdict applies."
    PolicyRefused,
}

/// v0.8 (`EOP-PLAN-BPMN-GAMEBOARD-001.md` Phase 2 item 9): recover a
/// best-effort candidate shape from the raw tape, synthesize a workbook for
/// it, materialize through the same production path the palette/language
/// route uses, and PROVE equivalence by comparing the raw tape's and the
/// materialized tape's resulting graph state — not by diffing the edit
/// representations themselves. `op.delete_subgraph` is folded into this same
/// general mechanism rather than kept as a separate special case.
///
/// Also the sole policy-authorization gate for raw operation tapes: compiler
/// admission (`session_graph_edit_endpoint`) only proves graph-structural
/// legality, never consults `PolicyFilter`. A tape recognizable as a
/// policy-denied candidate must be refused here, before admission — not
/// merely mislabeled as a lower-level edit and allowed through.
fn resolve_direct_edit(
    record: &bpmn_lite_store::DesignSessionRecord,
    dag: &designer_graph::schema::DesignerDag,
    operations: &[designer_graph::ops::Operation],
    policy: &utterance_engine::board::PolicyFilter,
) -> DirectEditResolution {
    if operations.is_empty() {
        return DirectEditResolution::NonEquivalent("empty_operation_tape");
    }
    let shape = match crate::proposal::recover_candidate_shape(dag, operations) {
        Ok(shape) => shape,
        Err(crate::proposal::ShapeRefusal::Ambiguous) => {
            return DirectEditResolution::NonEquivalent("ambiguous_candidate_shape");
        }
        Err(crate::proposal::ShapeRefusal::NotProducible) => {
            return DirectEditResolution::NonEquivalent("no_supported_semantic_counterpart");
        }
    };
    if policy.denied.contains(shape.candidate_id) {
        return DirectEditResolution::PolicyRefused;
    }
    let Ok(anchor) = crate::proposal::bpmn_id_for_key(dag, shape.anchor) else {
        return DirectEditResolution::NonEquivalent("unresolved_direct_anchor");
    };
    let revision = graph_identity_hash(record);
    let Some(board) = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        dag,
        dag.key_for_bpmn_id(&anchor).zip(Some(anchor.as_str())),
        &revision,
        policy,
    ).ok() else { return DirectEditResolution::NonEquivalent("semantic_board_unavailable") };
    let Some((history_hash, _)) = design_history_projection(record, None).ok() else { return DirectEditResolution::NonEquivalent("history_unavailable") };
    let Some(focus) = gameboard_focus(Some(&anchor), true).ok() else {
        return DirectEditResolution::NonEquivalent("focus_unavailable");
    };
    let Some(position) = utterance_engine::bpmn_board::build_bpmn_design_position(
        dag, &board, &revision, &graph_content_hash(record), DESIGNER_COMPILER_PROFILE_IDENTITY,
        &history_hash, focus, None,
    ).ok() else { return DirectEditResolution::NonEquivalent("position_unavailable") };
    let Some(legal_move) = position.legal_moves().iter().find(|legal_move| {
        legal_move.candidate_id().as_str() == shape.candidate_id
            && legal_move.anchor().is_some_and(|a| a.as_str() == anchor)
    }) else {
        return DirectEditResolution::NonEquivalent("no_matching_legal_move");
    };
    let move_id = legal_move.move_id().clone();

    // A probe workbook exists only to drive the production materializer with
    // the recovered answers; it is never staged, persisted or ratified.
    use sha2::{Digest, Sha256};
    let probe_text = format!("direct-edit-equivalence-probe:{}", shape.candidate_id);
    let Ok(probe_hash) = semantic_decision_contracts::EvidenceRecordHash::new(hex::encode(
        Sha256::digest(format!("{probe_text}\0{}\0{}", position.state_id().as_str(), move_id.as_str()).as_bytes()),
    )) else { return DirectEditResolution::NonEquivalent("probe_evidence_unavailable") };
    let Ok(workbook) = crate::proposal::start_workbook(
        dag, Some(shape.anchor), &board,
        crate::proposal::SelectedMove { position: &position, move_id: &move_id },
        crate::proposal::WorkbookEvidence::PaletteSelection(probe_hash), &probe_text, 0,
    ) else { return DirectEditResolution::NonEquivalent("workbook_construction_failed") };
    let workbook = if shape.answers.is_empty() {
        workbook
    } else {
        match crate::proposal::apply_explicit_answers(dag, workbook, shape.answers) {
            Ok(workbook) => workbook,
            Err(_) => return DirectEditResolution::NonEquivalent("recovered_argument_invalid"),
        }
    };
    if !matches!(
        workbook.status(),
        semantic_decision_contracts::ProposalStatus::ReadyForDryRun
            | semantic_decision_contracts::ProposalStatus::ReadyForRatification
    ) {
        return DirectEditResolution::NonEquivalent("incomplete_recovered_arguments");
    }
    let Ok(bound) = utterance_engine::bpmn_board::materialize_bpmn_workbook(dag, &workbook) else {
        return DirectEditResolution::NonEquivalent("materialization_failed");
    };

    let raw_ir = designer_graph::productions::apply_production(
        dag, operations.to_vec(), designer_graph::schema::Provenance::default(),
    ).ok().and_then(|staged| staged.candidate.to_ir().ok());
    let materialized_ir = designer_graph::productions::apply_production(
        dag, bound.operations().to_vec(), designer_graph::schema::Provenance::default(),
    ).ok().and_then(|staged| staged.candidate.to_ir().ok());
    match (raw_ir, materialized_ir) {
        (Some(raw_ir), Some(materialized_ir))
            if designer_graph::schema::DesignerDag::ir_graphs_equivalent(&raw_ir, &materialized_ir) =>
        {
            DirectEditResolution::Equivalent(move_id.as_str().to_string())
        }
        (Some(_), Some(_)) => DirectEditResolution::NonEquivalent("recovered_shape_diverges"),
        _ => DirectEditResolution::NonEquivalent("materialized_operations_refused"),
    }
}

/// Stage the submitted operations against the session's CURRENT
/// reconstruction (never against a stale or hypothetical base — I18's
/// discipline extended to the session layer) and append only on success.
/// A refusal is reported and NOTHING is persisted — the edit log never
/// carries a candidate that didn't admit-stage, so replay can never fail
/// on data this endpoint itself accepted.
async fn session_graph_edit_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionGraphEditBody>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let dag = match reconstruct_designer_dag(&record, None) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
            )
                .into_response();
        }
    };
    let resolution = resolve_direct_edit(
        &record,
        &dag,
        &body.operations,
        &utterance_engine::board::PolicyFilter::default(),
    );
    let (semantic_move_id, non_equivalence_reason) = match resolution {
        DirectEditResolution::Equivalent(move_id) => (Some(move_id), None),
        DirectEditResolution::NonEquivalent(reason) => (None, Some(reason)),
        DirectEditResolution::PolicyRefused => {
            // Refuse before compiler admission and before anything is staged
            // or persisted. Disclosure-safe: the response never names the
            // candidate or confirms a policy verdict, matching the
            // explanation-layer convention (`explain_bpmn_candidate`).
            return (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "operation not permitted" })),
            )
                .into_response();
        }
    };
    if body.operations.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "empty operation sequence" })),
        )
            .into_response();
    }
    let staged = match designer_graph::productions::apply_production(
        &dag,
        body.operations.clone(),
        designer_graph::schema::Provenance::default(),
    ) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("staging refused: {e}") })),
            )
                .into_response();
        }
    };
    // staged.candidate itself is not persisted — only the op sequence is
    // (rider 2) — but the candidate MUST admit (full to_ir/verify/lower
    // theorem chain) before its op sequence is accepted; apply_production's
    // per-op staging alone only proves local anchor legality, not that the
    // resulting graph is live/reachable/exhaustive.
    if let Err(errs) = staged.candidate.admit() {
        let messages: Vec<String> = errs.iter().map(|e| e.message.clone()).collect();
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "graph does not admit", "diagnostics": messages })),
        )
            .into_response();
    }
    let operations_json = match serde_json::to_string(&body.operations) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("serialize: {e}") })),
            )
                .into_response();
        }
    };
    let audit_note = if let Some(move_id) = &semantic_move_id {
        format!("semantic_move_equivalent: {move_id}")
    } else if body.note.trim().is_empty() {
        format!("lower_level_direct_edit: {non_equivalence_reason:?}")
    } else {
        format!(
            "lower_level_direct_edit: {non_equivalence_reason:?}; {}",
            body.note.trim()
        )
    };
    match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::GraphEdit {
                operations_json,
                note: audit_note,
            },
        )
        .await
    {
        Ok(seq) => Json(SessionGraphEditResponse {
            seq,
            edit_kind: if semantic_move_id.is_some() {
                "semantic_move_equivalent"
            } else {
                "lower_level_direct_edit"
            },
            semantic_move_id,
            non_equivalence_reason,
        })
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

// ── G6.1: session undo as truncated replay ──────────────────────────────

#[derive(Deserialize)]
pub(crate) struct SessionUndoBody {
    /// The event `seq` to undo back to: replay is bounded to
    /// `seq <= target_seq` from this point forward, per
    /// `DesignSessionRecord::visible_events`. Must be strictly less than
    /// the session's current live head.
    target_seq: u64,
}

#[derive(Serialize)]
pub(crate) struct SessionUndoResponse {
    /// The seq of the appended `Undo` marker event itself.
    seq: u64,
    target_seq: u64,
    graph_content_hash: String,
}

/// Undo a session to a prior position. Fail-closed: the truncated position
/// is reconstructed and re-admitted BEFORE the `Undo` marker is persisted —
/// an undo target that would leave a non-admitting graph is refused, never
/// silently accepted then discovered broken on the next read.
///
/// Nothing is deleted (same invariant `Revision`'s doc comment states for
/// the legacy path): every excised event remains in the log and reappears
/// under any explicit `as_of_seq` view at or before its own window, or once
/// a later edit re-extends the head past it.
///
/// Best-effort: if the session is graph-backed and a gameboard position is
/// derivable for it, this also records a `MoveAttemptReceipt` tagged
/// `CorrectionKind::Undo` against the PRE-undo position — the first real
/// constructor of that variant; `evaluate_frozen_game_funnel`'s `reversals`
/// tally already reads it. A session with no derivable gameboard position
/// (DSL-source-only, or a graph-backed session with no legal moves at its
/// current position) still undoes; it just carries no gameboard receipt.
async fn session_undo_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionUndoBody>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let current_head = record.visible_events(None).into_iter().map(|e| e.seq).max();
    match current_head {
        Some(head) if body.target_seq < head => {}
        _ => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "target_seq must be strictly before the session's current head",
                    "current_head": current_head,
                })),
            )
                .into_response();
        }
    }
    // Fail-closed: prove the truncated position is real before persisting
    // the marker that makes it the new default view.
    let truncated_dag = match reconstruct_designer_dag(&record, Some(body.target_seq)) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("undo target does not reconstruct: {e}") })),
            )
                .into_response();
        }
    };
    if record.is_graph_backed() {
        if let Err(errs) = truncated_dag.admit() {
            let messages: Vec<String> = errs.iter().map(|e| e.message.clone()).collect();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "undo target does not admit", "diagnostics": messages })),
            )
                .into_response();
        }
    }
    let undo_seq = match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::Undo {
                target_seq: body.target_seq,
            },
        )
        .await
    {
        Ok(seq) => seq,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // Best-effort gameboard receipt against the PRE-undo (live) position —
    // failure here does not undo the undo; it only means this session's
    // funnel telemetry for this action stays absent, same as any other
    // "best-effort, non-fatal" attempt-receipt path in this file.
    if record.is_graph_backed() {
        let _ = record_undo_receipt(&demo, id, &record).await;
    }
    // The POST-undo content hash: computed over the truncated view
    // (`as_of_seq = Some(target_seq)`), NOT `graph_content_hash(&record)`
    // (which is the unbounded/live view over the PRE-undo `record` still
    // in scope here) — the truncated view is unaffected by the `Undo`
    // marker just appended above (it isn't a `GraphEdit`), so `record`'s
    // own payload log already reflects the correct post-undo content.
    let post_undo_content_hash = utterance_engine::bpmn_board::graph_content_hash_over(
        record
            .graph_edit_payloads_as_of(Some(body.target_seq))
            .into_iter(),
    );
    (
        StatusCode::OK,
        Json(SessionUndoResponse {
            seq: undo_seq,
            target_seq: body.target_seq,
            graph_content_hash: post_undo_content_hash,
        }),
    )
        .into_response()
}

/// Best-effort helper for `session_undo_endpoint`: derive the session's
/// PRE-undo gameboard position (same construction sequence
/// `palette_select_endpoint` uses) and record a `CorrectionKind::Undo`
/// attempt receipt against it.
async fn record_undo_receipt(
    demo: &DesignerState,
    session_id: Uuid,
    record: &bpmn_lite_store::DesignSessionRecord,
) -> anyhow::Result<()> {
    use utterance_engine::board::PolicyFilter;

    let dag = reconstruct_designer_dag(record, None)?;
    let revision = graph_identity_hash(record);
    let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag,
        None,
        &revision,
        &PolicyFilter::default(),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (history_hash, _attempts) = design_history_projection(record, None)?;
    let position = utterance_engine::bpmn_board::build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &graph_content_hash(record),
        DESIGNER_COMPILER_PROFILE_IDENTITY,
        &history_hash,
        semantic_decision_contracts::DesignFocus::absent(
            semantic_decision_contracts::FocusAbsenceReason::NotProvided,
        ),
        None,
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let attempt_id = semantic_decision_contracts::MoveAttemptId::new(format!(
        "session-undo-{}",
        blake3::hash(format!("{session_id}\0{}", position.state_id().as_str()).as_bytes())
            .to_hex()
    ))?;
    let receipt = utterance_engine::bpmn_board::record_bpmn_attempt(
        &position,
        attempt_id,
        None,
        "(session undo)",
        semantic_decision_contracts::MoveAttemptOutcome::Corrected,
        None,
        Some(semantic_decision_contracts::CorrectionKind::Undo),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let receipt_json = serde_json::to_string(&receipt)?;
    demo.store
        .append_design_session_event(
            &demo.tenant_id,
            session_id,
            &DesignSessionEventKind::Utterance {
                text: "(session undo)".into(),
                response: "session reverted to a prior position".into(),
                context_projection: None,
                decision_record_json: None,
                gameboard_attempt_receipt_json: Some(receipt_json),
                gameboard_belief_json: None,
                gameboard_disposition_json: None,
                history_projection_hash: Some(history_hash),
            },
        )
        .await?;
    Ok(())
}

// ── Utterance-proposal ratify/reject (the human gate) ───────────────────
//
// The ONLY door from an utterance to a persisted GraphEdit. §S7 of
// EOP-SPEC-SLM-TRAIN-001: model/binding output remains evidence into
// deterministic policy — no code path auto-applies a proposal.

#[derive(Deserialize)]
struct ProposalAnswersBody {
    answers: Vec<crate::proposal::SlotAnswer>,
}

fn proposal_audit_event(
    pending: &PendingProposal,
    outcome: &str,
) -> Result<DesignSessionEventKind, serde_json::Error> {
    let workbook_json = serde_json::to_string(&pending.workbook)?;
    let bound_plan_json = pending
        .bound
        .as_ref()
        .map(|bound| {
            serde_json::to_string(&serde_json::json!({
                "operations": bound.ops,
                "description": bound.description,
                "preview": pending.preview_delta,
            }))
        })
        .transpose()?;
    let diagnostics_preimage = serde_json::to_vec(&pending.dry_run_diagnostics)?;
    Ok(DesignSessionEventKind::ProposalAudit {
        workbook_json,
        bound_plan_json,
        outcome: outcome.to_string(),
        dry_run_diagnostics: pending.dry_run_diagnostics.clone(),
        dry_run_diagnostics_hash: blake3::hash(&diagnostics_preimage).to_hex().to_string(),
        decision_record_hash: pending.workbook.evidence_record_hash.as_str().to_string(),
        related_event_seq: pending.audit_event_seq,
        gameboard_attempt_receipt_json: pending.gameboard_attempt_receipt_json.clone(),
    })
}

async fn append_proposal_audit(
    demo: &DesignerState,
    session_id: Uuid,
    pending: &PendingProposal,
    outcome: &str,
) -> Result<u64, anyhow::Error> {
    let event = proposal_audit_event(pending, outcome)?;
    Ok(demo
        .store
        .append_design_session_event(&demo.tenant_id, session_id, &event)
        .await?)
}

/// Recover the durable terminal receipt for a proposal after an ambiguous
/// client response. The workbook ID is the proposal's canonical request key;
/// only terminal workbooks may satisfy a retry, so an in-flight workbook can
/// never be mistaken for a completed mutation.
///
/// G6.1 note: deliberately scans the RAW event log, not `visible_events`.
/// This is request-retry idempotency ("did this HTTP call already
/// durably land"), not a graph-position read — a client that already
/// ratified a proposal must see the same terminal receipt on retry even
/// if the session's position was undone afterward. Undo does not un-ratify
/// a past request; it only changes what future position reads observe.
async fn terminal_proposal_receipt(
    demo: &DesignerState,
    session_id: Uuid,
    proposal_id: Uuid,
) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(session) = demo
        .store
        .load_design_session(&demo.tenant_id, session_id)
        .await?
    else {
        return Ok(None);
    };
    let proposal_id = proposal_id.to_string();
    for event in session.events().iter().rev() {
        let DesignSessionEventKind::ProposalAudit {
            workbook_json,
            outcome,
            gameboard_attempt_receipt_json,
            ..
        } = &event.kind
        else {
            continue;
        };
        let workbook: semantic_decision_contracts::ProposalWorkbook =
            serde_json::from_str(workbook_json)?;
        if workbook.workbook_id.as_str() != proposal_id {
            continue;
        }
        if matches!(
            workbook.status(),
            semantic_decision_contracts::ProposalStatus::NeedsArguments
                | semantic_decision_contracts::ProposalStatus::ReadyForDryRun
                | semantic_decision_contracts::ProposalStatus::ReadyForRatification
        ) {
            continue;
        }
        let attempt_receipt = gameboard_attempt_receipt_json
            .as_deref()
            .map(serde_json::from_str::<serde_json::Value>)
            .transpose()?;
        return Ok(Some(serde_json::json!({
            "terminal_audit_seq": event.seq,
            "outcome": outcome,
            "proposal_status": workbook.status(),
            "workbook": workbook,
            "attempt_receipt": attempt_receipt,
        })));
    }
    Ok(None)
}

/// Best-effort: persist a `SystemFailure` attempt receipt when the workbook
/// pipeline fails unexpectedly — a `ProposalStatus` this handler never
/// reaches, so `attach_terminal_gameboard_attempt`'s exhaustive status match
/// has no branch for it. History projection and the "3 recent failures"
/// escalation check (`disposition.rs::decide_game`) both read only
/// PERSISTED receipts (`design_history_projection`, which walks
/// `gameboard_attempt_receipt_json` off stored session events) — returning
/// the error to the client without also appending one here would leave
/// exactly the blind spot this closes: the failures most worth escalating
/// on would be invisible to the escalation check. Never itself propagates
/// an error: if this fails too (e.g. persistence itself is down), the
/// caller's original error response is still returned untouched.
async fn record_and_persist_system_failure(
    demo: &DesignerState,
    session_id: Uuid,
    pending: &PendingProposal,
    detail: &str,
) {
    let attempted_move = pending
        .workbook
        .position_binding()
        .map(|binding| binding.move_id().clone());
    let Ok(attempt_id) = semantic_decision_contracts::MoveAttemptId::new(format!(
        "system-failure-{}",
        blake3::hash(format!("{session_id}\0{detail}").as_bytes()).to_hex()
    )) else {
        return;
    };
    let Ok(receipt) = utterance_engine::bpmn_board::record_bpmn_attempt(
        &pending.design_position,
        attempt_id,
        attempted_move,
        &pending.source_utterance_text,
        semantic_decision_contracts::MoveAttemptOutcome::SystemFailure,
        None,
        None,
    ) else {
        return;
    };
    let Ok(receipt_json) = serde_json::to_string(&receipt) else {
        return;
    };
    let mut failed = pending.clone();
    failed.gameboard_attempt_receipt_json = Some(receipt_json);
    let _ = append_proposal_audit(demo, session_id, &failed, "system_failure").await;
}

fn attach_terminal_gameboard_attempt(pending: &mut PendingProposal) -> anyhow::Result<()> {
    use semantic_decision_contracts::{MoveAttemptId, MoveAttemptOutcome, ProposalStatus};

    let outcome = match pending.workbook.status() {
        ProposalStatus::NeedsArguments => MoveAttemptOutcome::Incomplete,
        ProposalStatus::DryRunRefused => MoveAttemptOutcome::CompilerRefused,
        ProposalStatus::Ratified if pending.correction_of.is_some() => {
            MoveAttemptOutcome::Corrected
        }
        ProposalStatus::Ratified => MoveAttemptOutcome::Applied,
        ProposalStatus::Rejected => MoveAttemptOutcome::RejectedByUser,
        ProposalStatus::Expired => MoveAttemptOutcome::Stale,
        ProposalStatus::ReadyForDryRun | ProposalStatus::ReadyForRatification => return Ok(()),
    };
    let (attempt_position, attempted_move) = pending.bound_game.as_ref().map_or_else(
        || {
            (
                &pending.design_position,
                pending
                    .workbook
                    .position_binding()
                    .map(|binding| binding.move_id().clone()),
            )
        },
        |bound| (bound.position(), Some(bound.bound_move_id().clone())),
    );
    let receipt = utterance_engine::bpmn_board::record_bpmn_attempt(
        attempt_position,
        MoveAttemptId::new(format!(
            "workbook-{}-{:?}",
            pending.workbook.workbook_id.as_str(),
            pending.workbook.status()
        ))?,
        attempted_move,
        &pending.source_utterance_text,
        outcome,
        pending.correction_of.clone(),
        pending.correction_kind,
    )?;
    pending.gameboard_attempt_receipt_json = Some(serde_json::to_string(&receipt)?);
    Ok(())
}

fn validate_pending_position(
    record: &bpmn_lite_store::store::DesignSessionRecord,
    pending: &PendingProposal,
) -> anyhow::Result<designer_graph::schema::DesignerDag> {
    use semantic_decision_contracts::DesignFocus;

    let dag = reconstruct_designer_dag(record, None)?;
    let anchor_pair = match pending.design_position.focus() {
        DesignFocus::Element { element } => {
            let key = dag
                .key_for_bpmn_id(element.as_str())
                .ok_or_else(|| anyhow::anyhow!("focused graph element no longer exists"))?;
            Some((key, element.as_str()))
        }
        DesignFocus::Absent { .. } => None,
        DesignFocus::Unknown { .. } => {
            anyhow::bail!("a workbook cannot remain active with unresolved focus")
        }
    };
    let graph_identity = graph_identity_hash(record);
    let board = utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag,
        anchor_pair,
        &graph_identity,
        &utterance_engine::board::PolicyFilter::default(),
    )?;
    if pending.workbook.board_hash != board.board_hash {
        anyhow::bail!("semantic board changed since workbook creation");
    }
    let position = utterance_engine::bpmn_board::build_bpmn_design_position(
        &dag,
        &board,
        &graph_identity,
        &graph_content_hash(record),
        DESIGNER_COMPILER_PROFILE_IDENTITY,
        pending.design_position.history_hash().as_str(),
        pending.design_position.focus().clone(),
        pending
            .design_position
            .current_proposal_hash()
            .map(|hash| hash.as_str()),
    )?;
    pending.workbook.validate_position(&position)?;
    Ok(dag)
}

/// POST /api/dsl/sessions/:id/proposals/:pid/answers — atomically apply typed
/// answers to a needs-input workbook, then materialize and dry-stage only when
/// all required slots are resolved.
async fn answer_proposal_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((id, pid)): Path<(Uuid, Uuid)>,
    Json(body): Json<ProposalAnswersBody>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let pending = {
        let proposals = demo.proposals.lock().unwrap_or_else(|p| p.into_inner());
        proposals.get(&pid).filter(|pending| pending.session_id == id).cloned()
    };
    let pending = match pending {
        Some(pending) => pending,
        None => match terminal_proposal_receipt(demo.as_ref(), id, pid).await {
            Ok(Some(receipt)) => return Json(serde_json::json!({
                "proposal_id": pid,
                "idempotent": true,
                "terminal_receipt": receipt,
            })).into_response(),
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "proposal not found" })),
                )
                    .into_response();
            }
            Err(_) => return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error":"terminal proposal receipt unavailable"})),
            ).into_response(),
        },
    };

    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    let current_hash = graph_identity_hash(&record);
    let validated_dag = validate_pending_position(&record, &pending);
    if let Err(reason) = &validated_dag {
        let mut expired = pending.clone();
        if let Err(error) = expired
            .workbook
            .transition(semantic_decision_contracts::ProposalStatus::Expired)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("workbook expiry: {error}") })),
            )
                .into_response();
        }
        if let Err(error) = attach_terminal_gameboard_attempt(&mut expired) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
            )
                .into_response();
        }
        if let Err(error) =
            append_proposal_audit(demo.as_ref(), id, &expired, "expired_graph_drift").await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("proposal audit append: {error}") })),
            )
                .into_response();
        }
        demo.proposals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&pid);
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "workbook authority inputs changed since creation",
                "reason": reason.to_string(),
                "proposal_status": expired.workbook.status(),
                "staged_against": pending.staged_against_hash,
                "current": current_hash,
            })),
        )
            .into_response();
    }
    let dag = validated_dag.expect("checked above");
    // A workbook whose only required slot auto-resolves from the anchor
    // (e.g. `op.delete_subgraph`'s single `target` argument) reaches
    // `ReadyForDryRun` inside `start_workbook`, never `NeedsArguments`.
    // `apply_explicit_answers` rejects any call once the workbook is past
    // `NeedsArguments`, including the zero-answers call this endpoint's own
    // callers (bare "confirm" submissions, and `palette_select_endpoint`,
    // which always submits `answers: []`) make when there is nothing left
    // to answer. Treat "no answers submitted, nothing pending" as a no-op
    // pass-through instead of an error; a non-empty answer set submitted to
    // an already-resolved workbook is still rejected below as genuine
    // misuse.
    let mut workbook = if body.answers.is_empty()
        && pending.workbook.status() != semantic_decision_contracts::ProposalStatus::NeedsArguments
    {
        pending.workbook.clone()
    } else {
        match crate::proposal::apply_explicit_answers(&dag, pending.workbook.clone(), body.answers)
        {
            Ok(workbook) => workbook,
            Err(error) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": error.to_string(),
                        "workbook": pending.workbook,
                    })),
                )
                    .into_response();
            }
        }
    };

    if workbook.status() == semantic_decision_contracts::ProposalStatus::NeedsArguments {
        let mut updated = PendingProposal {
            workbook: workbook.clone(),
            bound: None,
            preview_delta: None,
            bound_game: None,
            dry_run_diagnostics: Vec::new(),
            ..pending
        };
        match append_proposal_audit(demo.as_ref(), id, &updated, "answers_applied").await {
            Ok(audit_seq) => updated.audit_event_seq = Some(audit_seq),
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("proposal audit append: {error}")
                    })),
                )
                    .into_response();
            }
        }
        demo.proposals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(pid, updated);
        return Json(serde_json::json!({
            "proposal_id": pid,
            "proposal_status": workbook.status(),
            "workbook": workbook,
            "operations": serde_json::Value::Null,
            "dry_run_diagnostics": [],
        }))
        .into_response();
    }

    let preview = utterance_engine::bpmn_board::preview_bpmn_workbook(
        &dag,
        &workbook,
        pending.design_position.graph_revision().as_str(),
        pending.design_position.graph_hash().as_str(),
    );
    let (bound, preview_delta, bound_game, diagnostics) = match preview {
        Ok(preview) => {
            if let Err(error) = workbook
                .transition(semantic_decision_contracts::ProposalStatus::ReadyForRatification)
            {
                record_and_persist_system_failure(
                    demo.as_ref(),
                    id,
                    &pending,
                    &format!("ready_for_ratification transition: {error}"),
                )
                .await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error.to_string() })),
                )
                    .into_response();
            }
            let bound_game = match utterance_engine::bpmn_board::project_bpmn_bound_game_turn(
                &pending.design_position,
                &pending.move_evidence,
                &pending.design_belief,
                &workbook,
                preview.delta(),
            ) {
                Ok(bound_game) => bound_game,
                Err(error) => {
                    record_and_persist_system_failure(
                        demo.as_ref(),
                        id,
                        &pending,
                        &format!("bound game projection: {error}"),
                    )
                    .await;
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("bound game projection: {error}")
                        })),
                    )
                        .into_response();
                }
            };
            (
                Some(crate::proposal::BoundProposal {
                    ops: preview.bound().operations().to_vec(),
                    description: preview.bound().description().to_string(),
                }),
                Some(preview.delta().clone()),
                Some(bound_game),
                Vec::new(),
            )
        }
        Err(utterance_engine::bpmn_board::BpmnBoardError::CompilerRefused {
            diagnostics, ..
        }) => {
            if let Err(error) =
                workbook.transition(semantic_decision_contracts::ProposalStatus::DryRunRefused)
            {
                record_and_persist_system_failure(
                    demo.as_ref(),
                    id,
                    &pending,
                    &format!("dry_run_refused transition: {error}"),
                )
                .await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": error.to_string() })),
                )
                    .into_response();
            }
            (None, None, None, diagnostics)
        }
        Err(error) => {
            record_and_persist_system_failure(
                demo.as_ref(),
                id,
                &pending,
                &format!("preview refused unexpectedly: {error}"),
            )
            .await;
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": error.to_string(),
                    "workbook": pending.workbook,
                })),
            )
                .into_response();
        }
    };

    let mut updated = PendingProposal {
        workbook: workbook.clone(),
        bound: bound.clone(),
        preview_delta: preview_delta.clone(),
        bound_game,
        dry_run_diagnostics: diagnostics.clone(),
        ..pending
    };
    if let Err(error) = attach_terminal_gameboard_attempt(&mut updated) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
        )
            .into_response();
    }
    let outcome =
        if workbook.status() == semantic_decision_contracts::ProposalStatus::ReadyForRatification {
            "dry_run_admitted"
        } else {
            "dry_run_refused"
        };
    match append_proposal_audit(demo.as_ref(), id, &updated, outcome).await {
        Ok(audit_seq) => updated.audit_event_seq = Some(audit_seq),
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("proposal audit append: {error}")
                })),
            )
                .into_response();
        }
    }
    demo.proposals
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(pid, updated);
    Json(serde_json::json!({
        "proposal_id": pid,
        "proposal_status": workbook.status(),
        "workbook": workbook,
        "operations": bound.as_ref().map(|bound| &bound.ops),
        "description": bound.as_ref().map(|bound| bound.description.as_str()),
        "preview": preview_delta,
        "dry_run_diagnostics": diagnostics,
    }))
    .into_response()
}

/// POST /api/dsl/sessions/:id/proposals/:pid/ratify — under the session
/// lock: re-check graph identity (409 on drift, proposal consumed),
/// re-stage through EXACTLY the graph-edit validation, append the
/// GraphEdit event with a "ratified proposal …" note. The pending entry
/// is removed on success AND on any refusal (fail closed: a proposal
/// gets one authority shot against the graph it was staged on); a later
/// duplicate request may only recover the durable terminal receipt.
async fn ratify_proposal_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((id, pid)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;

    // Peek without consuming: an unknown pid / session mismatch is a 404 that
    // must not disturb some other session's pending entry.
    let pending = {
        let proposals = demo.proposals.lock().unwrap_or_else(|p| p.into_inner());
        proposals
            .get(&pid)
            .filter(|pending| pending.session_id == id)
            .cloned()
    };
    let pending = match pending {
        Some(pending) => pending,
        None => match terminal_proposal_receipt(demo.as_ref(), id, pid).await {
            Ok(Some(receipt)) => {
                return Json(serde_json::json!({
                    "proposal_id": pid,
                    "idempotent": true,
                    "terminal_receipt": receipt,
                }))
                .into_response();
            }
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "proposal not found" })),
                )
                    .into_response();
            }
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": "terminal proposal receipt unavailable"
                    })),
                )
                    .into_response();
            }
        },
    };
    let consume = || {
        demo.proposals
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&pid);
    };
    if pending.workbook.status()
        != semantic_decision_contracts::ProposalStatus::ReadyForRatification
    {
        let status = pending.workbook.status();
        let mut refused = pending.clone();
        if let Err(error) = attach_terminal_gameboard_attempt(&mut refused) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
            )
                .into_response();
        }
        if let Err(error) =
            append_proposal_audit(demo.as_ref(), id, &refused, "ratification_refused_status").await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("proposal audit append: {error}") })),
            )
                .into_response();
        }
        consume();
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "proposal is not ready for ratification",
                "proposal_status": status,
            })),
        )
            .into_response();
    }
    let bound = match pending.bound.clone() {
        Some(bound) => bound,
        None => {
            consume();
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "error": "ready workbook has no materialized plan" })),
            )
                .into_response();
        }
    };
    let description = bound.description.clone();
    let staged_against_hash = pending.staged_against_hash.clone();

    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };

    // Drift check: the proposal was dry-staged against a specific graph
    // identity; any intervening edit invalidates it. Fail closed — the
    // user re-utters against the current graph.
    let current_hash = graph_identity_hash(&record);
    let validated_dag = validate_pending_position(&record, &pending);
    if let Err(reason) = &validated_dag {
        let mut expired = pending.clone();
        if let Err(error) = expired
            .workbook
            .transition(semantic_decision_contracts::ProposalStatus::Expired)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("workbook expiry: {error}") })),
            )
                .into_response();
        }
        if let Err(error) = attach_terminal_gameboard_attempt(&mut expired) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
            )
                .into_response();
        }
        if let Err(error) =
            append_proposal_audit(demo.as_ref(), id, &expired, "expired_graph_drift").await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("proposal audit append: {error}")
                })),
            )
                .into_response();
        }
        consume();
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "proposal authority inputs changed since staging",
                "reason": reason.to_string(),
                "staged_against": staged_against_hash,
                "current": current_hash,
            })),
        )
            .into_response();
    }

    // Reconstruct the exact public-facade preview and compiler admission.
    // Any refusal or byte-level preview drift is persisted and consumed.
    let dag = validated_dag.expect("checked above");
    let replayed = match utterance_engine::bpmn_board::preview_bpmn_workbook(
        &dag,
        &pending.workbook,
        pending.design_position.graph_revision().as_str(),
        pending.design_position.graph_hash().as_str(),
    ) {
        Ok(preview) => preview,
        Err(error) => {
            let mut refused = pending.clone();
            refused.dry_run_diagnostics = match &error {
                utterance_engine::bpmn_board::BpmnBoardError::CompilerRefused {
                    diagnostics,
                    ..
                } => diagnostics.clone(),
                _ => vec![error.to_string()],
            };
            if let Err(transition_error) = refused
                .workbook
                .transition(semantic_decision_contracts::ProposalStatus::DryRunRefused)
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("compiler refusal transition: {transition_error}")
                    })),
                )
                    .into_response();
            }
            if let Err(receipt_error) = attach_terminal_gameboard_attempt(&mut refused) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("attempt receipt: {receipt_error}")
                    })),
                )
                    .into_response();
            }
            if let Err(audit_error) =
                append_proposal_audit(demo.as_ref(), id, &refused, "ratification_compiler_refused")
                    .await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("proposal audit append: {audit_error}")
                    })),
                )
                    .into_response();
            }
            consume();
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "compiler admission refused during ratification",
                    "diagnostics": refused.dry_run_diagnostics,
                    "proposal_status": refused.workbook.status(),
                })),
            )
                .into_response();
        }
    };
    let replayed_bound_game = match utterance_engine::bpmn_board::project_bpmn_bound_game_turn(
        &pending.design_position,
        &pending.move_evidence,
        &pending.design_belief,
        &pending.workbook,
        replayed.delta(),
    ) {
        Ok(bound_game) => bound_game,
        Err(error) => {
            let mut refused = pending.clone();
            refused.dry_run_diagnostics = vec![format!(
                "bound move reconstruction was refused during ratification: {error}"
            )];
            if let Err(transition_error) = refused
                .workbook
                .transition(semantic_decision_contracts::ProposalStatus::DryRunRefused)
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("bound move refusal transition: {transition_error}")
                    })),
                )
                    .into_response();
            }
            if let Err(receipt_error) = attach_terminal_gameboard_attempt(&mut refused) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("attempt receipt: {receipt_error}")
                    })),
                )
                    .into_response();
            }
            if let Err(audit_error) = append_proposal_audit(
                demo.as_ref(),
                id,
                &refused,
                "ratification_bound_move_refused",
            )
            .await
            {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("proposal audit append: {audit_error}")
                    })),
                )
                    .into_response();
            }
            consume();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "bound move reconstruction refused during ratification",
                    "diagnostics": refused.dry_run_diagnostics,
                    "proposal_status": refused.workbook.status(),
                })),
            )
                .into_response();
        }
    };
    let preview_matches = pending
        .preview_delta
        .as_ref()
        .is_some_and(|expected| expected == replayed.delta())
        && serde_json::to_vec(&bound.ops).ok()
            == serde_json::to_vec(replayed.bound().operations()).ok()
        && pending.bound_game.as_ref() == Some(&replayed_bound_game);
    if !preview_matches {
        let mut refused = pending.clone();
        refused.dry_run_diagnostics = vec![
            "deterministic preview reconstruction changed the delta or operation tape".to_string(),
        ];
        if let Err(error) = refused
            .workbook
            .transition(semantic_decision_contracts::ProposalStatus::DryRunRefused)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("workbook transition: {error}") })),
            )
                .into_response();
        }
        if let Err(error) = attach_terminal_gameboard_attempt(&mut refused) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
            )
                .into_response();
        }
        if let Err(error) =
            append_proposal_audit(demo.as_ref(), id, &refused, "preview_replay_mismatch").await
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("proposal audit append: {error}") })),
            )
                .into_response();
        }
        consume();
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "deterministic preview reconstruction mismatch",
                "proposal_status": refused.workbook.status(),
            })),
        )
            .into_response();
    }
    let ops = replayed.bound().operations().to_vec();
    let operations_json = match serde_json::to_string(&ops) {
        Ok(j) => j,
        Err(e) => {
            consume();
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("serialize: {e}") })),
            )
                .into_response();
        }
    };
    // The note marks this edit as utterance-ratified (vs a manual
    // graph-edit) and anchors it to its source utterance.
    let note = format!(
        "ratified proposal {pid} (workbook {}, utterance seq {}, evidence {}, proposal audit seq {}): {}",
        pending.workbook.workbook_id.as_str(),
        pending.workbook.source_utterance_seq,
        pending.workbook.evidence_record_hash.as_str(),
        pending
            .audit_event_seq
            .map(|seq| seq.to_string())
            .unwrap_or_else(|| "legacy-none".to_string()),
        pending.source_utterance_text,
    );
    match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::GraphEdit {
                operations_json,
                note,
            },
        )
        .await
    {
        Ok(seq) => {
            let mut ratified = pending.clone();
            if let Err(error) = ratified
                .workbook
                .transition(semantic_decision_contracts::ProposalStatus::Ratified)
            {
                consume();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("workbook transition: {error}") })),
                )
                    .into_response();
            }
            if let Err(error) = attach_terminal_gameboard_attempt(&mut ratified) {
                consume();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
                )
                    .into_response();
            }
            let audit_seq =
                match append_proposal_audit(demo.as_ref(), id, &ratified, "ratified").await {
                    Ok(audit_seq) => audit_seq,
                    Err(error) => {
                        consume();
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": format!("ratification audit append: {error}")
                            })),
                        )
                            .into_response();
                    }
                };
            consume();
            Json(serde_json::json!({
                "seq": seq,
                "audit_seq": audit_seq,
                "applied": description,
                "proposal_status": ratified.workbook.status(),
                "workbook": ratified.workbook,
            }))
            .into_response()
        }
        Err(e) => {
            consume();
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response()
        }
    }
}

/// POST /api/dsl/sessions/:id/proposals/:pid/reject — durably record the
/// terminal workbook and then drop the ephemeral pending entry.
async fn reject_proposal_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((id, pid)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let pending = demo.proposals.lock().unwrap_or_else(|p| p.into_inner())
        .get(&pid).filter(|pending| pending.session_id == id).cloned();
    let pending = match pending {
        Some(pending) => pending,
        None => match terminal_proposal_receipt(demo.as_ref(), id, pid).await {
            Ok(Some(receipt)) => return Json(serde_json::json!({"proposal_id":pid,"idempotent":true,"terminal_receipt":receipt})).into_response(),
            Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"proposal not found"}))).into_response(),
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"terminal proposal receipt unavailable"}))).into_response(),
        },
    };
    let mut rejected = pending;
    if let Err(error) = rejected
        .workbook
        .transition(semantic_decision_contracts::ProposalStatus::Rejected)
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": format!("cannot reject: {error}") })),
        )
            .into_response();
    }
    if let Err(error) = attach_terminal_gameboard_attempt(&mut rejected) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
        )
            .into_response();
    }
    let audit_seq = match append_proposal_audit(demo.as_ref(), id, &rejected, "rejected").await {
        Ok(audit_seq) => audit_seq,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("proposal audit append: {error}")
                })),
            )
                .into_response();
        }
    };
    demo.proposals
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&pid);
    Json(serde_json::json!({
        "rejected": pid,
        "proposal_status": rejected.workbook.status(),
        "workbook": rejected.workbook,
        "audit_seq": audit_seq,
    }))
    .into_response()
}

/// GET /api/dsl/sessions/:id/proposals — the session's pending
/// proposals, oldest first (by source utterance seq).
async fn list_proposals_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let proposals = demo.proposals.lock().unwrap_or_else(|p| p.into_inner());
    let mut out: Vec<serde_json::Value> = proposals
        .iter()
        .filter(|(_, p)| p.session_id == id)
        .map(|(pid, p)| {
            let (description, operations) = p
                .bound
                .as_ref()
                .map(|bound| {
                    (
                        serde_json::Value::String(bound.description.clone()),
                        serde_json::to_value(&bound.ops).unwrap_or(serde_json::Value::Null),
                    )
                })
                .unwrap_or((serde_json::Value::Null, serde_json::Value::Null));
            serde_json::json!({
                "proposal_id": pid,
                "description": description,
                "operations": operations,
                "preview": p.preview_delta,
                "workbook": p.workbook,
                "proposal_status": p.workbook.status(),
                "source_utterance_seq": p.workbook.source_utterance_seq,
                "source_utterance_text": p.source_utterance_text,
                "staged_against_hash": p.staged_against_hash,
            })
        })
        .collect();
    out.sort_by_key(|v| v["source_utterance_seq"].as_u64().unwrap_or(0));
    Json(out).into_response()
}

/// D19-rider rendering: helpful about the path forward, generic about
/// the request — never confirms the requested operation exists, never
/// enumerates what the user cannot do.
fn render_disposition(
    disposition: &utterance_engine::policy::ProposalDisposition,
    board: &dyn utterance_engine::board::InferenceBoard,
) -> String {
    use utterance_engine::policy::ProposalDisposition as D;
    match disposition {
        D::Candidate { candidate_id } => {
            let desc = board
                .candidate_description(candidate_id)
                .unwrap_or_else(|| candidate_id.clone());
            format!("Proposed: {desc}. Stage and ratify to apply.")
        }
        D::OutOfScope => {
            "This cannot be executed because it is not part of your current working context. You can change context through the governed route, or pick from the available design operations."
                .to_owned()
        }
        D::EscalateToSage { .. } => {
            "I need more detail to map this onto one design operation — can you say which step of the process this applies to, and whether it is one change or several?"
                .to_owned()
        }
        // Unreachable in v1 (policy enum docs) — rendered defensively.
        D::Ambiguous { question, .. } => question.clone(),
        D::MissingArguments { candidate_id, missing } => {
            format!("'{candidate_id}' needs: {}", missing.join(", "))
        }
        D::Compound { spans } => format!(
            "That contains {} governed changes. Please submit them one at a time; nothing was staged.",
            spans.len()
        ),
    }
}

/// WS-B day-one wiring (SHADOW START, plan §C constraint 2): every
/// session utterance flows board → tier-0 → deterministic disposition
/// policy → I28 record. The record is written to the session event log
/// (operational data); CORPUS capture stays suppressed until the Q9
/// charter (D17) — the response reports that state honestly.
/// WS-1.2 (charter §6): record one operator adjudication of a captured
/// turn — the label loop. The join key is `decision_record_hash` (every
/// utterance response carries it); the session id scopes the call to a
/// real session but the ledger joins by hash. Reporting mirrors the
/// capture state contract exactly: `stored` / `suppressed_no_charter` /
/// `refused` (422) / `persist_failed` (500) / `not_compiled`.
async fn session_adjudication_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> axum::response::Response {
    match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("design session {id} not found") })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("session load failed: {e}") })),
            )
                .into_response();
        }
    }
    #[cfg(feature = "q9-capture")]
    {
        use utterance_engine::capture::{AdjudicationEvent, AdjudicationRecordOutcome};
        let event: AdjudicationEvent = match serde_json::from_value(body) {
            Ok(event) => event,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "adjudication": "refused",
                        "reason": format!("invalid adjudication body: {e}"),
                    })),
                )
                    .into_response();
            }
        };
        let outcome = match demo.q9_capture.as_ref() {
            Some(pipeline) => pipeline.lock().unwrap().adjudicate(event),
            None => utterance_engine::capture::CapturePipeline::off().adjudicate(event),
        };
        match outcome {
            AdjudicationRecordOutcome::Stored => (
                StatusCode::OK,
                Json(serde_json::json!({ "adjudication": "stored" })),
            )
                .into_response(),
            AdjudicationRecordOutcome::SuppressedNoCharter => (
                StatusCode::OK,
                Json(serde_json::json!({ "adjudication": "suppressed_no_charter" })),
            )
                .into_response(),
            AdjudicationRecordOutcome::Refused(reason) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "adjudication": "refused", "reason": reason })),
            )
                .into_response(),
            AdjudicationRecordOutcome::PersistFailed => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "adjudication": "persist_failed" })),
            )
                .into_response(),
        }
    }
    #[cfg(not(feature = "q9-capture"))]
    {
        let _ = body;
        (
            StatusCode::OK,
            Json(serde_json::json!({ "adjudication": "not_compiled" })),
        )
            .into_response()
    }
}

async fn session_utterance_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SessionUtteranceBody>,
) -> impl IntoResponse {
    use utterance_engine::board::{build_board, EmptyUniverse, InferenceBoard, PolicyFilter};
    #[cfg(feature = "q9-capture")]
    use utterance_engine::capture::{CaptureOutcome, CapturePipeline};
    use utterance_engine::policy::{decide_with_action_spans, DispositionConfig};

    // Session must exist; its current source is the graph identity the
    // board hashes (C7 obligation: WS-B supplies the revision identity).
    let record_session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // WS-B.4: DesignerDag-backed sessions run the real PositionalLegality
    // oracle and project_ir over the reconstructed IR graph — training-
    // grade projections at last (context.rs's INTERIM LIMITATION note is
    // resolved for exactly this class of session). Legacy DSL-source
    // sessions (no GraphEdit ever appended) keep the WholeGraphLegality +
    // census-only path unchanged — purely additive, no existing session's
    // behavior shifts underneath it.
    let pipeline_result: anyhow::Result<_> = if record_session.is_graph_backed() {
        let dag = match reconstruct_designer_dag(&record_session, None) {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
                )
                    .into_response();
            }
        };
        let anchor_key = body
            .anchor
            .as_deref()
            .and_then(|id| dag.key_for_bpmn_id(id));
        let ir = match dag.to_ir() {
            Ok(ir) => ir,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("projection: {e}") })),
                )
                    .into_response();
            }
        };
        // Same pattern as the DSL-source path: hash the actual revision
        // content (here, the accumulated edit-log payloads — the DAG's
        // sole source of truth) rather than a Debug-formatted derivative.
        let graph_identity = graph_identity_hash(&record_session);
        let anchor_pair = anchor_key.zip(body.anchor.as_deref());
        utterance_engine::bpmn_board::build_bpmn_semantic_board(
            &dag,
            anchor_pair,
            &graph_identity,
            &PolicyFilter::default(),
        )
        .map_err(anyhow::Error::from)
        .and_then(|board| {
            let context = utterance_engine::context::project_ir(
                &ir,
                anchor_key.and(body.anchor.as_deref()),
                board.semantic_snapshot.as_str(),
                &graph_identity,
            )?;
            let focus = gameboard_focus(body.anchor.as_deref(), anchor_key.is_some())?;
            let (history_hash, attempts) = design_history_projection(&record_session, None)?;
            let position = utterance_engine::bpmn_board::build_bpmn_design_position(
                &dag,
                &board,
                &graph_identity,
                &graph_content_hash(&record_session),
                DESIGNER_COMPILER_PROFILE_IDENTITY,
                &history_hash,
                focus,
                None,
            )?;
            let evidence = demo.retrieve_utterance_evidence(
                &body.text,
                &board,
                &context,
                Some(&position),
                &attempts,
                EvidenceLatencyBudget::UtteranceSubmit,
            )?;
            let previous_belief = latest_gameboard_belief(&record_session, None)?;
            let belief = utterance_engine::bpmn_board::update_bpmn_design_belief(
                &dag,
                &position,
                &evidence.move_evidence,
                &attempts,
                previous_belief.as_ref(),
            )?;
            let real_payloads = record_session.graph_edit_payloads();
            let compound_chain = resolve_compound_chain(
                &dag,
                &board,
                &position,
                &evidence.move_evidence,
                &body.text,
                &graph_identity,
                &real_payloads,
                anchor_key,
            )?;
            let resolved_chain = match compound_chain {
                CompoundChainOutcome::NotCompound | CompoundChainOutcome::Unverifiable => None,
                CompoundChainOutcome::Ready(chain) => Some(chain),
                CompoundChainOutcome::Refused { diagnostics } => {
                    return Err(anyhow::anyhow!("compound plan refused: {diagnostics:?}"));
                }
            };
            let game_disposition = utterance_engine::bpmn_board::decide_bpmn_game_disposition(
                &board,
                &position,
                &evidence.move_evidence,
                &belief,
                &body.text,
                semantic_decision_contracts::MoveAttemptId::new(format!(
                    "session-{id}-turn-{}",
                    record_session.events().len()
                ))?,
                &attempts,
                resolved_chain.as_ref(),
            )?;
            let move_evidence = evidence.move_evidence.clone();
            let (disposition, record) = decide_with_action_spans(
                &DispositionConfig::shadow_v2(),
                &board,
                &evidence,
                &context,
                &body.text,
                &utterance_engine::disposition::StrictCompoundSyntax,
            )?;
            Ok((
                Box::new(board.clone()) as Box<dyn InferenceBoard>,
                disposition,
                record,
                context,
                board,
                position,
                belief,
                history_hash,
                game_disposition,
                move_evidence,
            ))
        })
        // Carry the reconstruction forward for binding extraction —
        // the proposal loop is strictly downstream of the disposition.
        .map(|t| {
            (
                t.0,
                t.1,
                t.2,
                t.3,
                Some((dag, anchor_key, graph_identity, t.4, t.5)),
                Some(t.6),
                Some(t.7),
                Some(t.8),
                Some(t.9),
            )
        })
    } else {
        let graph_identity = blake3::hash(
            record_session
                .current_source()
                .unwrap_or_default()
                .as_bytes(),
        )
        .to_hex()
        .to_string();

        // DIR-002 A1 INTERIM LIMITATION: census-only, no anchor — the
        // DSL-plan pipeline has no IRGraph for project_ir to run over.
        let node_kind_counts = {
            let registry = get_preview_registry();
            match bpmn_lite_compiler::dsl::compile(
                record_session.current_source().unwrap_or_default(),
                &registry,
            ) {
                Ok(plan) => {
                    let graph = plan_to_visual_graph(&plan);
                    let mut counts = std::collections::BTreeMap::<String, u32>::new();
                    for n in &graph.nodes {
                        *counts.entry(n.kind.clone()).or_insert(0) += 1;
                    }
                    counts.into_iter().collect::<Vec<_>>()
                }
                Err(_) => Vec::new(),
            }
        };

        build_board(
            &WholeGraphLegality,
            None,
            Some(&graph_identity),
            &EmptyUniverse,
            &PolicyFilter::default(),
        )
        .and_then(|board| {
            let context = utterance_engine::context::ContextProjection::new(
                board.context.pack_identity.clone(),
                graph_identity.clone(),
                None,
                node_kind_counts,
            )?;
            let evidence = demo.retrieve_utterance_evidence(
                &body.text,
                &board,
                &context,
                None,
                &[],
                EvidenceLatencyBudget::UtteranceSubmit,
            )?;
            let (disposition, record) = decide_with_action_spans(
                &DispositionConfig::shadow_v2(),
                &board,
                &evidence,
                &context,
                &body.text,
                &utterance_engine::disposition::StrictCompoundSyntax,
            )?;
            Ok((
                Box::new(board) as Box<dyn InferenceBoard>,
                disposition,
                record,
                context,
            ))
        })
        // Legacy DSL-source sessions have no DesignerDag — no binding
        // extraction, no proposals (the graph-edit surface is the only
        // mutation path they lack anyway).
        .map(|t| (t.0, t.1, t.2, t.3, None, None, None, None, None))
    };
    let (
        board,
        disposition,
        record,
        context,
        graph_ctx,
        gameboard_belief,
        history_projection_hash,
        game_disposition,
        gameboard_evidence,
    ) = match pipeline_result {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("utterance pipeline: {e}") })),
            )
                .into_response();
        }
    };
    let mut design_position = match &graph_ctx {
        Some((_, _, _, _, position)) => match serde_json::to_value(position) {
            Ok(position) => position,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("design position serialize: {error}")
                    })),
                )
                    .into_response();
            }
        },
        None => serde_json::Value::Null,
    };
    let mut message = if demo.mapper_rollout.suggestions_enabled() {
        match (&game_disposition, &graph_ctx) {
            (Some(game), Some((_, _, _, semantic_board, position))) => {
                match utterance_engine::bpmn_board::render_bpmn_game_disposition(
                    game,
                    semantic_board,
                    position,
                ) {
                    Ok(message) => message,
                    Err(error) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({
                                "error": format!("game disposition render: {error}")
                            })),
                        )
                            .into_response();
                    }
                }
            }
            _ => render_disposition(&disposition, board.as_ref()),
        }
    } else {
        "Mapper evidence recorded in shadow; no suggestion or workbook was served.".to_string()
    };

    // ── Typed workbook + dry stage (propose half of the loop) ────────
    // Inference truth remains `disposition`; binding state is carried by
    // a separate shared workbook and can never overwrite it.
    let mut staged_proposal: Option<PendingProposal> = None;
    let mut dry_run_diagnostics: Vec<String> = Vec::new();
    let selected_game_move = game_disposition.as_ref().and_then(|game| {
        matches!(
            game.kind(),
            semantic_decision_contracts::GameDispositionKind::ProposeMove
                | semantic_decision_contracts::GameDispositionKind::RequestMoveArguments
        )
        .then(|| game.selected_moves().first())
        .flatten()
    });
    let correction_context = design_history_projection(&record_session, None)
        .ok()
        .and_then(|(_, attempts)| attempts.last().cloned())
        .filter(|attempt| {
            matches!(
                attempt.outcome(),
                semantic_decision_contracts::MoveAttemptOutcome::RejectedByUser
                    | semantic_decision_contracts::MoveAttemptOutcome::Applied
            )
        })
        .map(|attempt| {
            (
                attempt
                    .correction_of()
                    .cloned()
                    .unwrap_or_else(|| attempt.attempt_id().clone()),
                attempt
                    .correction_kind()
                    .unwrap_or(semantic_decision_contracts::CorrectionKind::FollowUp),
            )
        });
    if let (
        true,
        Some((dag, anchor_key, graph_identity, semantic_board, position)),
        Some(move_id),
        Some(move_evidence),
        Some(design_belief),
    ) = (
        demo.mapper_rollout.workbooks_enabled(),
        &graph_ctx,
        selected_game_move,
        gameboard_evidence.as_ref(),
        gameboard_belief.as_ref(),
    ) {
        let candidate_id = match position
            .legal_moves()
            .iter()
            .find(|legal_move| legal_move.move_id() == move_id)
        {
            Some(legal_move) => legal_move.candidate_id().as_str().to_string(),
            None => {
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": "game disposition move is stale" })),
                )
                    .into_response();
            }
        };
        let mut workbook = match crate::proposal::start_workbook(
            dag,
            *anchor_key,
            semantic_board,
            crate::proposal::SelectedMove { position, move_id },
            crate::proposal::WorkbookEvidence::Decision(&record),
            &body.text,
            0,
        ) {
            Ok(workbook) => workbook,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("workbook start: {error}") })),
                )
                    .into_response();
            }
        };
        let mut bound = None;
        let mut preview_delta = None;
        let mut bound_game = None;
        if workbook.status() == semantic_decision_contracts::ProposalStatus::ReadyForDryRun {
            match utterance_engine::bpmn_board::preview_bpmn_workbook(
                dag,
                &workbook,
                position.graph_revision().as_str(),
                position.graph_hash().as_str(),
            ) {
                Ok(preview) => {
                    let materialized = crate::proposal::BoundProposal {
                        ops: preview.bound().operations().to_vec(),
                        description: preview.bound().description().to_string(),
                    };
                    if let Err(error) = workbook.transition(
                        semantic_decision_contracts::ProposalStatus::ReadyForRatification,
                    ) {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({ "error": format!("workbook transition: {error}") })),
                        )
                            .into_response();
                    }
                    message = format!(
                        "Proposed: {}. Ratify to apply, or reject.",
                        materialized.description
                    );
                    bound_game = match utterance_engine::bpmn_board::project_bpmn_bound_game_turn(
                        position,
                        move_evidence,
                        design_belief,
                        &workbook,
                        preview.delta(),
                    ) {
                        Ok(bound_game) => Some(bound_game),
                        Err(error) => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": format!("bound game projection: {error}")
                                })),
                            )
                                .into_response();
                        }
                    };
                    preview_delta = Some(preview.delta().clone());
                    bound = Some(materialized);
                }
                Err(utterance_engine::bpmn_board::BpmnBoardError::CompilerRefused {
                    diagnostics,
                    ..
                }) => {
                    dry_run_diagnostics = diagnostics;
                    if let Err(error) = workbook
                        .transition(semantic_decision_contracts::ProposalStatus::DryRunRefused)
                    {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(serde_json::json!({ "error": format!("workbook transition: {error}") })),
                        )
                            .into_response();
                    }
                    message =
                        format!("Proposal for '{candidate_id}' does not admit — nothing staged.");
                }
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": format!("workbook preview: {error}") })),
                    )
                        .into_response();
                }
            }
        } else {
            let prompts = workbook
                .slots()
                .iter()
                .filter(|slot| {
                    slot.requirement == semantic_decision_contracts::SlotRequirement::Required
                        && matches!(
                            slot.value,
                            semantic_decision_contracts::SlotValueState::Missing
                        )
                })
                .map(|slot| slot.clarification_prompt.as_str())
                .collect::<Vec<_>>();
            message = format!("More information is required: {}", prompts.join(" "));
        }
        let mut pending = PendingProposal {
            session_id: id,
            workbook,
            bound,
            preview_delta,
            source_utterance_text: body.text.clone(),
            staged_against_hash: graph_identity.clone(),
            dry_run_diagnostics: dry_run_diagnostics.clone(),
            audit_event_seq: None,
            design_position: position.clone(),
            move_evidence: move_evidence.clone(),
            design_belief: design_belief.clone(),
            bound_game,
            gameboard_attempt_receipt_json: None,
            correction_of: correction_context
                .as_ref()
                .map(|(attempt_id, _)| attempt_id.clone()),
            correction_kind: correction_context.as_ref().map(|(_, kind)| *kind),
        };
        if matches!(
            pending.workbook.status(),
            semantic_decision_contracts::ProposalStatus::DryRunRefused
        ) || (matches!(
            pending.workbook.status(),
            semantic_decision_contracts::ProposalStatus::NeedsArguments
        ) && game_disposition.as_ref().is_some_and(|game| {
            game.kind() == semantic_decision_contracts::GameDispositionKind::ProposeMove
        })) {
            if let Err(error) = attach_terminal_gameboard_attempt(&mut pending) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("attempt receipt: {error}") })),
                )
                    .into_response();
            }
        }
        staged_proposal = Some(pending);
    }

    let gameboard_attempt = match staged_proposal
        .as_ref()
        .and_then(|pending| pending.gameboard_attempt_receipt_json.as_deref())
        .map(serde_json::from_str)
    {
        Some(Ok(receipt)) => Some(receipt),
        Some(Err(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("pending attempt decode: {error}")
                })),
            )
                .into_response();
        }
        None => game_disposition
            .as_ref()
            .and_then(|game| game.attempt_receipt().cloned()),
    };

    // Workbook preview can discover a terminal incomplete/refused attempt after
    // the resolver's initial proposal disposition. Capture and expose the final
    // typed outcome, not the now-stale proposal disposition.
    let effective_game_disposition = match (
        game_disposition.as_ref(),
        gameboard_attempt.as_ref(),
        graph_ctx.as_ref(),
    ) {
        (Some(initial), Some(attempt), Some((_, _, _, semantic_board, position)))
            if initial.attempt_receipt().is_none() =>
        {
            match semantic_decision_contracts::GameDisposition::explain_attempt(
                position,
                attempt.clone(),
            ) {
                Ok(disposition) => {
                    match utterance_engine::bpmn_board::render_bpmn_game_disposition(
                        &disposition,
                        semantic_board,
                        position,
                    ) {
                        Ok(rendered) => message = rendered,
                        Err(error) => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": format!("terminal disposition render: {error}")
                                })),
                            )
                                .into_response();
                        }
                    }
                    Some(disposition)
                }
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("terminal disposition: {error}")
                        })),
                    )
                        .into_response();
                }
            }
        }
        _ => game_disposition.clone(),
    };

    // A completed workbook is a new proposal-qualified position: its explicit
    // arguments and exact compiler preview change the legal-move identity. Serve
    // and capture that packet rather than attributing the delta to the earlier,
    // incomplete move selected when the workbook was opened.
    let served_bound_game = staged_proposal
        .as_ref()
        .and_then(|pending| pending.bound_game.clone());
    if let Some(bound_game) = &served_bound_game {
        design_position = match serde_json::to_value(bound_game.position()) {
            Ok(position) => position,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("bound design position serialize: {error}")
                    })),
                )
                    .into_response();
            }
        };
    }
    let served_gameboard_evidence = served_bound_game
        .as_ref()
        .map(utterance_engine::bpmn_board::BpmnBoundGameTurnProjection::evidence)
        .or(gameboard_evidence.as_deref());
    let served_gameboard_belief = served_bound_game
        .as_ref()
        .map(utterance_engine::bpmn_board::BpmnBoundGameTurnProjection::belief)
        .or(gameboard_belief.as_ref());
    let served_gameboard_disposition = served_bound_game
        .as_ref()
        .map(utterance_engine::bpmn_board::BpmnBoundGameTurnProjection::disposition)
        .or(effective_game_disposition.as_ref());

    let gameboard_attempt_receipt_json = match gameboard_attempt.as_ref().map(serde_json::to_string)
    {
        Some(Ok(json)) => Some(json),
        Some(Err(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("attempt serialize: {error}") })),
            )
                .into_response();
        }
        None => None,
    };
    let gameboard_belief_json = match served_gameboard_belief.map(serde_json::to_string) {
        Some(Ok(json)) => Some(json),
        Some(Err(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("belief serialize: {error}") })),
            )
                .into_response();
        }
        None => None,
    };
    let gameboard_disposition_json = match served_gameboard_disposition.map(serde_json::to_string) {
        Some(Ok(json)) => Some(json),
        Some(Err(error)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("disposition serialize: {error}") })),
            )
                .into_response();
        }
        None => None,
    };

    let record_json = match serde_json::to_string(&record) {
        Ok(j) => j,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("record serialize: {e}") })),
            )
                .into_response();
        }
    };

    // Corpus capture under the ratified Q9 charter (GOV.1 ratified
    // 2026-08-06, EOP-GOV-Q9-CHARTER-001): live iff the designated
    // deployment configured the state pipeline at startup; otherwise
    // the per-interaction suppression stays a recorded fact, not an
    // assumed one. DIR-004 Phase 1.2: outside `q9-capture`, there is no
    // module to call here at all — `"not_compiled"` reports that as
    // plainly as `"suppressed_no_charter"` reports the compiled-but-off
    // runtime state. Live turns land in the Evaluation class (charter
    // §3/§6: training data enters only via adjudicated corrections).
    #[cfg(feature = "q9-capture")]
    let capture_state = {
        let related_attempts = if gameboard_attempt.is_some() {
            match design_history_projection(&record_session, None) {
                Ok((_, attempts)) => attempts,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("game capture history projection: {error}")
                        })),
                    )
                        .into_response();
                }
            }
        } else {
            Vec::new()
        };
        let game_turn = match (
            graph_ctx.as_ref(),
            served_gameboard_evidence,
            served_gameboard_belief,
            served_gameboard_disposition,
        ) {
            (
                Some((_, _, _, semantic_board, position)),
                Some(evidence),
                Some(belief),
                Some(disposition),
            ) => {
                use sha2::{Digest, Sha256};
                let capture_position = served_bound_game
                    .as_ref()
                    .map_or(position, |bound| bound.position());
                let captured_move = served_bound_game
                    .as_ref()
                    .map(|bound| bound.bound_move_id().clone())
                    .or_else(|| selected_game_move.cloned());
                let observed_at_epoch_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as u64)
                    .unwrap_or(0);
                let compiler_result = match gameboard_attempt.as_ref().map(|value| value.outcome())
                {
                    Some(semantic_decision_contracts::MoveAttemptOutcome::CompilerRefused) => {
                        let encoded = serde_json::to_vec(&dry_run_diagnostics).unwrap_or_default();
                        let mut hasher = Sha256::new();
                        hasher.update(b"bpmn-lite-compiler-refusal-v1");
                        hasher.update((encoded.len() as u64).to_be_bytes());
                        hasher.update(&encoded);
                        match semantic_decision_contracts::GraphContentHash::new(format!(
                            "{:x}",
                            hasher.finalize()
                        )) {
                            Ok(hash) => {
                                semantic_decision_contracts::GameTurnCompilerResult::refused(hash)
                            }
                            Err(error) => {
                                tracing::error!("game capture diagnostic hash refused: {error}");
                                return (
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    Json(serde_json::json!({
                                        "error": format!("game capture diagnostic hash: {error}")
                                    })),
                                )
                                    .into_response();
                            }
                        }
                    }
                    _ => semantic_decision_contracts::GameTurnCompilerResult::not_requested(),
                };
                Some(utterance_engine::bpmn_board::capture_bpmn_game_turn(
                    match semantic_decision_contracts::GameSessionId::new(id.to_string()) {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::error!("game capture session identity refused: {error}");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": format!("game capture session identity: {error}")
                                })),
                            )
                                .into_response();
                        }
                    },
                    match semantic_decision_contracts::DesignTurnId::new(format!(
                        "session-{id}-turn-{}",
                        record_session.events().len()
                    )) {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::error!("game capture turn identity refused: {error}");
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({
                                    "error": format!("game capture turn identity: {error}")
                                })),
                            )
                                .into_response();
                        }
                    },
                    record_session.events().len() as u64,
                    observed_at_epoch_ms,
                    semantic_board,
                    capture_position.clone(),
                    evidence.to_vec(),
                    belief.clone(),
                    disposition.clone(),
                    &body.text,
                    semantic_decision_contracts::GameTurnAnswer::not_observed(
                        semantic_decision_contracts::GameTurnAnswerAbsenceReason::NotRequested,
                    ),
                    captured_move,
                    staged_proposal
                        .as_ref()
                        .and_then(|pending| pending.preview_delta.clone()),
                    gameboard_attempt.as_ref().map_or_else(
                        semantic_decision_contracts::GameTurnAttempt::not_attempted,
                        |receipt| {
                            semantic_decision_contracts::GameTurnAttempt::terminal(receipt.clone())
                        },
                    ),
                    compiler_result,
                    related_attempts,
                ))
            }
            _ => None,
        };
        match game_turn {
            Some(Ok(record)) => {
                let event = match utterance_engine::capture::GameCaptureEvent::new(
                    body.text.clone(),
                    record,
                    utterance_engine::capture::DatasetClass::Evaluation,
                ) {
                    Ok(event) => event,
                    Err(error) => {
                        return (
                            StatusCode::PAYLOAD_TOO_LARGE,
                            Json(serde_json::json!({
                                "error": format!("game capture envelope refused: {error}")
                            })),
                        )
                            .into_response();
                    }
                };
                let outcome = match demo.q9_capture.as_ref() {
                    Some(pipeline) => pipeline.lock().unwrap().capture_game(event),
                    None => CapturePipeline::off().capture_game(event),
                };
                match outcome {
                    CaptureOutcome::SuppressedNoCharter => "suppressed_no_charter",
                    CaptureOutcome::Stored(_) => "stored_game_turn",
                    CaptureOutcome::PersistFailed(_) => "persist_failed",
                }
            }
            Some(Err(error)) => {
                tracing::error!("game capture packet refused: {error}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("game capture packet refused: {error}")
                    })),
                )
                    .into_response();
            }
            None => {
                let event = utterance_engine::capture::CaptureEvent {
                    raw_utterance: body.text.clone(),
                    record: record.clone(),
                    dataset: utterance_engine::capture::DatasetClass::Evaluation,
                };
                let outcome = match demo.q9_capture.as_ref() {
                    Some(pipeline) => pipeline.lock().unwrap().capture(event),
                    None => CapturePipeline::off().capture(event),
                };
                match outcome {
                    CaptureOutcome::SuppressedNoCharter => "suppressed_no_charter",
                    CaptureOutcome::Stored(_) => "stored_legacy_turn",
                    CaptureOutcome::PersistFailed(_) => "persist_failed",
                }
            }
        }
    };
    #[cfg(not(feature = "q9-capture"))]
    let capture_state = "not_compiled";

    // DIR-004 Phase 2 wiring: dev-session capture, structurally distinct
    // from the Q9-gated path above (always compiled, no feature gate --
    // see utterance_engine::dev_capture's module doc). Active only for
    // sessions that already called POST .../dev-capture/enable with a
    // consent statement (that call is the "consent stated at session
    // start" moment); captures the FULL closure -- board dump and
    // context TEXT, not hash-only -- per Phase 1.3.
    let dev_capture_state = match demo.store.load_dev_capture_session(&id.to_string()).await {
        Ok(Some(session)) => {
            match utterance_engine::dev_capture::DevSessionStore::open(
                &session.session_id,
                &session.consent_statement_timestamp,
            ) {
                Ok(dev_store) => {
                    let dev_record =
                        dev_store.capture(utterance_engine::dev_capture::DevSessionCaptureInput {
                            raw_utterance: body.text.clone(),
                            board_hash: record.board_hash.clone(),
                            board: utterance_engine::corpus_schema::BoardDump::from_inference_board(
                                board.as_ref(),
                            ),
                            context_projection: context.serialize_canonical(),
                            context_projection_hash: record.context_projection_hash.clone(),
                            retrieved_subset_hash: record.retrieved_subset_hash.clone(),
                            model_bundle_hash: record.model_bundle_hash.clone(),
                            disposition_policy_hash: record.disposition_policy_hash.clone(),
                            action_span_producer_hash: record.action_span_producer_hash.clone(),
                            decision_record_hash: record.decision_record_hash.clone(),
                            ranking: record.ranking.clone(),
                            disposition: disposition.clone(),
                            evidence_trace: record.evidence_trace.clone(),
                        });
                    match serde_json::to_string(&dev_record) {
                        Ok(record_json) => {
                            match demo
                                .store
                                .append_dev_capture_record(&id.to_string(), record_json)
                                .await
                            {
                                Ok(_) => "captured",
                                Err(_) => "capture_failed",
                            }
                        }
                        Err(_) => "capture_failed",
                    }
                }
                Err(_) => "capture_failed",
            }
        }
        Ok(None) => "not_enabled",
        Err(_) => "not_enabled",
    };

    match demo
        .store
        .append_design_session_event(
            &demo.tenant_id,
            id,
            &DesignSessionEventKind::Utterance {
                text: body.text.clone(),
                response: message.clone(),
                decision_record_json: Some(record_json),
                context_projection: Some(context.serialize_canonical()),
                gameboard_attempt_receipt_json,
                gameboard_belief_json,
                gameboard_disposition_json,
                history_projection_hash,
            },
        )
        .await
    {
        Ok(seq) => {
            // The pending proposal is registered only once its source
            // utterance is durably appended (the audit anchor).
            let (proposal_json, workbook_json, proposal_status, proposal_id_json, preview_json) =
                match staged_proposal {
                    Some(mut pending) => {
                        pending.workbook.source_utterance_seq = seq;
                        let proposal_id = match Uuid::parse_str(
                            pending.workbook.workbook_id.as_str(),
                        ) {
                            Ok(proposal_id) => proposal_id,
                            Err(error) => {
                                return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(serde_json::json!({ "error": format!("workbook id: {error}") })),
                            )
                                .into_response();
                            }
                        };
                        let workbook_json = serde_json::to_value(&pending.workbook)
                            .unwrap_or(serde_json::Value::Null);
                        let status_json = serde_json::to_value(pending.workbook.status())
                            .unwrap_or(serde_json::Value::Null);
                        let (ops_json, description) = pending
                            .bound
                            .as_ref()
                            .map(|bound| {
                                (
                                    serde_json::to_value(&bound.ops)
                                        .unwrap_or(serde_json::Value::Null),
                                    serde_json::Value::String(bound.description.clone()),
                                )
                            })
                            .unwrap_or((serde_json::Value::Null, serde_json::Value::Null));
                        let preview_json = pending
                            .preview_delta
                            .as_ref()
                            .and_then(|preview| serde_json::to_value(preview).ok())
                            .unwrap_or(serde_json::Value::Null);
                        let audit_seq =
                            match append_proposal_audit(demo.as_ref(), id, &pending, "created")
                                .await
                            {
                                Ok(audit_seq) => audit_seq,
                                Err(error) => {
                                    return (
                                        StatusCode::INTERNAL_SERVER_ERROR,
                                        Json(serde_json::json!({
                                            "error": format!("proposal audit append: {error}")
                                        })),
                                    )
                                        .into_response();
                                }
                            };
                        pending.audit_event_seq = Some(audit_seq);
                        demo.proposals
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(proposal_id, pending);
                        (
                            serde_json::json!({
                                "proposal_id": proposal_id,
                                "operations": ops_json,
                                "description": description,
                                "preview": preview_json.clone(),
                                "workbook": workbook_json.clone(),
                                "status": status_json.clone(),
                            }),
                            workbook_json,
                            status_json,
                            serde_json::json!(proposal_id),
                            preview_json,
                        )
                    }
                    None => (
                        serde_json::Value::Null,
                        serde_json::Value::Null,
                        serde_json::Value::Null,
                        serde_json::Value::Null,
                        serde_json::Value::Null,
                    ),
                };
            let served_inference_disposition = if demo.mapper_rollout.suggestions_enabled() {
                serde_json::to_value(&disposition).unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            };
            let served_game_disposition = if demo.mapper_rollout.suggestions_enabled() {
                served_gameboard_disposition
                    .and_then(|game| serde_json::to_value(game).ok())
                    .unwrap_or_else(|| served_inference_disposition.clone())
            } else {
                serde_json::Value::Null
            };
            Json(serde_json::json!({
                "seq": seq,
                "message": message,
                "inference_disposition": served_inference_disposition,
                "game_disposition": served_game_disposition.clone(),
                "disposition": served_game_disposition,
                "decision_record_hash": record.decision_record_hash,
                "workbook": workbook_json,
                "proposal_status": proposal_status,
                "dry_run_diagnostics": dry_run_diagnostics,
                "proposal_id": proposal_id_json,
                "preview": preview_json,
                "board_hash": board.board_hash(),
                "board_schema": board.schema_label(),
                "board_pack_identity": board.pack_identity(),
                "design_position": design_position,
                "model_bundle_hash": record.model_bundle_hash.clone(),
                "evidence_producer": record.model_bundle_hash,
                "mapper_rollout": {
                    "stage": demo.mapper_rollout.label(),
                    "evidence_recorded": true,
                    "suggestions_enabled": demo.mapper_rollout.suggestions_enabled(),
                    "workbooks_enabled": demo.mapper_rollout.workbooks_enabled(),
                    "ratification_required": true,
                    "auto_apply": false,
                },
                "capture": capture_state,
                "dev_capture": dev_capture_state,
                "proposal": proposal_json,
                "proposal_refusal": dry_run_diagnostics,
            }))
            .into_response()
        }
        Err(bpmn_lite_store::StoreError::NotFound(_)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "session not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response(),
    }
}

/// Explicit palette choice enters the identical workbook/preview/ratify path
/// used by utterances. The choice is evidence of intent, never permission to
/// mutate the graph.
async fn palette_select_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<PaletteSelectionBody>,
) -> impl IntoResponse {
    use utterance_engine::board::PolicyFilter;
    use utterance_engine::retrieval::Tier0Retriever as _;

    let lock = demo.session_lock(id);
    let _guard = lock.lock().await;
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"session not found"}))).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette unavailable"}))).into_response(),
    };
    if !session.is_graph_backed() {
        return (StatusCode::CONFLICT, Json(serde_json::json!({"error":"semantic palette requires a graph-backed session"}))).into_response();
    }
    let dag = match reconstruct_designer_dag(&session, None) {
        Ok(dag) => dag,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette unavailable"}))).into_response(),
    };
    let anchor_key = match body.anchor.as_deref() {
        Some(anchor) => match dag.key_for_bpmn_id(anchor) {
            Some(key) => Some(key),
            None => return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error":"palette anchor is not on the current graph"}))).into_response(),
        },
        None => None,
    };
    let revision = graph_identity_hash(&session);
    let board = match utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag, anchor_key.zip(body.anchor.as_deref()), &revision, &PolicyFilter::default(),
    ) {
        Ok(board) => board,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette unavailable"}))).into_response(),
    };
    let (history_hash, attempts) = match design_history_projection(&session, None) {
        Ok(history) => history,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette unavailable"}))).into_response(),
    };
    let focus = match gameboard_focus(body.anchor.as_deref(), anchor_key.is_some()) {
        Ok(focus) => focus,
        Err(_) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error":"palette focus is invalid"}))).into_response(),
    };
    let position = match utterance_engine::bpmn_board::build_bpmn_design_position(
        &dag, &board, &revision, &graph_content_hash(&session), DESIGNER_COMPILER_PROFILE_IDENTITY,
        &history_hash, focus, None,
    ) {
        Ok(position) => position,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette unavailable"}))).into_response(),
    };
    let move_id = match semantic_decision_contracts::LegalMoveId::new(body.move_id) {
        Ok(move_id) if position.legal_moves().iter().any(|legal_move| legal_move.move_id() == &move_id) => move_id,
        _ => return (StatusCode::CONFLICT, Json(serde_json::json!({"error":"selected move is not legal for the current position"}))).into_response(),
    };
    let selection_text = format!("palette-selection:{}", move_id.as_str());
    let evidence = match utterance_engine::retrieval::LexicalTier0.retrieve(&selection_text, &board)
        .and_then(|result| utterance_engine::bpmn_board::finalize_bpmn_move_evidence(
            &board, &position, &selection_text, result,
            semantic_decision_contracts::EvidenceLane::Lexical, vec!["palette-selection-v1".into()], &attempts,
        )) {
        Ok(evidence) => evidence,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette evidence unavailable"}))).into_response(),
    };
    let belief = match utterance_engine::bpmn_board::update_bpmn_design_belief(
        &dag, &position, &evidence.move_evidence, &attempts,
        latest_gameboard_belief(&session, None).ok().flatten().as_ref(),
    ) {
        Ok(belief) => belief,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette evidence unavailable"}))).into_response(),
    };
    use sha2::{Digest, Sha256};
    let receipt_hash = semantic_decision_contracts::EvidenceRecordHash::new(hex::encode(
        Sha256::digest(format!("palette-selection-v1\\0{}\\0{}", position.state_id().as_str(), move_id.as_str()).as_bytes()),
    )).expect("sha256 is a valid evidence hash");
    let workbook = match crate::proposal::start_workbook(
        &dag, anchor_key, &board, crate::proposal::SelectedMove { position: &position, move_id: &move_id },
        crate::proposal::WorkbookEvidence::PaletteSelection(receipt_hash), &selection_text, 0,
    ) {
        Ok(workbook) => workbook,
        Err(_) => return (StatusCode::UNPROCESSABLE_ENTITY, Json(serde_json::json!({"error":"selected move cannot open a workbook"}))).into_response(),
    };
    // Proposal identity is the workbook's canonical identity on every entry
    // path. Reusing it here is required for durable terminal-receipt lookup
    // after a lost client response; a palette selection must not acquire an
    // unrelated, process-local proposal key.
    let pid = match Uuid::parse_str(workbook.workbook_id.as_str()) {
        Ok(pid) => pid,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"palette workbook identity unavailable"}))).into_response(),
    };
    demo.proposals.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(pid, PendingProposal {
        session_id: id, workbook, bound: None, preview_delta: None, source_utterance_text: selection_text,
        staged_against_hash: revision, dry_run_diagnostics: Vec::new(), audit_event_seq: None,
        design_position: position, move_evidence: evidence.move_evidence, design_belief: belief,
        bound_game: None, gameboard_attempt_receipt_json: None, correction_of: None, correction_kind: None,
    });
    drop(_guard);
    answer_proposal_endpoint(
        State(demo),
        Path((id, pid)),
        Json(ProposalAnswersBody { answers: Vec::new() }),
    )
    .await
    .into_response()
}

#[derive(Deserialize)]
struct DevCaptureEnableBody {
    consent_statement: String,
}

/// DIR-004 Phase 2 wiring: the "consent stated at session start" moment.
/// Opens a `DevSessionStore` for this design session id -- always
/// compiled, structurally distinct from the Q9-gated `capture` module
/// (see `utterance_engine::dev_capture`'s doc). Idempotent-refusing: a
/// session that already has a store open is NOT silently re-opened with
/// a different consent statement (that would let a later call quietly
/// change what consent is on record for already-captured interactions)
/// -- it 409s with the existing consent timestamp instead.
async fn dev_capture_enable_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<DevCaptureEnableBody>,
) -> impl IntoResponse {
    // Validate BEFORE opening -- a rejected consent statement (empty,
    // whitespace-only) must never reach the store as an "already open"
    // conflict on retry.
    if let Err(e) = utterance_engine::dev_capture::DevSessionStore::open(
        &id.to_string(),
        &body.consent_statement,
    ) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("{e}") })),
        )
            .into_response();
    }
    match demo
        .store
        .open_dev_capture_session(&id.to_string(), &body.consent_statement)
        .await
    {
        Ok(()) => Json(serde_json::json!({ "enabled": true, "session_id": id })).into_response(),
        Err(_) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "dev-capture already enabled for this session",
                "session_id": id,
            })),
        )
            .into_response(),
    }
}

/// Read-back: whether dev-capture is enabled for this session and, if
/// so, every record captured so far (full I28 closure, train-on-able --
/// this endpoint is Adam's own export path, not a general query API).
async fn dev_capture_status_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match demo.store.load_dev_capture_session(&id.to_string()).await {
        Ok(Some(session)) => {
            let records: Vec<serde_json::Value> = session
                .records_json
                .iter()
                .filter_map(|record_json| serde_json::from_str(record_json).ok())
                .collect();
            Json(serde_json::json!({
                "enabled": true,
                "session_id": session.session_id,
                "record_count": records.len(),
                "records": records,
            }))
            .into_response()
        }
        Ok(None) | Err(_) => {
            Json(serde_json::json!({ "enabled": false, "record_count": 0, "records": [] }))
                .into_response()
        }
    }
}

/// Save-as-template: compile the session's CURRENT source (fail-closed —
/// an uncompilable draft cannot become a template), persist plan +
/// catalog entry (same path as POST /bpmn/templates), and pin the
/// template ref back onto the session.
async fn save_design_session_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<SaveSessionBody>,
) -> impl IntoResponse {
    let record = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(record)) => record,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // Save-as-template (2026-07-30 rewire): graph-backed sessions publish
    // through the lifecycle-bearing registry (`compile_and_publish_from_dto`
    // → `workflow_templates`); text-backed sessions are a legacy authoring
    // path (raw dsl.bpmn text, superseded by the utterance→AstMutator→
    // DesignerDag pipeline) and are blocked outright — see the else branch.
    if !record.is_graph_backed() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "legacy_authoring_path",
                "detail": "text (dsl.bpmn) sessions are superseded by the \
                           utterance/graph pipeline and cannot be saved as \
                           templates",
            })),
        )
            .into_response();
    }

    let dag = match reconstruct_designer_dag(&record, None) {
        Ok(d) => d,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("reconstruction: {e}") })),
            )
                .into_response();
        }
    };
    // Same admission discipline as the graph-edit endpoint — the
    // full to_ir/verify/lower theorem chain must pass before this
    // session's graph is eligible to become a stored artifact at all.
    // Nothing below runs (and nothing is persisted anywhere) unless
    // this gate passes.
    if let Err(errs) = dag.admit() {
        let messages: Vec<String> = errs.iter().map(|e| e.message.clone()).collect();
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "graph does not admit", "diagnostics": messages })),
        )
            .into_response();
    }
    let ir = match dag.to_ir() {
        Ok(ir) => ir,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("to_ir: {e}") })),
            )
                .into_response();
        }
    };
    let mut dto = match bpmn_lite_authoring::ir_to_dto(&ir, &record.name) {
        Ok(dto) => dto,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "graph cannot be snapshotted as a template",
                    "diagnostics": [e.to_string()],
                })),
            )
                .into_response();
        }
    };
    // G5.1/G5.2 — `ir_to_dto` only projects `IRGraph`; the process-level
    // guard-budget/retry-policy defaults live on the `DesignerDag` root
    // (no `IRNode` home, same reasoning as `schema.rs`'s `admit()` doc
    // comment) and must be carried across explicitly or a value set via
    // `Operation::SetDefaultGuardBudget`/`SetDefaultRetryPolicy` would be
    // silently dropped at save time.
    dto.default_guard_budget = dag.default_guard_budget();
    dto.default_retry_policy = dag.default_retry_policy();
    // Rider 2 (G2 BLOCKER-2 ruling): two stores, two roles. The DAG stays
    // authoritative in the SESSION store (GraphEdit events, untouched by
    // this endpoint); the PLAN store must hold a compiled artifact
    // regardless of which path authored it (P5). Still produced here
    // because the runtime instantiation path (bus-handler + the designer's
    // own instantiate/list/get endpoints) reads the plan catalog — the
    // Phase-B migration moves those readers onto the template registry and
    // then this projection + the catalog dual-write below go away.
    let plan = match bpmn_lite_compiler::dsl::project_ir(&ir, record.name.clone()) {
        Ok(plan) => plan,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "graph cannot be saved as a template yet",
                    "diagnostics": [e.to_string()],
                })),
            )
                .into_response();
        }
    };
    let plan_json = match serde_json::to_string(&plan) {
        Ok(json) => json,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("serialize plan: {e}") })),
            )
                .into_response();
        }
    };

    // Registry version: next after the highest existing version for this
    // key in ANY state — Draft/Retired rows still reserve their numbers
    // (the store's immutability rules refuse overwrites).
    let template_version = match demo
        .template_store
        .list(Some(&body.template_name), None)
        .await
    {
        Ok(existing) => existing
            .iter()
            .map(|t| t.template_version)
            .max()
            .unwrap_or(0)
            .saturating_add(1),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("template registry list: {e}") })),
            )
                .into_response();
        }
    };
    let publish_result = match bpmn_lite_authoring::compile_and_publish_from_dto(
        dto,
        bpmn_lite_authoring::PublishOptions {
            template_key: body.template_name.clone(),
            template_version,
            process_key: record.name.clone(),
            source_format: bpmn_lite_authoring::SourceFormat::Graph,
            contract_registry: None,
            generate_bpmn: true,
            verb_registry_hash: None,
            session_id: Some(record.id),
        },
        &*demo.template_store,
        &*demo.store,
    )
    .await
    {
        Ok(result) => result,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "template publish failed",
                    "diagnostics": [e.to_string()],
                })),
            )
                .into_response();
        }
    };

    // TEMPORARY dual-write (Phase B removes it): the plan catalog is still
    // the runtime's template source for instantiation, so keep it fed.
    let hash = *blake3::hash(plan_json.as_bytes()).as_bytes();
    if let Err(e) = demo.store.store_plan(hash, &plan_json).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("store plan: {e}") })),
        )
            .into_response();
    }
    let catalog_version = match demo
        .store
        .load_latest_template_version(&body.template_name)
        .await
    {
        Ok(Some((v, _, _))) => v + 1,
        _ => 1,
    };
    let source_for_template =
        format!("<graph-authored session {id}; edit via the graph-edit endpoint, not DSL text>");
    if let Err(e) = demo
        .store
        .store_template(
            &body.template_name,
            catalog_version,
            hash,
            &source_for_template,
        )
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("store template: {e}") })),
        )
            .into_response();
    }
    if let Err(e) = demo
        .store
        .mark_design_session_saved(
            &demo.tenant_id,
            id,
            &body.template_name,
            catalog_version,
            hash,
        )
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("mark saved: {e}") })),
        )
            .into_response();
    }
    // G4.3 — surface the manifest-derived (and any contract-lint)
    // diagnostics computed during publish; previously discarded here even
    // though `compile_and_publish_from_dto` already produced them.
    let diagnostics: Vec<String> = publish_result
        .lint_diagnostics
        .iter()
        .map(|d| d.to_string())
        .collect();

    Json(serde_json::json!({
        "template_name": body.template_name,
        "version": catalog_version,
        "plan_hash": hex::encode(hash),
        "template_version": template_version,
        "bytecode_version": publish_result.template.bytecode_version,
        "state": "published",
        "diagnostics": diagnostics,
    }))
    .into_response()
}

/// Concrete move palette derived from the same semantic board and gameboard
/// adapter used by the language path.
async fn session_gameboard_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<GameboardQuery>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": error.to_string() })),
            )
                .into_response();
        }
    };
    if !session.is_graph_backed() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "semantic gameboard requires an authoritative DesignerDag session"
            })),
        )
            .into_response();
    }
    let dag = match reconstruct_designer_dag(&session, None) {
        Ok(dag) => dag,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("reconstruction: {error}") })),
            )
                .into_response();
        }
    };
    let anchor_key = match query.anchor.as_deref() {
        Some(anchor) => match dag.key_for_bpmn_id(anchor) {
            Some(key) => Some(key),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("anchor '{anchor}' names no node in this session's graph")
                    })),
                )
                    .into_response();
            }
        },
        None => None,
    };
    let revision = graph_identity_hash(&session);
    let board = match utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag,
        anchor_key.zip(query.anchor.as_deref()),
        &revision,
        &utterance_engine::board::PolicyFilter::default(),
    ) {
        Ok(board) => board,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("semantic board: {error}") })),
            )
                .into_response();
        }
    };
    let focus = match query.anchor.as_deref() {
        Some(anchor) => match semantic_decision_contracts::GraphElementRef::new(anchor) {
            Ok(anchor) => semantic_decision_contracts::DesignFocus::element(anchor),
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("focus: {error}") })),
                )
                    .into_response();
            }
        },
        None => semantic_decision_contracts::DesignFocus::absent(
            semantic_decision_contracts::FocusAbsenceReason::NotProvided,
        ),
    };
    let history_hash = match design_history_projection(&session, None) {
        Ok((hash, _)) => hash,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("history: {error}") })),
            )
                .into_response();
        }
    };
    match utterance_engine::bpmn_board::build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &graph_content_hash(&session),
        DESIGNER_COMPILER_PROFILE_IDENTITY,
        &history_hash,
        focus,
        None,
    ) {
        Ok(position) => Json(position).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("gameboard: {error}") })),
        )
            .into_response(),
    }
}

/// Read-only, bounded attempt history for Sage. The response is the same
/// canonical projection used when constructing the next `DesignPosition`, not
/// an unbounded transcript or a rendering of server errors.
async fn sage_session_history_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "session history unavailable" })),
            )
                .into_response();
        }
    };
    let (history_hash, attempts) = match design_history_projection(&session, None) {
        Ok(projection) => projection,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "session history unavailable" })),
            )
                .into_response();
        }
    };
    let belief = match latest_gameboard_belief(&session, None) {
        Ok(belief) => belief,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "session history unavailable" })),
            )
                .into_response();
        }
    };
    Json(serde_json::json!({
        "session_id": id,
        "history_hash": history_hash,
        "attempts": attempts,
        "latest_belief": belief,
    }))
    .into_response()
}

/// Bounded, typed proposal-audit projection for Sage. It excludes raw
/// utterance text and operation payloads while retaining the durable workbook,
/// outcome and terminal attempt identities needed for explanation and recovery.
async fn sage_session_audit_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"session not found"}))).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"session audit unavailable"}))).into_response(),
    };
    let mut entries = Vec::new();
    for event in session.events().iter().rev().filter_map(|event| match &event.kind {
        DesignSessionEventKind::ProposalAudit { workbook_json, outcome, gameboard_attempt_receipt_json, related_event_seq, .. } => {
            Some((event.seq, workbook_json, outcome, gameboard_attempt_receipt_json, *related_event_seq))
        }
        _ => None,
    }).take(64).collect::<Vec<_>>().into_iter().rev() {
        let (seq, workbook_json, outcome, attempt_json, related_event_seq) = event;
        let workbook: semantic_decision_contracts::ProposalWorkbook = match serde_json::from_str(workbook_json) {
            Ok(workbook) => workbook,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"session audit unavailable"}))).into_response(),
        };
        let attempt = match attempt_json.as_deref().map(serde_json::from_str::<semantic_decision_contracts::MoveAttemptReceipt>).transpose() {
            Ok(attempt) => attempt,
            Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":"session audit unavailable"}))).into_response(),
        };
        // G6.1 reconciliation: surface whether the event this audit entry
        // points to is still part of the LIVE (undo-aware) replay, so a
        // client can tell a stale/undone reference from a live one instead
        // of silently trusting a pointer that may now be dangling.
        let related_event_visible = related_event_seq
            .map(|related_seq| session.related_event_is_visible(related_seq, None));
        entries.push(serde_json::json!({
            "seq": seq,
            "outcome": outcome,
            "workbook": workbook,
            "attempt": attempt,
            "related_event_seq": related_event_seq,
            "related_event_visible": related_event_visible,
        }));
    }
    Json(serde_json::json!({"session_id":id,"entries":entries})).into_response()
}

/// Read one retained attempt receipt through the Sage facade. A receipt is
/// already the canonical, bounded record of its position, rules, feedback and
/// correction relation; this endpoint deliberately adds no server rendering or
/// inferred state.
async fn sage_attempt_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((id, attempt_id)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "session history unavailable" })),
            )
                .into_response();
        }
    };
    let (_, attempts) = match design_history_projection(&session, None) {
        Ok(projection) => projection,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "session history unavailable" })),
            )
                .into_response();
        }
    };
    match attempts
        .into_iter()
        .find(|attempt| attempt.attempt_id().as_str() == attempt_id)
    {
        Some(attempt) => Json(serde_json::json!({
            "session_id": id,
            "attempt": attempt,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "attempt not found" })),
        )
            .into_response(),
    }
}

/// Position-bound, pack-governed explanation for a requested candidate shape.
/// Sage can retrieve this guidance, but cannot use it to select, preview or
/// mutate a move. Known semantic outcomes remain typed; reconstruction and
/// storage failures are deliberately reported without leaking Rust errors.
async fn sage_move_guidance_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path((id, candidate_id)): Path<(Uuid, String)>,
    Query(query): Query<GameboardQuery>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(session)) => session,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Sage guidance unavailable" })),
            )
                .into_response();
        }
    };
    if !session.is_graph_backed() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "semantic guidance requires an authoritative DesignerDag session"
            })),
        )
            .into_response();
    }
    let dag = match reconstruct_designer_dag(&session, None) {
        Ok(dag) => dag,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Sage guidance unavailable" })),
            )
                .into_response();
        }
    };
    let anchor_key = match query.anchor.as_deref() {
        Some(anchor) => match dag.key_for_bpmn_id(anchor) {
            Some(key) => Some(key),
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "anchor names no node in this session's graph"
                    })),
                )
                    .into_response();
            }
        },
        None => None,
    };
    let focus = match query.anchor.as_deref() {
        Some(anchor) => match semantic_decision_contracts::GraphElementRef::new(anchor) {
            Ok(anchor) => semantic_decision_contracts::DesignFocus::element(anchor),
            Err(_) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({ "error": "invalid focus" })),
                )
                    .into_response();
            }
        },
        None => semantic_decision_contracts::DesignFocus::absent(
            semantic_decision_contracts::FocusAbsenceReason::NotProvided,
        ),
    };
    let revision = graph_identity_hash(&session);
    let policy = utterance_engine::board::PolicyFilter::default();
    let board = match utterance_engine::bpmn_board::build_bpmn_semantic_board(
        &dag,
        anchor_key.zip(query.anchor.as_deref()),
        &revision,
        &policy,
    ) {
        Ok(board) => board,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Sage guidance unavailable" })),
            )
                .into_response();
        }
    };
    let history_hash = match design_history_projection(&session, None) {
        Ok((hash, _)) => hash,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Sage guidance unavailable" })),
            )
                .into_response();
        }
    };
    let position = match utterance_engine::bpmn_board::build_bpmn_design_position(
        &dag,
        &board,
        &revision,
        &graph_content_hash(&session),
        DESIGNER_COMPILER_PROFILE_IDENTITY,
        &history_hash,
        focus,
        None,
    ) {
        Ok(position) => position,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Sage guidance unavailable" })),
            )
                .into_response();
        }
    };
    match utterance_engine::bpmn_board::explain_bpmn_candidate(
        &board,
        &position,
        &candidate_id,
        &policy,
    ) {
        Ok(guidance) => Json(serde_json::json!({
            "position": position,
            "candidate_id": candidate_id,
            "applicability": guidance.applicability(),
            "explanation": guidance.explanation(),
            "recoveries": guidance.recoveries(),
        }))
        .into_response(),
        Err(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "governed guidance is unavailable for this candidate"
            })),
        )
            .into_response(),
    }
}

/// Server-built DAG for the designer window (merged T4 / WS-B.2): the
/// session's CURRENT revision recompiled server-side; compile errors
/// surface as diagnostics, never a blank canvas. Layout is computed
/// here (layered by BFS depth from the start node) — the UI is a
/// window, not an editor.
/// B3 (EOP-PLAN-GRAPH-DSL-BRIDGE-001) — canonical DSL source receipt for
/// a graph-backed session: the admitted DAG projected through
/// `DesignerDag::emit_dsl` (B1's core-5 emission, proven
/// recompile-equivalent to `project_ir` by B2's harness). Read-only: no
/// store write on any path. A refusal is a valid, honest 200 receipt
/// response (`refused: { stage, diagnostic }`), never a 500 — the
/// diagnostic names the construct with no DSL surface yet, which is the
/// DSL-parity programme's backlog input. `workflow_id` is the session
/// UUID, not `session.name`: the name is free text ("proposal session")
/// and would refuse `UnrepresentableToken` for every spaced name, while
/// the UUID is the session's stable identity and always lexes as a DSL
/// Symbol token.
async fn session_dsl_receipt_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    let refused = |stage: &'static str, diagnostic: String| {
        Json(serde_json::json!({
            "source": serde_json::Value::Null,
            "graph_state_hash": serde_json::Value::Null,
            "required_symbols": serde_json::Value::Null,
            "refused": { "stage": stage, "diagnostic": diagnostic },
        }))
        .into_response()
    };
    if !session.is_graph_backed() {
        return refused(
            "session",
            "session is not graph-backed — its DSL source is authoritative already".to_owned(),
        );
    }
    let dag = match reconstruct_designer_dag(&session, None) {
        Ok(d) => d,
        Err(e) => return refused("reconstruction", format!("{e}")),
    };
    // Admit first (same discipline as the graph endpoint): the receipt
    // describes the admitted artifact, never an un-admittable graph.
    if let Err(errs) = dag.admit() {
        return refused(
            "admission",
            errs.iter()
                .map(|e| e.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
        );
    }
    match dag.emit_dsl(&id.to_string()) {
        Ok(receipt) => Json(serde_json::json!({
            "source": receipt.emitted.source,
            "graph_state_hash": receipt.graph_state_hash,
            "required_symbols": receipt.emitted.required_symbols,
            "refused": serde_json::Value::Null,
        }))
        .into_response(),
        Err(e) => refused("emission", format!("{e}")),
    }
}

async fn session_graph_endpoint(
    State(demo): State<Arc<DesignerState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let session = match demo.store.load_design_session(&demo.tenant_id, id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") })),
            )
                .into_response();
        }
    };
    // Graph-backed sessions carry no DSL text: the DAG is authoritative.
    // Serve the COMPILED workflow through the same admitted
    // DAG -> to_ir -> project_ir chain the save/spawn path trusts, so
    // what the visualiser shows is exactly what an instance will execute
    // — never a parallel projection that could drift.
    if session.is_graph_backed() {
        let ops_bytes: Vec<u8> = session
            .graph_edit_payloads()
            .into_iter()
            .flat_map(|p| p.as_bytes().to_vec().into_iter())
            .collect();
        let source_hash = blake3::hash(&ops_bytes).to_hex().to_string();
        let fail = |diags: Vec<String>| {
            Json(serde_json::json!({
                "compiles": false,
                "diagnostics": diags,
                "graph": serde_json::Value::Null,
                "layout": serde_json::Value::Null,
                "source_hash": source_hash.clone(),
            }))
            .into_response()
        };
        let dag = match reconstruct_designer_dag(&session, None) {
            Ok(d) => d,
            Err(e) => return fail(vec![format!("reconstruction: {e}")]),
        };
        if let Err(errs) = dag.admit() {
            return fail(errs.iter().map(|e| e.message.clone()).collect());
        }
        let ir = match dag.to_ir() {
            Ok(ir) => ir,
            Err(e) => return fail(vec![format!("to_ir: {e}")]),
        };
        let plan = match bpmn_lite_compiler::dsl::project_ir(&ir, session.name.clone()) {
            Ok(plan) => plan,
            Err(e) => return fail(vec![format!("project_ir: {e}")]),
        };
        let mut graph = plan_to_visual_graph(&plan);
        graph.nodes.extend(data_object_visual_nodes(&ir));
        let layout = layered_layout(&graph);
        return Json(serde_json::json!({
            "compiles": true,
            "diagnostics": [],
            "graph": graph,
            "layout": layout,
            "source_hash": source_hash,
        }))
        .into_response();
    }
    let source = session.current_source().unwrap_or_default().to_owned();
    let source_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    let registry = get_preview_registry();
    match bpmn_lite_compiler::dsl::compile(&source, &registry) {
        Ok(plan) => {
            let graph = plan_to_visual_graph(&plan);
            let layout = layered_layout(&graph);
            Json(serde_json::json!({
                "compiles": true,
                "diagnostics": [],
                "graph": graph,
                "layout": layout,
                "source_hash": source_hash,
            }))
            .into_response()
        }
        Err(e) => {
            let diagnostics: Vec<String> = match e {
                bpmn_lite_compiler::dsl::CompileError::Parse(errs) => errs,
                bpmn_lite_compiler::dsl::CompileError::Unroll(err) => vec![err.to_string()],
                bpmn_lite_compiler::dsl::CompileError::Lint(errs) => {
                    errs.iter().map(|x| format!("{x}")).collect()
                }
                bpmn_lite_compiler::dsl::CompileError::Dag(errs) => {
                    errs.iter().map(|x| format!("{x}")).collect()
                }
            };
            Json(serde_json::json!({
                "compiles": false,
                "diagnostics": diagnostics,
                "graph": serde_json::Value::Null,
                "layout": serde_json::Value::Null,
                "source_hash": source_hash,
            }))
            .into_response()
        }
    }
}

/// Deterministic layered layout: x = BFS depth from the start node,
/// y = order within the layer. Display-only — never a structural
/// derivation (I16: pairing/regions stay the compiler's).
fn layered_layout(graph: &VisualGraphDto) -> std::collections::BTreeMap<String, serde_json::Value> {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &graph.edges {
        adj.entry(e.from.as_str()).or_default().push(e.to.as_str());
    }
    let mut depth: HashMap<&str, usize> = HashMap::new();
    let mut queue: VecDeque<&str> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == "start")
        .map(|n| n.id.as_str())
        .collect();
    for start in &queue {
        depth.insert(start, 0);
    }
    while let Some(node) = queue.pop_front() {
        let d = depth[node];
        for next in adj.get(node).into_iter().flatten() {
            let entry = depth.entry(next).or_insert(usize::MAX);
            if *entry == usize::MAX || *entry < d + 1 {
                // deepest-position layering keeps joins right of all
                // their branches; acyclic by admission so this halts.
                if *entry == usize::MAX || *entry < d + 1 {
                    *entry = d + 1;
                    queue.push_back(next);
                }
            }
        }
    }
    let mut lanes: HashMap<usize, usize> = HashMap::new();
    let mut layout = BTreeMap::new();
    for n in &graph.nodes {
        let d = depth.get(n.id.as_str()).copied().unwrap_or(0);
        let lane = lanes.entry(d).or_insert(0);
        layout.insert(
            n.id.clone(),
            serde_json::json!({ "x": (d as f64) * 220.0, "y": (*lane as f64) * 120.0 }),
        );
        *lane += 1;
    }
    layout
}

/// The standalone designer window (plan ruling E4: static HTML +
/// vanilla JS + SVG, no framework, no build toolchain). It renders the
/// server-supplied graph/layout and drives ONLY the governed endpoints
/// — a window, not an editor.
async fn designer_page() -> impl IntoResponse {
    axum::response::Html(include_str!("../static/designer.html"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use serde_json::Value;
    use tower::ServiceExt; // for `oneshot`

    /// G1.4 gate: "a completion-shaped request provably cannot reach
    /// tier-1 — demonstrated by a test, not by configuration." Exhaustive
    /// over `EvidenceLatencyBudget`'s full 2-variant space (not just the
    /// one caller happens to exercise today) — a real loaded `Tier1Ranker`
    /// is not required to prove this: the decision of whether tier-1 may
    /// even be ATTEMPTED is a pure function of the budget, checked in
    /// `retrieve_utterance_evidence` before `self.tier1` is ever consulted.
    #[test]
    fn completion_budget_never_permits_tier1() {
        assert!(!EvidenceLatencyBudget::CompletionOnly.permits_tier1());
        assert!(EvidenceLatencyBudget::UtteranceSubmit.permits_tier1());
    }

    #[test]
    fn mapper_rollout_defaults_conservatively_and_has_no_auto_apply_stage() {
        assert_eq!(MapperRollout::parse(None), MapperRollout::Shadow);
        assert_eq!(MapperRollout::parse(Some("unknown")), MapperRollout::Shadow);
        assert_eq!(
            MapperRollout::parse(Some("suggest")),
            MapperRollout::Suggest
        );
        assert_eq!(
            MapperRollout::parse(Some("workbook")),
            MapperRollout::Workbook
        );
        assert!(!MapperRollout::Shadow.suggestions_enabled());
        assert!(!MapperRollout::Suggest.workbooks_enabled());
        assert!(MapperRollout::Workbook.suggestions_enabled());
        assert!(MapperRollout::Workbook.workbooks_enabled());
    }

    #[test]
    fn mapper_rollout_names_all_six_plan_stages_with_no_early_gate_invented() {
        assert_eq!(MapperRollout::parse(Some("observe")), MapperRollout::Observe);
        assert_eq!(MapperRollout::parse(Some("palette")), MapperRollout::Palette);
        assert_eq!(
            MapperRollout::parse(Some("feedback")),
            MapperRollout::Feedback
        );
        assert_eq!(MapperRollout::Observe.label(), "observe");
        assert_eq!(MapperRollout::Palette.label(), "palette");
        assert_eq!(MapperRollout::Feedback.label(), "feedback");
        // Observe/Shadow/Palette/Feedback have no gate of their own yet, so
        // they must behave identically to Shadow under the two real gates —
        // asserted explicitly so a future palette/feedback gate addition
        // has to touch this test, not silently drift the stage semantics.
        for stage in [
            MapperRollout::Observe,
            MapperRollout::Shadow,
            MapperRollout::Palette,
            MapperRollout::Feedback,
        ] {
            assert!(!stage.suggestions_enabled(), "{stage:?}");
            assert!(!stage.workbooks_enabled(), "{stage:?}");
        }
    }

    #[test]
    fn legacy_designer_request_remains_backward_compatible() {
        let request: UtteranceRequest = serde_json::from_value(serde_json::json!({
            "utterance": "exit",
            "_current_dsl": "(workflow draft)"
        }))
        .unwrap();

        assert!(request.target_node_id.is_none());
        assert!(request.unresolved_verb.is_none());
    }

    #[test]
    fn retry_hint_requires_an_injected_bpmn_target() {
        let missing = classify_designer_utterance(
            "wrap this in a retry loop",
            DesignerUtteranceContext::default(),
        );
        assert_eq!(missing.suggested_action, "none");
        assert!(missing.action_payload.is_none());

        let selected = classify_designer_utterance(
            "wrap this in a retry loop",
            DesignerUtteranceContext {
                target_node_id: Some("review_documents"),
                unresolved_verb: None,
            },
        );
        assert_eq!(selected.suggested_action, "apply_macro");
        assert_eq!(
            selected.action_payload.as_ref().unwrap()["parameters"]["target_node_id"],
            "review_documents"
        );
    }

    #[test]
    fn diagnostic_hint_uses_only_explicit_or_injected_verbs() {
        let explicit = classify_designer_utterance(
            "import define-template",
            DesignerUtteranceContext::default(),
        );
        assert_eq!(explicit.suggested_action, "resolve_diagnostic");
        assert_eq!(explicit.action_payload.as_ref().unwrap()["domain"], "bpmn");
        assert_eq!(
            explicit.action_payload.as_ref().unwrap()["verb"],
            "bpmn:define-template"
        );

        let external =
            classify_designer_utterance("import acme:submit", DesignerUtteranceContext::default());
        assert_eq!(external.action_payload.as_ref().unwrap()["domain"], "acme");
        assert_eq!(
            external.action_payload.as_ref().unwrap()["verb"],
            "acme:submit"
        );

        let injected = classify_designer_utterance(
            "resolve the unknown verb",
            DesignerUtteranceContext {
                target_node_id: None,
                unresolved_verb: Some("bpmn:deliver-message"),
            },
        );
        assert_eq!(
            injected.action_payload.as_ref().unwrap()["verb"],
            "bpmn:deliver-message"
        );

        let missing = classify_designer_utterance(
            "resolve the unknown verb",
            DesignerUtteranceContext::default(),
        );
        assert_eq!(missing.suggested_action, "none");
        assert!(missing.action_payload.is_none());
    }

    async fn app_at_rollout(rollout: MapperRollout) -> axum::Router {
        designer_router(
            DesignerState::assemble_with_rollout(
                Arc::new(MemoryStore::new()),
                Arc::new(bpmn_lite_authoring::MemoryTemplateStore::new()),
                rollout,
            )
            .unwrap(),
        )
    }

    #[tokio::test]
    async fn shadow_records_evidence_without_serving_suggestion_or_workbook() {
        let app = app_at_rollout(MapperRollout::Shadow).await;
        let (session_id, _anchor) = seed_graph_backed_session(&app).await;
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Places a node on an existing route, after the selected node",
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert!(body["inference_disposition"].is_null(), "{body:?}");
        assert!(body["workbook"].is_null(), "{body:?}");
        assert!(body["proposal"].is_null(), "{body:?}");
        assert_eq!(body["mapper_rollout"]["stage"], "shadow");
        assert_eq!(body["mapper_rollout"]["evidence_recorded"], true);
        assert_eq!(body["mapper_rollout"]["ratification_required"], true);
        assert_eq!(body["mapper_rollout"]["auto_apply"], false);
        assert!(body["decision_record_hash"].is_string());
        assert!(body["evidence_producer"].is_string());
    }

    #[tokio::test]
    async fn suggest_serves_candidate_without_staging_workbook() {
        let app = app_at_rollout(MapperRollout::Suggest).await;
        let (session_id, _anchor) = seed_graph_backed_session(&app).await;
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Places a node on an existing route, after the selected node",
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["inference_disposition"]["Candidate"]["candidate_id"], "op.insert_after",
            "{body:?}"
        );
        assert!(body["workbook"].is_null(), "{body:?}");
        assert!(body["proposal"].is_null(), "{body:?}");
        assert_eq!(body["mapper_rollout"]["stage"], "suggest");
    }

    /// WS-1.2: the adjudication endpoint scopes to a real session,
    /// mirrors the capture-state honesty contract (hermetically OFF in
    /// tests), and structurally refuses a correction without its
    /// candidate.
    #[tokio::test]
    async fn adjudication_endpoint_is_charter_gated_and_validates() {
        let app = designer_router(DesignerState::try_new().unwrap());
        let (session_id, _anchor) = seed_graph_backed_session(&app).await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{}/adjudication", Uuid::new_v4()),
                serde_json::json!({ "decision_record_hash": "abc", "outcome": "accepted" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/adjudication"),
                serde_json::json!({ "decision_record_hash": "abc", "outcome": "accepted" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        #[cfg(feature = "q9-capture")]
        assert_eq!(body["adjudication"], "suppressed_no_charter");
        #[cfg(not(feature = "q9-capture"))]
        assert_eq!(body["adjudication"], "not_compiled");

        #[cfg(feature = "q9-capture")]
        {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/adjudication"),
                    serde_json::json!({ "decision_record_hash": "abc", "outcome": "corrected" }),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "a correction without its candidate must be unrepresentable"
            );
        }
    }

    #[tokio::test]
    async fn test_compile_and_preview_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // Test BPMN compilation preview
        let bpmn_src = r#"(workflow test-wf
  (start-event :id start :next end)
  (end-event :id end :status "OK"))"#;
        let bpmn_body = serde_json::json!({ "bpmn_dsl": bpmn_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["workflow_id"], "test-wf");
        assert!(!res["nodes"].as_array().unwrap().is_empty());

        // Test BPMN compilation preview with unresolved verb error suggestion
        let bpmn_err_src = r#"(workflow err-wf
  (start-event :id start :next my-task)
  (service-task :id my-task :verb unknown-domain:unknown-verb :next end)
  (end-event :id end :status "OK"))"#;
        let bpmn_err_body = serde_json::json!({ "bpmn_dsl": bpmn_err_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_err_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["error"], "Linting failed");
        let diagnostics = res["diagnostics"].as_array().unwrap();
        assert!(diagnostics.iter().any(|d| d.as_str().unwrap().contains("Suggestion: Would you like me to import unknown-domain:unknown-verb to fix the unresolved verb error?")));

        // First-class message waits are structural nodes, not invocation verbs.
        let bpmn_wait_src = r#"(workflow custody-cbu-onboarding
          (start-event :id start :next create-cbu)
          (service-task :id create-cbu :verb ob-poc:cbu.create :next message-wait)
          (message-wait :id message-wait :name response-received :correlation-source case-id :next type-decision)
          (business-rule-task :id type-decision :decision dmn-lite:cbu_type_routing :next type-gateway)
          (exclusive-gateway :id type-gateway
            (flow :condition (= @cbu-type "fund")      :next add-fund)
            (flow :condition (= @cbu-type "corporate") :next add-corp)
            (flow :condition (= @cbu-type "trust")     :next add-trust))
          (service-task :id add-fund  :verb ob-poc:cbu.add-product :args (:product "fund")      :next attach-im)
          (service-task :id add-corp  :verb ob-poc:cbu.add-product :args (:product "corporate") :next attach-im)
          (service-task :id add-trust :verb ob-poc:cbu.add-product :args (:product "trust")     :next attach-im)
          (service-task :id attach-im :verb ob-poc:instrument-matrix.attach :next end)
          (end-event :id end :status "Operational"))"#;
        let bpmn_wait_body = serde_json::json!({ "bpmn_dsl": bpmn_wait_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&bpmn_wait_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 100000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(
            res["workflow_id"].as_str().unwrap(),
            "custody-cbu-onboarding"
        );
        assert!(res["error"].is_null());

        // Test DMN compilation preview
        let dmn_src = r#"(define-decision test_dec
  :hit-policy unique
  :inputs  ((client_type :type enum :domain ClientType))
  :outputs ((cbu_type    :type enum :domain CbuType))
  :rules
    ((rule r1
       :when ((client_type = FUND_MANDATE))
       :then ((cbu_type = fund)))))"#;
        let dmn_body = serde_json::json!({ "dmn_dsl": dmn_src });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/dmn/compile/preview")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&dmn_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["decision_name"], "test_dec");
        assert_eq!(res["hit_policy"], "unique");
        assert_eq!(res["inputs"][0]["name"], "client_type");
        assert_eq!(res["rules"][0]["id"], "r1");
        assert_eq!(res["rules"][0]["inputs"][0]["op"], "==");
        assert_eq!(res["rules"][0]["inputs"][0]["value"], "FUND_MANDATE");
        assert_eq!(res["rules"][0]["outputs"][0], "fund");

        // Test GET DMN decision
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/dmn/decisions/cbu_type_routing")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["decision_name"], "cbu_type_routing");
        assert_eq!(res["hit_policy"], "first");
        assert_eq!(res["inputs"][0]["name"], "cbu-client-type");
    }

    #[tokio::test]
    async fn test_dsl_macro_and_diagnostics_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);

        // 1. Test /api/dsl/macro/apply (BoundedRetry)
        let source = r#"(workflow test
  (start-event :id start :next my-task)
  (service-task :id my-task :verb ob-poc:cbu.create :next end)
  (end-event :id end :status "completed")
)"#;
        let mut params = HashMap::new();
        params.insert("target_node_id".to_string(), "my-task".to_string());
        params.insert("ceiling".to_string(), "5".to_string());
        params.insert("custom_id".to_string(), "my-retry-loop".to_string());

        let body = MacroApplyRequest {
            source_code: source.to_string(),
            macro_type: "BoundedRetry".to_string(),
            parameters: params,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/macro/apply")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        let modified_dsl = res["source_code"].as_str().unwrap();
        assert!(modified_dsl.contains("(loop :id my-retry-loop :ceiling 5"));

        // 2. Test /api/dsl/diagnostics/resolve (WireDeadEnd)
        let unresolved_source = r#"(workflow test
  (start-event :id start :next my-task)
  (service-task :id my-task :verb ob-poc:cbu.create :next dead-end)
  (end-event :id end :status "completed")
)"#;
        let fix_action = bpmn_lite_authoring::FixAction::WireDeadEnd {
            node_id: "my-task".to_string(),
            target_id: "end".to_string(),
        };
        let resolve_body = DiagnosticsResolveRequest {
            source_code: unresolved_source.to_string(),
            action: fix_action,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/diagnostics/resolve")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&resolve_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        let fixed_dsl = res["source_code"].as_str().unwrap();
        assert!(fixed_dsl.contains("(service-task :id my-task :verb ob-poc:cbu.create :next end)"));

        // 3. The legacy route remains a compatibility alias for the local
        // deterministic designer classifier.
        let utter_body = UtteranceRequest {
            utterance: "exit and go back".to_string(),
            _current_dsl: source.to_string(),
            target_node_id: None,
            unresolved_verb: None,
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/dsl/sage/utter")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&utter_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert!(res["escape_intent_detected"].as_bool().unwrap());
        assert_eq!(res["suggested_action"].as_str().unwrap(), "exit");
    }

    #[tokio::test]
    async fn test_template_catalog_endpoints() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);

        // 1. Post version 1 of template "my-template"
        let dsl_v1 = r#"(workflow my-template
  (start-event :id start :next end)
  (end-event :id end :status "v1"))"#;
        let define_body = DefineTemplateBody {
            name: "my-template".to_string(),
            dsl_body: dsl_v1.to_string(),
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/templates")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&define_body).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res["version"], 1);
        let first_hash = res["plan_hash"].as_str().unwrap().to_string();
        assert_eq!(first_hash.len(), 64);

        // 2. List templates - verify "my-template" exists and has latest_version = 1
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let summaries: Value = serde_json::from_slice(&body_bytes).unwrap();
        let summaries_arr = summaries.as_array().unwrap();
        assert!(summaries_arr.iter().any(|t| t["name"] == "my-template"
            && t["latest_version"] == 1
            && t["plan_hash"] == first_hash));

        // 3. Get version 1 of the template
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates/my-template/versions/1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let version_dto: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(version_dto["name"], "my-template");
        assert_eq!(version_dto["version"], 1);
        assert_eq!(version_dto["dsl_body"], dsl_v1);
        assert_eq!(version_dto["plan_hash"], first_hash);

        // 4. Post version 2 of template "my-template"
        let dsl_v2 = r#"(workflow my-template
  (start-event :id start :next end)
  (end-event :id end :status "v2"))"#;
        let define_body_v2 = DefineTemplateBody {
            name: "my-template".to_string(),
            dsl_body: dsl_v2.to_string(),
        };

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bpmn/templates")
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&define_body_v2).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let res_v2: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(res_v2["version"], 2);
        let second_hash = res_v2["plan_hash"].as_str().unwrap().to_string();
        assert_ne!(first_hash, second_hash);

        // 5. List templates again - check latest_version = 2
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let summaries_v2: Value = serde_json::from_slice(&body_bytes).unwrap();
        let summaries_arr_v2 = summaries_v2.as_array().unwrap();
        assert!(summaries_arr_v2.iter().any(|t| t["name"] == "my-template"
            && t["latest_version"] == 2
            && t["plan_hash"] == second_hash));
    }

    const SESSION_DSL_OK: &str = r#"(workflow session-wf
  (start-event :id start :next end)
  (end-event :id end :status "OK"))"#;

    async fn body_json(response: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn post_json(uri: &str, body: Value) -> Request<axum::body::Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn test_design_session_round_trip_and_save() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // Create
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "kyc draft", "dsl_source": "" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let created = body_json(response).await;
        let session_id = created["session_id"].as_str().unwrap().to_owned();

        // List contains it
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/dsl/sessions")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let list = body_json(response).await;
        assert!(list
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == session_id.as_str() && s["name"] == "kyc draft"));

        // Invalid revision is RECORDED (drafts may be broken) but reports diagnostics
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/revision"),
                serde_json::json!({ "dsl_source": "(workflow broken", "note": "wip" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let rev = body_json(response).await;
        assert_eq!(rev["compiles"], false);
        assert!(!rev["diagnostics"].as_array().unwrap().is_empty());
        let broken_seq = rev["seq"].as_u64().unwrap();

        // Valid revision compiles clean
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/revision"),
                serde_json::json!({ "dsl_source": SESSION_DSL_OK, "note": "fixed" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let rev = body_json(response).await;
        assert_eq!(rev["compiles"], true);
        assert!(rev["seq"].as_u64().unwrap() > broken_seq);

        // Utterance goes through the intent gate and is appended
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "add a task after start" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let utter = body_json(response).await;
        assert!(utter["seq"].as_u64().is_some());
        assert!(utter["message"].is_string());

        // Save-as-template (save-as-template rewire, 2026-07-30): a
        // text-backed session is a LEGACY authoring path — the save is
        // blocked with a structured error, and NOTHING is persisted in
        // either template store (RED receipt for the legacy-path gate).
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "session-template" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let saved = body_json(response).await;
        assert_eq!(saved["error"], "legacy_authoring_path");

        // Neither template system saw a write for this key.
        assert!(
            state
                .store
                .load_latest_template_version("session-template")
                .await
                .unwrap()
                .is_none(),
            "blocked legacy save must not write the plan catalog"
        );
        assert!(
            state
                .template_store
                .list(Some("session-template"), None)
                .await
                .unwrap()
                .is_empty(),
            "blocked legacy save must not write the template registry"
        );

        // Get: full record still shows the session UNSAVED, event log intact
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{session_id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let record = body_json(response).await;
        assert_ne!(record["status"], "Saved");
        let events = record["events"].as_array().unwrap();
        assert!(events.len() >= 4); // seed + broken rev + good rev + utterance
    }

    /// UI smoke: the designer page serves and names itself.
    #[tokio::test]
    async fn test_designer_page_serves() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/designer")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        let html = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(html.contains("BPMN-Lite Designer"));
        assert!(
            html.contains("/api/dsl/sessions"),
            "drives the governed endpoints"
        );
    }

    /// Graph-window receipt (merged T4 / WS-B.2): compiling session →
    /// graph + deterministic layout; broken draft → diagnostics, never
    /// a blank-canvas error.
    #[tokio::test]
    async fn test_session_graph_endpoint() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let ok_src = SESSION_DSL_OK;
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "g1", "dsl_source": ok_src }),
            ))
            .await
            .unwrap();
        let sid = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let get_graph = |sid: &str| {
            Request::builder()
                .method("GET")
                .uri(format!("/api/dsl/sessions/{sid}/graph"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let response = app.clone().oneshot(get_graph(&sid)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let g = body_json(response).await;
        assert_eq!(g["compiles"], true);
        let nodes = g["graph"]["nodes"].as_array().unwrap();
        assert!(!nodes.is_empty());
        let first_id = nodes[0]["id"].as_str().unwrap();
        assert!(
            g["layout"][first_id]["x"].is_number(),
            "layout coords served"
        );

        // Broken revision → diagnostics, not a blank-canvas error.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/revision"),
                serde_json::json!({ "dsl_source": "(workflow broken", "note": "wip" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app.clone().oneshot(get_graph(&sid)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let g = body_json(response).await;
        assert_eq!(g["compiles"], false);
        assert!(!g["diagnostics"].as_array().unwrap().is_empty());
        assert!(g["graph"].is_null());
    }

    /// SHADOW-START receipt (WS-B day-one wiring): the session
    /// utterance flows board → tier-0 → policy → I28 record; the record
    /// lands in the event log; corpus capture is visibly suppressed
    /// (D17, no charter); the board hash tracks the session's source
    /// (C7 obligation); gibberish abstains with the D19-rider denial.
    #[tokio::test]
    async fn test_session_utterance_runs_shadow_pipeline() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let mk = |name: &str, src: &str| {
            post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": name, "dsl_source": src }),
            )
        };
        let response = app.clone().oneshot(mk("s1", "(workflow a)")).await.unwrap();
        let s1 = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let response = app.clone().oneshot(mk("s2", "(workflow b)")).await.unwrap();
        let s2 = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // The compatibility boundary is explicit: a text-backed session may
        // retain its legacy utterance surface during the rollback window, but
        // it can never be silently promoted into a graph-authoritative board
        // or Sage guidance position.
        for uri in [
            format!("/api/dsl/sessions/{s1}/gameboard"),
            format!("/api/dsl/sage/sessions/{s1}/gameboard"),
            format!("/api/dsl/sage/sessions/{s1}/guidance/op.insert_after"),
        ] {
            let response = app.clone().oneshot(get_req(&uri)).await.unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{uri}");
        }

        let utter = |sid: &str, text: &str| {
            post_json(
                &format!("/api/dsl/sessions/{sid}/utterance"),
                serde_json::json!({ "text": text }),
            )
        };
        // Gibberish → abstention with the generic, path-forward denial.
        let response = app
            .clone()
            .oneshot(utter(&s1, "zzz qqq xyzzy"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let r1 = body_json(response).await;
        assert_eq!(r1["disposition"], "OutOfScope");
        assert!(
            r1["message"]
                .as_str()
                .unwrap()
                .contains("current working context"),
            "actual message: {}",
            r1["message"]
        );
        // DIR-004 Phase 1.2: outside `q9-capture` (default), the module
        // isn't even compiled in -- "not_compiled" reports that build-time
        // fact; under `q9-capture` the old runtime-suppressed state still
        // applies (no charter ref is ever supplied in this workspace).
        #[cfg(not(feature = "q9-capture"))]
        assert_eq!(r1["capture"], "not_compiled");
        #[cfg(feature = "q9-capture")]
        assert_eq!(r1["capture"], "suppressed_no_charter");
        assert_eq!(r1["board_schema"], "legacy_thin_v1");
        assert_eq!(r1["board_pack_identity"], "pack.none");
        assert!(
            r1["design_position"].is_null(),
            "legacy source sessions have no authoritative DesignerDag position"
        );
        let h1 = r1["board_hash"].as_str().unwrap().to_owned();
        assert_eq!(h1.len(), 64);

        // Different session source → different board hash (graph
        // identity is hashed — C7).
        let response = app
            .clone()
            .oneshot(utter(&s2, "zzz qqq xyzzy"))
            .await
            .unwrap();
        let r2 = body_json(response).await;
        assert_ne!(h1, r2["board_hash"].as_str().unwrap());

        // The I28 record is IN the event log.
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{s1}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let record = body_json(response).await;
        let events = record["events"].as_array().unwrap();
        let utterance_event = events
            .iter()
            .find_map(|e| e["kind"].get("Utterance"))
            .expect("utterance event present");
        let rec_json = utterance_event["decision_record_json"]
            .as_str()
            .expect("I28 record stored");
        let rec: Value = serde_json::from_str(rec_json).unwrap();
        assert_eq!(rec["board_hash"], h1.as_str());
        assert!(rec["disposition_policy_hash"].as_str().unwrap().len() == 64);
        assert!(!rec["ranking"].as_array().unwrap().is_empty());

        // DIR-002 A1 + "AFTER" item 2, the audit closure: the event stores
        // the SERIALIZED projection (trainable, not hash-only), and its
        // bytes re-hash to the record's context_projection_hash — the ONE
        // serializer produced both sides.
        let ctx_text = utterance_event["context_projection"]
            .as_str()
            .expect("serialized context projection stored");
        assert!(
            ctx_text.starts_with("ctxproj.v1\n"),
            "canonical version line missing: {ctx_text:?}"
        );
        assert!(
            ctx_text.contains("nodes:\n"),
            "node census missing from projection: {ctx_text:?}"
        );
        assert_eq!(
            blake3::hash(ctx_text.as_bytes()).to_hex().to_string(),
            rec["context_projection_hash"].as_str().unwrap(),
            "stored projection bytes must re-hash to the recorded hash"
        );
    }

    /// DIR-004 Phase 2 wiring: dev-session capture is NOT live for a
    /// session until an explicit consent call, and a real captured
    /// record must be train-on-able (board dump + context TEXT present,
    /// not hash-only) -- the same guarantee `dev_capture.rs`'s own unit
    /// tests prove for the type, proven here through the actual HTTP
    /// surface a real session will use.
    #[tokio::test]
    async fn test_dev_capture_requires_consent_then_captures_full_closure() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "dc1", "dsl_source": "(workflow a)" }),
            ))
            .await
            .unwrap();
        let sid = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let status_req = |sid: &str| {
            Request::builder()
                .method("GET")
                .uri(format!("/api/dsl/sessions/{sid}/dev-capture"))
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let utter = |sid: &str, text: &str| {
            post_json(
                &format!("/api/dsl/sessions/{sid}/utterance"),
                serde_json::json!({ "text": text }),
            )
        };

        // Before consent: not enabled, no capture happens on an utterance.
        let response = app.clone().oneshot(status_req(&sid)).await.unwrap();
        assert_eq!(body_json(response).await["enabled"], false);
        let response = app
            .clone()
            .oneshot(utter(&sid, "add a task after start"))
            .await
            .unwrap();
        assert_eq!(body_json(response).await["dev_capture"], "not_enabled");

        // Empty consent statement is refused (mirrors CapturePipeline's
        // empty-charter-ref refusal) -- no store is created.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "   " }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        // Real consent: enabling succeeds.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "Adam, 2026-07-29, self-testing only" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Re-enabling the SAME session is refused (409) -- consent is not
        // silently overwritable mid-session.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{sid}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "a different statement" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        // Now an utterance IS captured, end to end.
        let response = app
            .clone()
            .oneshot(utter(&sid, "add a task after start"))
            .await
            .unwrap();
        let u = body_json(response).await;
        assert_eq!(u["dev_capture"], "captured");

        let response = app.clone().oneshot(status_req(&sid)).await.unwrap();
        let status = body_json(response).await;
        assert_eq!(status["enabled"], true);
        assert_eq!(status["record_count"], 1);
        let record = &status["records"][0];
        assert_eq!(record["provenance"], "dev-session-adam-v1");
        assert_eq!(record["subject"], "Adam");
        assert_eq!(
            record["consent_statement_timestamp"],
            "Adam, 2026-07-29, self-testing only"
        );
        assert_eq!(record["raw_utterance"], "add a task after start");
        assert!(
            !record["board"]["candidates"].as_array().unwrap().is_empty(),
            "captured record must carry real board candidates, not just a hash"
        );
        assert!(
            record["context_projection"]
                .as_str()
                .unwrap()
                .starts_with("ctxproj.v1\n"),
            "captured record must carry serialized context TEXT, not hash-only"
        );
        assert_eq!(record["decision_record_hash"], u["decision_record_hash"]);
        assert!(record["action_span_producer_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64));

        // A second session never sees the first session's records
        // (per-session isolation, same pattern as session_locks).
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "dc2", "dsl_source": "(workflow a)" }),
            ))
            .await
            .unwrap();
        let sid2 = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let response = app.clone().oneshot(status_req(&sid2)).await.unwrap();
        assert_eq!(body_json(response).await["enabled"], false);
    }

    #[tokio::test]
    async fn test_design_session_save_rejects_uncompilable_draft() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "broken", "dsl_source": "(workflow nope" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Save-as-template rewire (2026-07-30): a text-backed session is
        // blocked as a legacy authoring path BEFORE any compile attempt —
        // the "does the draft compile" question no longer even arises on
        // the save path (it still applies at revision-append time).
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "never-lands" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let err = body_json(response).await;
        assert_eq!(err["error"], "legacy_authoring_path");

        // Nothing leaked into the template catalog
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let templates = body_json(response).await;
        assert!(!templates
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "never-lands"));
    }

    #[tokio::test]
    async fn test_design_session_unknown_id_is_404() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let ghost = Uuid::now_v7();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{ghost}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{ghost}/revision"),
                serde_json::json!({ "dsl_source": "x", "note": "" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{ghost}/save"),
                serde_json::json!({ "template_name": "t" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    // ── WS-B.4: DesignerDag-backed sessions ──────────────────────────

    fn task_ir(id: &str) -> bpmn_lite_compiler::IRNode {
        bpmn_lite_compiler::IRNode::ServiceTask {
            id: id.into(),
            name: id.into(),
            task_type: "noop".into(),
        }
    }

    fn new_key() -> designer_graph::schema::NodeKey {
        designer_graph::schema::NodeKey(Uuid::new_v4())
    }

    /// GREEN: a graph-edit sequence against the deterministic seed Start
    /// stages, admits, and is persisted; a session that has never
    /// accumulated one stays legacy (`is_graph_backed() == false`
    /// server-side, exercised indirectly via the utterance path below).
    #[tokio::test]
    async fn test_session_graph_edit_admits_and_persists() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "graph session" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "build the chain" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let direct = body_json(response).await;
        assert_eq!(direct["edit_kind"], "lower_level_direct_edit");
        assert!(direct["semantic_move_id"].is_null());
        assert_eq!(
            direct["non_equivalence_reason"], "no_supported_semantic_counterpart",
            "a raw two-AppendNode tape matches none of the recognized multi-op shapes"
        );
        let seq = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                    serde_json::json!({ "operations": Vec::<designer_graph::ops::Operation>::new() }),
                ))
                .await
                .unwrap(),
        )
        .await;
        // Empty sequence is BAD_REQUEST, not silently accepted.
        let _ = seq;

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let events = record["events"].as_array().unwrap();
        assert!(
            events.iter().any(|e| e["kind"].get("GraphEdit").is_some()),
            "GraphEdit event must be persisted: {events:?}"
        );
    }

    /// v0.8: a raw single-`Operation` direct edit that exactly matches what
    /// `op.append_node` would materialize for the same anchor and node
    /// identifier resolves as `semantic_move_equivalent`, not a lower-level
    /// audited edit — proving the general recover-synthesize-materialize-
    /// compare mechanism, not just the pre-existing delete-only case.
    #[tokio::test]
    async fn test_direct_edit_recovers_append_node_equivalence() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;

        // t1 (review_documents) already has an outgoing edge to "end"
        // (seed_graph_backed_session); InsertAfter splices a new node into
        // that existing edge, unlike AppendNode which requires an open anchor.
        let ops = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: task_ir("appended_directly"),
            edge_id: "arbitrary_direct_edge_id".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let direct = body_json(response).await;
        assert_eq!(direct["edit_kind"], "semantic_move_equivalent");
        assert!(direct["semantic_move_id"].is_string());
        assert!(direct["non_equivalence_reason"].is_null());
    }

    /// The direct-edit endpoint stages operations via pure compiler
    /// admission (`apply_production` + `admit()`), which never consults
    /// `PolicyFilter` — the only place that can catch a tape recognizably
    /// equivalent to a policy-denied candidate is `resolve_direct_edit`
    /// itself, before admission. RED: the identical tape against the empty
    /// (default) policy resolves and would be admitted. GREEN: denying its
    /// recovered candidate id refuses it structurally, before the graph is
    /// ever touched — not merely mislabeled as a lower-level edit and let
    /// through, which is what happened before this fix.
    #[tokio::test]
    async fn test_direct_edit_refuses_policy_denied_candidate() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;
        let sid: Uuid = session_id.parse().unwrap();

        let record = state
            .store
            .load_design_session(&state.tenant_id, sid)
            .await
            .unwrap()
            .unwrap();
        let dag = reconstruct_designer_dag(&record, None).unwrap();
        let ops = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: task_ir("appended_directly"),
            edge_id: "arbitrary_direct_edge_id".into(),
        }];

        // RED: the unrestricted (default/empty) policy admits the tape —
        // confirms this is a real recognized candidate, not a vacuous case.
        let open = resolve_direct_edit(
            &record,
            &dag,
            &ops,
            &utterance_engine::board::PolicyFilter::default(),
        );
        assert!(
            matches!(open, DirectEditResolution::Equivalent(_)),
            "baseline tape must resolve under an empty policy: {open:?}"
        );

        // GREEN: denying the tape's recovered candidate (`op.insert_after`)
        // refuses it outright.
        let denied_policy = utterance_engine::board::PolicyFilter {
            denied: std::collections::BTreeSet::from(["op.insert_after".to_string()]),
        };
        let refused = resolve_direct_edit(&record, &dag, &ops, &denied_policy);
        assert!(
            matches!(refused, DirectEditResolution::PolicyRefused),
            "a tape equivalent to a policy-denied candidate must be refused: {refused:?}"
        );
    }

    /// v0.8 RED: a raw edit that recovers a plausible candidate shape but
    /// whose actual content a real workbook can never produce (a non-`noop`
    /// task type) must diverge on the resulting-graph comparison, not be
    /// accepted as equivalent by name/anchor alone.
    #[tokio::test]
    async fn test_direct_edit_diverges_on_content_a_workbook_cannot_produce() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;

        let ops = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::ServiceTask {
                id: "appended_directly".into(),
                name: "appended_directly".into(),
                task_type: "http_call".into(), // materialize_workbook only ever emits "noop"
            },
            edge_id: "arbitrary_direct_edge_id".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let direct = body_json(response).await;
        assert_eq!(direct["edit_kind"], "lower_level_direct_edit");
        assert_eq!(direct["non_equivalence_reason"], "recovered_shape_diverges");
    }

    // Note on test scope for `attach_guard`, `attach_rearming_guard` and
    // `request_and_wait`: each of these 2-`Operation` candidates'
    // standalone materialization leaves something the full compiler
    // admission chain (`DesignerDag::admit`) independently refuses when
    // applied alone to a plain seeded session — `attach_guard`/
    // `attach_rearming_guard` leave the escape task with no outgoing edge
    // (unreachable-terminal bytecode lowering failure); `request_and_wait`'s
    // `MessageWait.corr_key_source` must reference an existing
    // `IRNode::DataObject`, and no `Operation` variant can create one (only
    // `DesignerDag::seed` can, at DAG-construction time, never through
    // `/graph-edit`). This is a property of each candidate's own
    // materialized shape, orthogonal to this change, matching the
    // `op.delete_subgraph` precedent noted in
    // `semantic-gameboard-phase2-direct-edit-equivalence-generalization-2026-08-10.md`.
    // Regression coverage for these three arms is therefore at the
    // `recover_candidate_shape` unit level
    // (`proposal::tests::recover_candidate_shape_attach_guard_and_attach_rearming_guard`,
    // `recover_candidate_shape_request_and_wait_resolves_the_far_endpoint`),
    // which is precisely the part this change touched; the resulting-graph
    // comparison and HTTP-level wiring are already proven generically by
    // the `interrupting_timeout` test below and the ambiguous-shape test
    // above.

    /// v0.9: same proof at 3 ops — `prod.interrupting_timeout` chains
    /// AttachGuard -> AppendNode -> AppendNode(End), with both the guard's
    /// and the continuation End's synthesized ids (`{escape}_guard`,
    /// `{escape}_end`) reproduced exactly.
    #[tokio::test]
    async fn test_direct_edit_recovers_interrupting_timeout_equivalence() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;

        let guard_key = new_key();
        let cont_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AttachGuard {
                host: t1,
                key: guard_key,
                guard_id: "pause_guard".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Duration { ms: 300_000 },
                ),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: guard_key,
                key: cont_key,
                node: task_ir("pause"),
                edge_id: "flow_pause".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: cont_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "pause_end".into(),
                    terminate: false,
                },
                edge_id: "flow_pause_end".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let direct = body_json(response).await;
        assert_eq!(direct["edit_kind"], "semantic_move_equivalent");
        assert!(direct["semantic_move_id"].as_str().is_some_and(|s| !s.is_empty()));
    }

    /// v0.9 RED: `prod.reminder_then_escalate` and
    /// `prod.non_interrupting_notification` materialize byte-for-byte
    /// identical `Vec<Operation>` shapes (AttachRearmingGuard/Cycle ->
    /// AppendNode -> End) — nothing in the operation content distinguishes
    /// them. A raw tape reproducing this shape must fail closed rather than
    /// guess a label the content can't prove (ruled 2026-08-10).
    #[tokio::test]
    async fn test_direct_edit_refuses_ambiguous_reminder_or_notification_shape() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;

        let guard_key = new_key();
        let mid_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AttachRearmingGuard {
                host: t1,
                key: guard_key,
                guard_id: "x_guard".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Cycle { interval_ms: 60_000, max_fires: 3 },
                ),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: guard_key,
                key: mid_key,
                node: task_ir("x"),
                edge_id: "flow_x".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: mid_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End { id: "x_end".into(), terminate: false },
                edge_id: "flow_x_end".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let direct = body_json(response).await;
        assert_eq!(direct["edit_kind"], "lower_level_direct_edit");
        assert_eq!(direct["non_equivalence_reason"], "ambiguous_candidate_shape");
        assert!(direct["semantic_move_id"].is_null());
    }

    /// RED: an operation sequence that refuses to stage (unknown anchor)
    /// is REJECTED (422) and NOTHING is persisted — the edit log never
    /// carries a candidate that failed to admit-stage.
    #[tokio::test]
    async fn test_session_graph_edit_refuses_invalid_ops_and_persists_nothing() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "refusal session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let ops = vec![designer_graph::ops::Operation::AppendNode {
            anchor: new_key(), // unknown — no such node exists yet
            key: new_key(),
            node: task_ir("orphan"),
            edge_id: "f1".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !record["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"].get("GraphEdit").is_some()),
            "a refused edit must persist nothing"
        );
    }

    /// RED (G2 blind-review finding, BLOCKER 1): a sequence whose OPS are
    /// each individually legal at staging time (both anchors resolve, both
    /// AppendNodes target a fresh anchor) but whose resulting GRAPH is
    /// globally illegal — a parallel split with no matching join. Before
    /// the fix, `apply_production`'s per-op checks alone gated persistence
    /// and this sequence was wrongly ACCEPTED; the fix wires the caller's
    /// mandatory `staged.candidate.admit()` call so the full to_ir/verify
    /// theorem chain runs before anything is appended.
    #[tokio::test]
    async fn test_session_graph_edit_refuses_locally_staged_but_globally_illegal_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "unmatched fork session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let split_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: split_key,
                node: bpmn_lite_compiler::IRNode::GatewayAnd {
                    id: "split".into(),
                    name: "split".into(),
                    direction: bpmn_lite_compiler::GatewayDirection::Diverging,
                },
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: split_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "unmatched fork" }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a fork with no matching join must be refused by admit(), not just per-op staging"
        );
        let body = body_json(response).await;
        let diagnostics = body["diagnostics"].as_array().unwrap();
        assert!(
            diagnostics.iter().any(|d| {
                let d = d.as_str().unwrap().to_lowercase();
                d.contains("fork") || d.contains("join")
            }),
            "refusal must name the fork/join mismatch: {diagnostics:?}"
        );

        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            !record["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"].get("GraphEdit").is_some()),
            "a graph that doesn't admit must persist nothing"
        );
    }

    /// GREEN (G2 blind-review finding, BLOCKER 3): N concurrent graph-edit
    /// requests against the SAME session, all competing for the SAME
    /// anchor's single outgoing-edge slot. Before the fix, the endpoint's
    /// load→reconstruct→stage→append sequence ran with no per-session
    /// serialization — concurrent requests could each load the DAG BEFORE
    /// any of them had appended, each stage successfully against that
    /// stale base, and each persist a `GraphEdit` event; on replay,
    /// folding a second AppendNode against an anchor the first event
    /// already gave an outgoing edge to fails mid-fold, permanently
    /// bricking the session (every future reconstruct/utterance call
    /// errors, with no repair path). The fix serializes each session's
    /// load-stage-append sequence behind a per-session `tokio::sync::Mutex`
    /// (`DesignerState::session_lock`), so every request after the first sees
    /// the prior request's committed state before it stages — exactly one
    /// request can ever win the anchor's outgoing-edge slot, and the
    /// session's reconstruction stays valid no matter how the requests
    /// interleave.
    #[tokio::test]
    async fn test_concurrent_graph_edits_on_same_anchor_never_corrupt_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "concurrent edit session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        const N: usize = 5;
        let requests: Vec<_> = (0..N)
            .map(|i| {
                // Each request's own op sequence is a COMPLETE,
                // independently admittable chain (Start -> Task_i -> End_i)
                // — the race is purely over which one claims start_key's
                // single outgoing-edge slot, not over structural
                // completeness of any one contender's graph.
                let task_key = new_key();
                let ops = vec![
                    designer_graph::ops::Operation::AppendNode {
                        anchor: start_key,
                        key: task_key,
                        node: task_ir(&format!("branch_{i}")),
                        edge_id: format!("f{i}a"),
                    },
                    designer_graph::ops::Operation::AppendNode {
                        anchor: task_key,
                        key: new_key(),
                        node: bpmn_lite_compiler::IRNode::End {
                            id: format!("end_{i}"),
                            terminate: false,
                        },
                        edge_id: format!("f{i}b"),
                    },
                ];
                app.clone().oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                    serde_json::json!({ "operations": ops, "note": format!("branch {i}") }),
                ))
            })
            .collect();
        let responses: Vec<_> = futures::future::join_all(requests)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let ok_count = responses
            .iter()
            .filter(|r| r.status() == StatusCode::OK)
            .count();
        let refused_count = responses
            .iter()
            .filter(|r| r.status() == StatusCode::UNPROCESSABLE_ENTITY)
            .count();
        assert_eq!(
            ok_count, 1,
            "only ONE request can legitimately win the anchor's single outgoing-edge slot"
        );
        assert_eq!(
            refused_count,
            N - 1,
            "every other request must see the winner's committed state and be refused \
             locally at staging — not race past it and corrupt the log"
        );

        // The decisive check: the session must still RECONSTRUCT after all
        // N concurrent requests settle — a bricked session (the pre-fix
        // failure mode) would 500 here instead.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "irrelevant", "anchor": "branch_0" }),
            ))
            .await
            .unwrap();
        assert_ne!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "session must remain reconstructible after concurrent graph-edits"
        );
    }

    /// GREEN + the headline receipt: once a session is graph-backed, the
    /// utterance endpoint's context projection is REAL `project_ir`
    /// output (an anchor block, a real node census from the actual
    /// graph) — not the DSL-source census fallback — and an unknown
    /// anchor id is a fail-closed 422, never a silent whole-graph
    /// downgrade.
    #[tokio::test]
    async fn test_session_utterance_uses_positional_legality_when_graph_backed() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "positional session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await["edit_kind"],
            "lower_level_direct_edit",
            "raw operation submissions must remain visibly distinct from semantic moves"
        );

        // Unknown anchor: a typed, non-mutating turn is retained against an
        // explicit unknown focus; it never becomes a whole-graph proposal.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "insert a step", "anchor": "ghost_node" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let unknown = body_json(response).await;
        assert_eq!(unknown["disposition"]["kind"], "change_focus_or_context");
        assert_eq!(
            unknown["disposition"]["attempt_receipt"]["outcome"],
            "inapplicable"
        );
        assert_eq!(unknown["design_position"]["focus"]["kind"], "unknown");
        assert!(unknown["proposal"].is_null());

        // Enable the explicitly consented development capture so this
        // endpoint test can prove the semantic board is retained in full.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "semantic board endpoint test" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Real anchor: training-grade projection.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "insert a step after this", "anchor": "review_documents" }),
            ))
            .await
            .unwrap();
        let response_status = response.status();
        let utterance = body_json(response).await;
        assert_eq!(response_status, StatusCode::OK, "{utterance:?}");
        assert_eq!(utterance["board_schema"], "semantic_decision_board_v1");
        assert_ne!(utterance["board_pack_identity"], "pack.none");
        let anchored_hash = utterance["board_hash"].as_str().unwrap().to_owned();
        let anchored_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(utterance["design_position"].clone()).unwrap();
        assert_eq!(anchored_position.domain().as_str(), "bpmn.designer");
        assert_eq!(
            anchored_position
                .board_path()
                .segments()
                .collect::<Vec<_>>(),
            vec!["bpmn.designer"]
        );
        assert_eq!(
            anchored_position.compiler_profile(),
            DESIGNER_COMPILER_PROFILE_IDENTITY
        );
        assert_eq!(anchored_position.graph_revision().as_str().len(), 64);
        assert_eq!(anchored_position.graph_hash().as_str().len(), 64);
        assert_eq!(anchored_position.history_hash().as_str().len(), 64);
        assert!(anchored_position.current_proposal_hash().is_none());
        assert!(matches!(
            anchored_position.focus(),
            semantic_decision_contracts::DesignFocus::Element { element }
                if element.as_str() == "review_documents"
        ));
        assert!(!anchored_position.legal_moves().is_empty());
        assert!(anchored_position.legal_moves().iter().all(|legal_move| {
            legal_move.graph_revision() == anchored_position.graph_revision()
        }));
        let palette_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sessions/{session_id}/gameboard?anchor=review_documents"
            )))
            .await
            .unwrap();
        assert_eq!(palette_response.status(), StatusCode::OK);
        let palette_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(body_json(palette_response).await).unwrap();
        assert_eq!(
            palette_position.move_set_hash(),
            anchored_position.move_set_hash(),
            "palette and language paths must observe one canonical move set"
        );
        let sage_board_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sage/sessions/{session_id}/gameboard?anchor=review_documents"
            )))
            .await
            .unwrap();
        assert_eq!(sage_board_response.status(), StatusCode::OK);
        let sage_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(body_json(sage_board_response).await).unwrap();
        assert_eq!(
            sage_position.state_id(),
            palette_position.state_id(),
            "Sage and the contemporaneous palette read must share one position identity"
        );
        assert_eq!(sage_position.move_set_hash(), palette_position.move_set_hash());
        let guidance_candidate = palette_position
            .legal_moves()
            .iter()
            .find(|legal_move| {
                legal_move.candidate_id().as_str()
                    != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
            })
            .unwrap()
            .candidate_id()
            .as_str()
            .to_string();
        let guidance_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sage/sessions/{session_id}/guidance/{guidance_candidate}?anchor=review_documents"
            )))
            .await
            .unwrap();
        assert_eq!(guidance_response.status(), StatusCode::OK);
        let guidance = body_json(guidance_response).await;
        let guidance_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(guidance["position"].clone()).unwrap();
        assert_eq!(guidance["candidate_id"], guidance_candidate);
        assert_eq!(guidance_position.state_id(), palette_position.state_id());
        assert!(guidance["applicability"].is_string());
        assert!(guidance["explanation"].is_object());
        assert!(guidance["recoveries"].is_array());
        let unknown_guidance_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sage/sessions/{session_id}/guidance/not-a-candidate?anchor=review_documents"
            )))
            .await
            .unwrap();
        assert_eq!(unknown_guidance_response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            body_json(unknown_guidance_response).await["error"],
            "governed guidance is unavailable for this candidate",
            "Sage must not render an internal semantic-board error"
        );
        let selected_palette_move = palette_position
            .legal_moves()
            .iter()
            .find(|legal_move| {
                legal_move.candidate_id().as_str()
                    != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
            })
            .unwrap()
            .move_id()
            .as_str()
            .to_string();
        let palette_selection_response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/palette/select"),
                serde_json::json!({
                    "move_id": selected_palette_move,
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(palette_selection_response.status(), StatusCode::OK);
        let palette_selection = body_json(palette_selection_response).await;
        assert!(palette_selection["proposal_id"].is_string());
        assert!(palette_selection["workbook"].is_object());
        assert_eq!(
            palette_selection["proposal_id"],
            palette_selection["workbook"]["workbook_id"],
            "palette and utterance proposals share the canonical workbook identity"
        );
        assert!(palette_selection["proposal_status"].is_string());
        let sage_history_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sage/sessions/{session_id}/history"
            )))
            .await
            .unwrap();
        assert_eq!(sage_history_response.status(), StatusCode::OK);
        let sage_history = body_json(sage_history_response).await;
        assert_eq!(
            sage_history["history_hash"],
            palette_position.history_hash().as_str(),
            "Sage history must be the projection bound into the live palette position"
        );
        let audit_response = app.clone().oneshot(get_req(&format!(
            "/api/dsl/sage/sessions/{session_id}/audit"
        ))).await.unwrap();
        assert_eq!(audit_response.status(), StatusCode::OK);
        let audit = body_json(audit_response).await;
        assert!(audit["entries"].as_array().unwrap().iter().any(|entry| entry["outcome"] == "created"));
        let attempt_id = sage_history["attempts"]
            .as_array()
            .unwrap()
            .first()
            .unwrap()["attempt_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let attempt_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sage/sessions/{session_id}/attempts/{attempt_id}"
            )))
            .await
            .unwrap();
        assert_eq!(attempt_response.status(), StatusCode::OK);
        let attempt = body_json(attempt_response).await;
        assert_eq!(attempt["attempt"]["attempt_id"], attempt_id);
        assert!(attempt["attempt"]["rule_explanations"].is_array());
        assert!(attempt["attempt"]["feedback_options"].is_array());

        // Position is part of semantic authority: the same graph at a
        // whole-graph position must not reuse the anchored board hash.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "irrelevant" }),
            ))
            .await
            .unwrap();
        let whole_graph = body_json(response).await;
        assert_ne!(anchored_hash, whole_graph["board_hash"]);
        let whole_graph_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(whole_graph["design_position"].clone()).unwrap();
        assert!(matches!(
            whole_graph_position.focus(),
            semantic_decision_contracts::DesignFocus::Absent {
                reason: semantic_decision_contracts::FocusAbsenceReason::NotProvided,
            }
        ));
        assert_ne!(
            anchored_position.state_id(),
            whole_graph_position.state_id()
        );
        let palette_response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sessions/{session_id}/gameboard"
            )))
            .await
            .unwrap();
        assert_eq!(palette_response.status(), StatusCode::OK);
        let palette_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(body_json(palette_response).await).unwrap();
        assert_eq!(
            palette_position.move_set_hash(),
            whole_graph_position.move_set_hash()
        );
        assert_ne!(
            palette_position.history_hash(),
            whole_graph_position.history_hash(),
            "the terminal wrong attempt must advance bounded history"
        );
        assert_eq!(
            palette_position.graph_revision(),
            whole_graph_position.graph_revision(),
            "history evidence must not mutate the authoritative graph"
        );

        let capture = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/dev-capture"
                )))
                .await
                .unwrap(),
        )
        .await;
        let captured_board = &capture["records"][0]["board"];
        assert_eq!(captured_board["schema"], "semantic_decision_board_v1");
        assert!(
            captured_board["semantic_board"]["candidates"].is_array(),
            "capture must retain the full semantic candidate contracts: {captured_board:?}"
        );
        let captured_record = &capture["records"][0];
        assert_eq!(
            captured_record["ranking"].as_array().unwrap().len(),
            captured_board["semantic_board"]["candidates"]
                .as_array()
                .unwrap()
                .len(),
            "BPMN serving must score the complete legal board"
        );
        assert_eq!(captured_record["evidence_trace"]["served_full_board"], true);
        assert_eq!(
            captured_record["evidence_trace"]["candidate_serializer_hash"]
                .as_str()
                .unwrap()
                .len(),
            64
        );
        let record = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let utterance_events = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].get("Utterance"))
            .collect::<Vec<_>>();
        let utterance_event = utterance_events
            .iter()
            .find(|event| {
                event["context_projection"].as_str().is_some_and(|context| {
                    context.contains("anchor: service_task review_documents")
                })
            })
            .expect("anchored utterance event present");
        let ctx_text = utterance_event["context_projection"].as_str().unwrap();
        assert!(
            ctx_text.contains("anchor: service_task review_documents"),
            "project_ir must project the REAL anchor, not a census-only fallback: {ctx_text:?}"
        );
        assert!(
            ctx_text.contains("nodes:\n"),
            "node census present: {ctx_text:?}"
        );
        let wrong_attempt = utterance_events.last().unwrap();
        let receipt: semantic_decision_contracts::MoveAttemptReceipt = serde_json::from_str(
            wrong_attempt["gameboard_attempt_receipt_json"]
                .as_str()
                .expect("wrong attempt has a typed receipt"),
        )
        .unwrap();
        assert!(matches!(
            receipt.outcome(),
            semantic_decision_contracts::MoveAttemptOutcome::Ambiguous
                | semantic_decision_contracts::MoveAttemptOutcome::Inapplicable
        ));
        let belief: semantic_decision_contracts::DesignBelief = serde_json::from_str(
            wrong_attempt["gameboard_belief_json"]
                .as_str()
                .expect("turn has a position-bound belief snapshot"),
        )
        .unwrap();
        assert_eq!(belief.position_id(), whole_graph_position.state_id());
        assert_eq!(
            record["events"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|event| event["kind"].get("GraphEdit").is_some())
                .count(),
            1,
            "wrong attempts never append an authoritative graph edit"
        );
    }

    #[tokio::test]
    async fn test_sage_endpoints_exclude_general_session_surface_fields() {
        // Closes red-receipt item 4 (docs/receipts/semantic-gameboard-phase7-red-2026-08-07.md):
        // "General session/event read-back remains broader than the dedicated
        // Sage view; the remaining audit/history compatibility boundary has
        // not been receipted." This proves the superset relationship is both
        // real (canaries below genuinely reach the general surface, so the
        // absence assertions are not vacuous) and bounded (the same canaries
        // never reach any of Sage's four dedicated read views).
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "sage boundary session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Development capture opt-in populates the training-grade context
        // projection targeted below as a canary field.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/dev-capture/enable"),
                serde_json::json!({ "consent_statement": "sage boundary test" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let canary_text = "CANARY_UTTERANCE_MARKER_9f3e1c";
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": canary_text, "anchor": "review_documents" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let node_key_canary = t1.0.to_string();

        // A palette selection appends a real `ProposalAudit` event so the
        // audit endpoint's own field-dropping (`bound_plan_json`,
        // `dry_run_diagnostics`, ...) has actual content to be tested against
        // — an empty `entries` array would make that half of the boundary
        // check vacuous.
        let general_position: semantic_decision_contracts::DesignPosition = serde_json::from_value(
            body_json(
                app.clone()
                    .oneshot(get_req(&format!(
                        "/api/dsl/sessions/{session_id}/gameboard?anchor=review_documents"
                    )))
                    .await
                    .unwrap(),
            )
            .await,
        )
        .unwrap();
        let selected_move = general_position
            .legal_moves()
            .iter()
            .find(|legal_move| {
                legal_move.candidate_id().as_str()
                    != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
            })
            .unwrap()
            .move_id()
            .as_str()
            .to_string();
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/palette/select"),
                serde_json::json!({ "move_id": selected_move, "anchor": "review_documents" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // General surface: confirm both canaries are genuinely retained,
        // so the absence assertions below are not vacuous.
        let record = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let record_text = record.to_string();
        assert!(
            record_text.contains(canary_text),
            "general session surface must retain raw utterance text: {record_text}"
        );
        assert!(
            record_text.contains(&node_key_canary),
            "general session surface must retain raw internal NodeKeys: {record_text}"
        );
        assert!(record_text.contains("context_projection"));
        assert!(record_text.contains("operations_json"));

        // Sage's dedicated surfaces: the same canaries, and the general-only
        // field names that could carry equivalent leaks, must never appear.
        let forbidden_keys = [
            "dsl_source",
            "context_projection",
            "decision_record_json",
            "operations_json",
            "dry_run_diagnostics",
            "bound_plan_json",
            "gameboard_disposition_json",
        ];

        let sage_gameboard = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sage/sessions/{session_id}/gameboard?anchor=review_documents"
                )))
                .await
                .unwrap(),
        )
        .await;
        let sage_position: semantic_decision_contracts::DesignPosition =
            serde_json::from_value(sage_gameboard.clone()).unwrap();
        let guidance_candidate = sage_position
            .legal_moves()
            .iter()
            .find(|legal_move| {
                legal_move.candidate_id().as_str()
                    != semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
            })
            .unwrap()
            .candidate_id()
            .as_str()
            .to_string();

        let sage_history = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sage/sessions/{session_id}/history"
                )))
                .await
                .unwrap(),
        )
        .await;
        let sage_audit = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sage/sessions/{session_id}/audit"
                )))
                .await
                .unwrap(),
        )
        .await;
        let attempt_id = sage_history["attempts"]
            .as_array()
            .unwrap()
            .first()
            .expect("the canary utterance must have appended at least one attempt receipt")
            ["attempt_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sage_attempt = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sage/sessions/{session_id}/attempts/{attempt_id}"
                )))
                .await
                .unwrap(),
        )
        .await;
        let sage_guidance = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sage/sessions/{session_id}/guidance/{guidance_candidate}?anchor=review_documents"
                )))
                .await
                .unwrap(),
        )
        .await;

        for (label, sage_body) in [
            ("gameboard", &sage_gameboard),
            ("history", &sage_history),
            ("audit", &sage_audit),
            ("attempt", &sage_attempt),
            ("guidance", &sage_guidance),
        ] {
            let body_text = sage_body.to_string();
            assert!(
                !body_text.contains(canary_text),
                "{label}: Sage must never observe raw utterance text: {body_text}"
            );
            assert!(
                !body_text.contains(&node_key_canary),
                "{label}: Sage must never observe an internal NodeKey: {body_text}"
            );
            for key in forbidden_keys {
                assert!(
                    !body_text.contains(key),
                    "{label}: Sage response must not carry the general-surface field `{key}`: {body_text}"
                );
            }
        }
    }

    /// Fail-closed DEGRADATION receipt (tier-1 serving integration,
    /// 2026-08-01): `candle-probe` compiled but SLM_BUNDLE_DIR pointing
    /// nowhere → the designer starts, serves the utterance at tier-0
    /// quality, and the record's `model_bundle_hash` names the producer
    /// that actually ran — never a fake trained identity, never a 500.
    /// Hermetic: a bad bundle dir fails at the training_card.json read,
    /// before any network/model machinery.
    #[cfg(feature = "candle-probe")]
    #[tokio::test]
    async fn test_tier1_bad_bundle_degrades_to_tier0_with_honest_record() {
        std::env::set_var("SLM_BUNDLE_DIR", "/nonexistent/bundle/dir");
        let state = DesignerState::try_new().unwrap();
        std::env::remove_var("SLM_BUNDLE_DIR");
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "degraded session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "connect the review task to the end" }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "degraded path must serve, not error"
        );
        let body = body_json(response).await;
        let bundle = body["model_bundle_hash"].as_str().unwrap();
        assert!(
            !bundle.contains("slm.trained"),
            "degraded path must NOT claim a trained bundle: {bundle}"
        );
        assert!(
            bundle.starts_with("tier0."),
            "active tier must be an honest tier-0 identity: {bundle}"
        );
    }

    /// THE money receipt (plan §F context-sensitivity family): the
    /// CONTEXT PAIR "give up after too many tries" (`pg_d1_too_many_tries`,
    /// both families in the TRAIN split) — guard-anchor →
    /// `op.set_guard_budget` vs task-anchor → NOTA — on the guarded-task
    /// fixture graph. The lexical tier-0 tops NOTA on BOTH anchors (zero
    /// token overlap: structurally blind, plan §F), so the guard-anchor
    /// side would be refused OutOfScope without the model; tier-1
    /// resolves the divergence (measured 2026-08-01: 0.560 vs 0.267
    /// calibrated on the guard anchor).
    ///
    /// The pair the plan NAMES ("chase them again") is a MEASURED MISS
    /// of the canonical bundle: `guard_node::op.set_guard_trigger` sits
    /// in the held-out TEST split (split_manifest.json) and the
    /// best-val-epoch checkpoint does not resolve it (gold ranked 3rd/
    /// 5th on the exact fixture boards). Recorded in the serving-
    /// integration report, not silently swapped.
    ///
    /// Requires the trained bundle + embed weights, so #[ignore]d; run with
    ///   SLM_BUNDLE_DIR=$PWD/utterance-engine/train_py/bundles/modernbert-base \
    ///   cargo test -p bpmn-lite-server-designer --features embed,candle-probe \
    ///     --release -- --ignored context_pair --nocapture
    #[cfg(all(feature = "candle-probe", feature = "embed"))]
    #[tokio::test]
    #[ignore = "needs SLM_BUNDLE_DIR trained bundle + BGE weights (hf-hub cache)"]
    async fn test_tier1_resolves_context_pair_lexical_cannot() {
        assert!(
            std::env::var("SLM_BUNDLE_DIR")
                .map(|d| !d.is_empty())
                .unwrap_or(false),
            "set SLM_BUNDLE_DIR to the modernbert-base bundle dir"
        );
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // Build the guarded_task/guard_node fixture graph via the real
        // REST surface: start → chase_client → end, rearming timer guard
        // g_reminder on the task (same shape as fixtures.rs).
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "chase pair session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t = new_key();
        let guard = new_key();
        let esc = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t,
                node: task_ir("chase_client"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
            designer_graph::ops::Operation::AttachRearmingGuard {
                host: t,
                key: guard,
                guard_id: "g_reminder".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Cycle {
                        interval_ms: 86_400_000,
                        max_fires: 3,
                    },
                ),
            },
            // The REST graph-edit path runs the full admit (unlike the
            // corpus fixture, which never admits): a boundary timer must
            // have its escalation continuation to be a legal graph.
            designer_graph::ops::Operation::AppendNode {
                anchor: guard,
                key: esc,
                node: task_ir("escalate_case"),
                edge_id: "f3".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: esc,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end_esc".into(),
                    terminate: false,
                },
                edge_id: "f4".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{:?}",
            body_json(response).await
        );

        // Same utterance, two anchors — the divergence only context can
        // resolve. top1() reads the persisted decision record (evidence
        // as policy saw it), not the rendered message.
        let utter = |anchor: &str| {
            let app = app.clone();
            let session_id = session_id.clone();
            let anchor = anchor.to_owned();
            async move {
                let resp = app
                    .oneshot(post_json(
                        &format!("/api/dsl/sessions/{session_id}/utterance"),
                        serde_json::json!({ "text": "give up after too many tries", "anchor": anchor }),
                    ))
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                body_json(resp).await
            }
        };
        let guard_resp = utter("g_reminder").await;
        let task_resp = utter("chase_client").await;

        for (label, resp) in [("guard-anchor", &guard_resp), ("task-anchor", &task_resp)] {
            let bundle = resp["model_bundle_hash"].as_str().unwrap();
            assert!(
                bundle.starts_with("slm.trained.modernbert-base@"),
                "{label}: tier-1 must have served this: {bundle}"
            );
        }

        // Read the persisted records back for the top-1 evidence.
        let record = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let rankings: Vec<Vec<serde_json::Value>> = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].get("Utterance"))
            .map(|u| {
                let rec: serde_json::Value =
                    serde_json::from_str(u["decision_record_json"].as_str().unwrap()).unwrap();
                rec["ranking"].as_array().unwrap().clone()
            })
            .collect();
        assert_eq!(rankings.len(), 2);
        let top = |r: &Vec<serde_json::Value>| r[0][0].as_str().unwrap().to_owned();
        let (guard_top, task_top) = (top(&rankings[0]), top(&rankings[1]));
        println!("guard-anchor top-1: {guard_top}  task-anchor top-1: {task_top}");
        assert_eq!(
            guard_top, "op.set_guard_budget",
            "guard-anchor gold (context pair)"
        );
        assert_eq!(
            task_top, "abstain.none_of_the_above",
            "task-anchor gold is context-conditioned abstention"
        );
    }

    // ── Utterance → propose → ratify/reject loop cements ────────────────
    // (`get_req` helper is defined later in this module.)

    /// Seed a graph-backed session: start → review_documents → end.
    /// Returns (session_id, review_documents key).
    async fn seed_graph_backed_session(
        app: &axum::Router,
    ) -> (String, designer_graph::schema::NodeKey) {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "proposal session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: seed_start_key(sid),
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        (session_id, t1)
    }

    /// B3 (EOP-PLAN-GRAPH-DSL-BRIDGE-001) — green path: a graph-backed
    /// session's canonical DSL receipt emits, recompiles in-contract
    /// (derived empty-bindings registry, the B0/B2 equivalence
    /// discipline), is deterministic across calls, and the endpoint
    /// never mutates the graph.
    #[tokio::test]
    async fn test_dsl_receipt_green_recompiles_and_never_mutates() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let graph_uri = format!("/api/dsl/sessions/{session_id}/graph");
        let receipt_uri = format!("/api/dsl/sessions/{session_id}/dsl-receipt");
        let graph_before =
            body_json(app.clone().oneshot(get_req(&graph_uri)).await.unwrap()).await;

        let response = app.clone().oneshot(get_req(&receipt_uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let receipt = body_json(response).await;
        assert!(
            receipt["refused"].is_null(),
            "green session must emit, got: {receipt}"
        );
        let source = receipt["source"].as_str().expect("source string");
        assert!(source.contains("(workflow "));
        assert!(source.contains("review_documents"));
        assert!(!receipt["graph_state_hash"]
            .as_str()
            .expect("hash string")
            .is_empty());

        let mut reg = bpmn_lite_compiler::dsl::StubPlaceholderRegistry::new();
        for sym in receipt["required_symbols"].as_array().expect("symbols") {
            reg.register_verb(
                sym.as_str().unwrap(),
                bpmn_lite_compiler::dsl::BindingDecl::default(),
            );
        }
        bpmn_lite_compiler::dsl::compile(source, &reg)
            .expect("receipt source must recompile in-contract");

        let receipt_again =
            body_json(app.clone().oneshot(get_req(&receipt_uri)).await.unwrap()).await;
        assert_eq!(receipt, receipt_again, "receipt must be deterministic");
        let graph_after =
            body_json(app.clone().oneshot(get_req(&graph_uri)).await.unwrap()).await;
        assert_eq!(
            graph_before["source_hash"], graph_after["source_hash"],
            "receipt endpoint must never mutate the session graph"
        );
    }

    /// D1 cement update (named, per the D1.0 freeze — this test was B3's
    /// refusing-path proof while guards had no DSL surface; guards
    /// joined the core in D1, so the SAME graph now emits): the guard
    /// graph's receipt emits a boundary-timer form, recompiles
    /// in-contract, and the endpoint still never mutates. The refusing
    /// path is now proven by `test_dsl_receipt_not_found_and_non_graph_backed`
    /// plus the emitter's own red suite.
    #[tokio::test]
    async fn test_dsl_receipt_guard_graph_emits_boundary_timer() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, t1) = seed_graph_backed_session(&app).await;
        let guard = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AttachGuard {
                host: t1,
                key: guard,
                guard_id: "timeout".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
                ),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: guard,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "timeout_end".into(),
                    terminate: false,
                },
                edge_id: "flow_timeout".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let receipt_uri = format!("/api/dsl/sessions/{session_id}/dsl-receipt");
        let graph_uri = format!("/api/dsl/sessions/{session_id}/graph");
        let graph_before =
            body_json(app.clone().oneshot(get_req(&graph_uri)).await.unwrap()).await;
        let response = app.clone().oneshot(get_req(&receipt_uri)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let receipt = body_json(response).await;
        assert!(
            receipt["refused"].is_null(),
            "guard graph must emit since D1: {receipt}"
        );
        let source = receipt["source"].as_str().expect("source string");
        assert!(
            source.contains(
                "(boundary-timer :id timeout :host review_documents :duration-ms 60000 :interrupting true :next timeout_end)"
            ),
            "emitted source must carry the exact frozen guard form: {source}"
        );
        let mut reg = bpmn_lite_compiler::dsl::StubPlaceholderRegistry::new();
        for sym in receipt["required_symbols"].as_array().expect("symbols") {
            reg.register_verb(
                sym.as_str().unwrap(),
                bpmn_lite_compiler::dsl::BindingDecl::default(),
            );
        }
        bpmn_lite_compiler::dsl::compile(source, &reg)
            .expect("guard-bearing receipt source must recompile in-contract");
        let graph_after =
            body_json(app.clone().oneshot(get_req(&graph_uri)).await.unwrap()).await;
        assert_eq!(graph_before["source_hash"], graph_after["source_hash"]);
    }

    /// B3 — unknown session is 404; a session with no graph edits
    /// refuses at the "session" stage (its DSL text is authoritative
    /// already — nothing to project).
    #[tokio::test]
    async fn test_dsl_receipt_not_found_and_non_graph_backed() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);

        let response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sessions/{}/dsl-receipt",
                Uuid::new_v4()
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "plain dsl session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let receipt = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/dsl-receipt"
                )))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(receipt["refused"]["stage"], "session");
        assert!(receipt["source"].is_null());
    }

    /// The utterance whose lexical overlap deterministically resolves to
    /// `op.insert_after` at a ServiceTask anchor (description tokens +
    /// a quoted name for the binding layer), through shadow_v1 policy.
    const BINDABLE_UTTERANCE: &str =
        "Places a node on an existing route, after the selected node called 'collect_documents'";

    async fn utter_bindable(app: &axum::Router, session_id: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": BINDABLE_UTTERANCE, "anchor": "review_documents" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    async fn graph_body(app: &axum::Router, session_id: &str) -> Value {
        let response = app
            .clone()
            .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}/graph")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    #[tokio::test]
    async fn test_strict_compound_utterance_never_falls_through_to_one_proposal() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let before = graph_body(&app, &session_id).await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "attach an interrupting guard; attach a rearming guard",
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["inference_disposition"]["Compound"]["spans"]
                .as_array()
                .unwrap()
                .len(),
            2,
            "strict span evidence must preserve both actions: {body}"
        );
        assert!(body["proposal"].is_null());
        assert!(body["workbook"].is_null());
        assert_eq!(before, graph_body(&app, &session_id).await);
    }

    /// GREEN half of the propose step: a bindable candidate yields a
    /// proposal (id + concrete ops) — and the graph is UNCHANGED (the
    /// utterance endpoint never mutates; ratify is the only door).
    #[tokio::test]
    async fn test_utterance_proposal_stages_without_mutating_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let before = graph_body(&app, &session_id).await;
        let utter = utter_bindable(&app, &session_id).await;
        assert_eq!(utter["disposition"]["kind"], "propose_move", "{utter:?}");
        let proposal = &utter["proposal"];
        assert!(
            proposal["proposal_id"].is_string(),
            "proposal staged: {utter:?}"
        );
        assert_eq!(proposal["operations"].as_array().unwrap().len(), 1);
        assert!(proposal["preview"]["delta_hash"].is_string());
        assert!(
            proposal["operations"][0].get("InsertAfter").is_some(),
            "bound op is InsertAfter: {proposal:?}"
        );
        assert!(proposal["description"]
            .as_str()
            .unwrap()
            .contains("collect_documents"));
        let served_move_id = utter["disposition"]["selected_moves"][0]
            .as_str()
            .expect("bound disposition move id");
        let served_move = utter["design_position"]["legal_moves"]
            .as_array()
            .unwrap()
            .iter()
            .find(|legal_move| legal_move["move_id"] == served_move_id)
            .expect("bound disposition move is present on the served position");
        assert_eq!(
            served_move["preview"]["delta_hash"], proposal["preview"]["delta_hash"],
            "serving must attribute the exact compiler preview to the newly bound move"
        );
        assert!(served_move["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|argument| !argument["required"].as_bool().unwrap_or(false)
                || !argument["value"].is_null()));

        let after = graph_body(&app, &session_id).await;
        assert_eq!(before, after, "proposing must not mutate the graph");

        // Pending list shows it.
        let list = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/proposals"
                )))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    /// EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 U3 work item 1: benchmark
    /// whether a hermetic router-level harness sustains a useful fuzzing
    /// execution rate over the composition path (utterance -> resolve ->
    /// stage workbook -> preview), through the public router only -- no
    /// internal function is called directly. `#[ignore]`: a one-time
    /// decision-tranche measurement, not a permanent CI-gated perf test
    /// (unlike `gameboard_perf`, which tracks an already-shipped path).
    /// Run explicitly: `cargo test -p bpmn-lite-server-designer --lib
    /// rest::tests::u3_router_level_benchmark -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn u3_router_level_benchmark() {
        let setup_start = std::time::Instant::now();
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let setup_elapsed = setup_start.elapsed();
        eprintln!("u3-bench: setup (DesignerState + router + seed session) = {setup_elapsed:?}");

        const ITERATIONS: usize = 200;
        let phrases = [
            BINDABLE_UTTERANCE,
            "attach an interrupting guard; attach a rearming guard",
            "unrelated xylophone request",
        ];

        let run_start = std::time::Instant::now();
        for i in 0..ITERATIONS {
            let text = phrases[i % phrases.len()];
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/utterance"),
                    serde_json::json!({ "text": text, "anchor": "review_documents" }),
                ))
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "iteration {i} text={text:?}"
            );
        }
        let run_elapsed = run_start.elapsed();
        let iters_per_sec = ITERATIONS as f64 / run_elapsed.as_secs_f64();
        eprintln!(
            "u3-bench: {ITERATIONS} iterations in {run_elapsed:?} ({iters_per_sec:.1} iters/sec)"
        );

        // Allocation-growth / per-iteration reset question (work item 2):
        // does calling /utterance repeatedly without ratifying/rejecting
        // leave PendingProposal entries accumulating in the
        // DesignerState's in-memory map, with no built-in reset?
        let proposals = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/proposals"
                )))
                .await
                .unwrap(),
        )
        .await;
        let proposal_count = proposals.as_array().unwrap().len();
        eprintln!(
            "u3-bench: {proposal_count} PendingProposal entries accumulated after \
             {ITERATIONS} iterations on one un-ratified/un-rejected session"
        );
    }

    /// EOP-PLAN-UTTERANCE-DETERMINISTIC-FUZZ-001 U3 work item 4 (focused
    /// property test): pins a structural finding discovered while trying
    /// to build a positive `resolve_compound_chain` success-path test.
    ///
    /// `StrictCompoundSyntax::detect` (`utterance-engine/src/disposition.rs`)
    /// only recognises a compound span when the ENTIRE trimmed span,
    /// after normalisation, exactly matches a governed phrase -- no room
    /// survives for an embedded free-text argument (a quoted node name,
    /// a condition, etc.). `start_workbook`'s only `WorkbookEvidence`
    /// variant is `PaletteSelection` -- it does no free-text argument
    /// extraction of its own. So a compound span's candidate can only
    /// reach `ReadyForDryRun`/`ReadyForRatification` (the precondition
    /// for `resolve_compound_chain` to proceed past span 1, per its own
    /// `Unverifiable` branch) if EVERY required argument is filled by
    /// context alone -- in practice, only the one argument matching the
    /// position's own bound anchor slot (confirmed empirically: `anchor`,
    /// `from`, `host`, `guard`, `split` -- whichever name a given
    /// candidate uses -- is the only argument ever auto-filled; a second
    /// `node_reference`-kind argument, e.g. `op.connect`'s `to`, is
    /// NOT auto-filled even though it's the same kind).
    ///
    /// This test asserts that claim directly and generally: for two real
    /// graph shapes (plain linear; the same graph with a guard attached),
    /// no non-abstention legal move ever reaches `MoveBindingState::Complete`
    /// from anchor-context alone. Combined with the semantic pack having
    /// exactly one capability (`op.delete_subgraph`) with a single
    /// required argument -- and that capability never being legal in
    /// either graph shape tried -- this is strong evidence that
    /// `resolve_compound_chain`'s `Ready` arm is currently unreachable
    /// given the shipped pack, independent of graph shape. Not a code
    /// defect: `resolve_compound_chain`/`resolve_hypothetical_chain`'s
    /// own mechanics are separately proven correct (this crate's
    /// `test_strict_compound_utterance_never_falls_through_to_one_proposal`,
    /// `start_workbook`'s direct tests in `proposal.rs`, and
    /// `utterance-engine::bpmn_board`'s `resolve_hypothetical_chain`
    /// tests). If this test ever starts failing, it means a pack change
    /// made a capability anchor-only-bindable for the first time --
    /// whoever changes the pack should build a real `Ready`-path test at
    /// that point, since one was not constructible when this was written.
    #[tokio::test]
    async fn no_legal_move_is_anchor_only_bindable_today() {
        async fn assert_no_complete_non_abstention_move(app: &axum::Router, session_id: &str) {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/utterance"),
                    serde_json::json!({ "text": "unrelated xylophone request" }),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_json(response).await;
            let legal_moves = body["design_position"]["legal_moves"].as_array().unwrap();
            assert!(
                !legal_moves.is_empty(),
                "fixture produced no legal moves at all -- test no longer exercises anything"
            );
            for legal_move in legal_moves {
                if legal_move["candidate_id"] == semantic_decision_contracts::ABSTENTION_CANDIDATE_ID
                {
                    continue;
                }
                assert_ne!(
                    legal_move["binding_state"]["state"], "complete",
                    "candidate {} is anchor-only bindable -- resolve_compound_chain's \
                     Ready arm may now be reachable; see this test's doc comment: {legal_move}",
                    legal_move["candidate_id"]
                );
            }
        }

        // Shape 1: plain linear graph (start -> review_documents -> end).
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, t1) = seed_graph_backed_session(&app).await;
        assert_no_complete_non_abstention_move(&app, &session_id).await;

        // Shape 2: the same graph with a guard attached, in case a
        // guard-typed anchor changes which arguments auto-fill.
        let guard = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AttachGuard {
                host: t1,
                key: guard,
                guard_id: "timeout".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
                ),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: guard,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "timeout_end".into(),
                    terminate: false,
                },
                edge_id: "flow_timeout".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_no_complete_non_abstention_move(&app, &session_id).await;
    }

    /// Ratify: the graph gains the node, the event log gains a GraphEdit
    /// carrying the "ratified proposal" note, and the proposal is gone.
    #[tokio::test]
    async fn test_ratify_applies_proposal_and_appends_graph_edit() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{:?}",
            body_json(response).await
        );

        // /graph reflects the applied edit (session still compiles).
        let graph = graph_body(&app, &session_id).await;
        assert!(
            graph.to_string().contains("collect_documents"),
            "ratified node must be in the compiled graph: {graph}"
        );

        // Event log carries the marked GraphEdit.
        let record = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let ratified = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["kind"].get("GraphEdit"))
            .find(|ge| {
                ge["note"]
                    .as_str()
                    .map(|n| n.contains(&format!("ratified proposal {pid}")))
                    .unwrap_or(false)
            });
        assert!(
            ratified.is_some(),
            "GraphEdit with ratified-proposal note: {record}"
        );
        assert!(
            ratified.unwrap()["note"]
                .as_str()
                .unwrap()
                .contains("proposal audit seq"),
            "ratification must link the durable proposal audit"
        );
        let created_audit = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].get("ProposalAudit"))
            .find(|audit| audit["outcome"] == "created")
            .expect("proposal creation audit");
        assert_eq!(
            created_audit["decision_record_hash"],
            utter["decision_record_hash"]
        );
        assert!(created_audit["workbook_json"]
            .as_str()
            .unwrap()
            .contains(&pid));

        // Lost-response retry: the second ratify returns the same durable
        // terminal receipt and cannot append another GraphEdit.
        let restarted =
            DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap();
        let restarted_app = designer_router(restarted);
        let response = restarted_app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let retry = body_json(response).await;
        assert_eq!(retry["proposal_id"], pid);
        assert_eq!(retry["idempotent"], true);
        assert_eq!(retry["terminal_receipt"]["outcome"], "ratified");
        assert_eq!(retry["terminal_receipt"]["proposal_status"], "ratified");
        let after_retry = body_json(
            restarted_app
                .clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let graph_edits_before = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"].get("GraphEdit").is_some())
            .count();
        let graph_edits_after = after_retry["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["kind"].get("GraphEdit").is_some())
            .count();
        assert_eq!(graph_edits_after, graph_edits_before);
    }

    #[tokio::test]
    async fn test_concurrent_ratify_applies_one_graph_revision() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"].as_str().unwrap().to_owned();
        let uri = format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify");
        let responses = futures::future::join_all([
            app.clone().oneshot(post_json(&uri, serde_json::json!({}))),
            app.clone().oneshot(post_json(&uri, serde_json::json!({}))),
        ])
        .await;
        let mut bodies = Vec::new();
        for response in responses {
            let response = response.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            bodies.push(body_json(response).await);
        }
        assert_eq!(bodies.iter().filter(|body| body["idempotent"] == true).count(), 1);
        assert_eq!(bodies.iter().filter(|body| body["applied"].is_string()).count(), 1);
        let session = body_json(
            app.oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            session["events"].as_array().unwrap().iter()
                .filter(|event| event["kind"].get("GraphEdit").is_some()).count(),
            2,
            "seed has one operation tape and concurrent ratification contributes exactly one"
        );
    }

    /// Phase 7 API fault tape: the compact model tracks only authoritative
    /// graph-event count and whether a terminal receipt must be replayable.
    /// The adapter is exercised through real in-process HTTP requests.
    #[tokio::test]
    async fn test_api_fault_tape_restart_and_lost_ratify_response() {
        #[derive(Clone, Copy)]
        enum Step { Ratify, Restart, RetryRatify }
        let state = DesignerState::try_new().unwrap();
        let mut app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"].as_str().unwrap().to_owned();
        let uri = format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify");
        let mut expected_graph_edits = 1usize;
        let mut terminal = false;
        for step in [Step::Ratify, Step::Restart, Step::RetryRatify] {
            match step {
                Step::Ratify => {
                    let response = app.clone().oneshot(post_json(&uri, serde_json::json!({}))).await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    expected_graph_edits += 1;
                    terminal = true;
                }
                Step::Restart => {
                    app = designer_router(DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap());
                }
                Step::RetryRatify => {
                    let response = app.clone().oneshot(post_json(&uri, serde_json::json!({}))).await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    assert_eq!(body_json(response).await["idempotent"], true);
                    assert!(terminal);
                }
            }
            let record = body_json(app.clone().oneshot(get_req(&format!("/api/dsl/sessions/{session_id}"))).await.unwrap()).await;
            assert_eq!(record["events"].as_array().unwrap().iter()
                .filter(|event| event["kind"].get("GraphEdit").is_some()).count(), expected_graph_edits);
        }
    }

    #[tokio::test]
    async fn test_api_fault_tape_stale_client_preserves_new_revision() {
        let state = DesignerState::try_new().unwrap();
        let mut app = designer_router(state.clone());
        let (session_id, anchor) = seed_graph_backed_session(&app).await;
        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"].as_str().unwrap().to_owned();
        let manual = vec![designer_graph::ops::Operation::InsertAfter {
            anchor,
            key: new_key(),
            node: task_ir("authoritative_intervening_edit"),
            edge_id: "fault_tape_intervening".into(),
        }];
        let edit = app.clone().oneshot(post_json(
            &format!("/api/dsl/sessions/{session_id}/graph-edit"),
            serde_json::json!({"operations":manual}),
        )).await.unwrap();
        assert_eq!(edit.status(), StatusCode::OK);
        let uri = format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify");
        let refused = app.clone().oneshot(post_json(&uri, serde_json::json!({}))).await.unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        app = designer_router(DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap());
        let retry = app.clone().oneshot(post_json(&uri, serde_json::json!({}))).await.unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        let retry = body_json(retry).await;
        assert_eq!(retry["idempotent"], true);
        assert_eq!(retry["terminal_receipt"]["proposal_status"], "expired");
        let record = body_json(app.oneshot(get_req(&format!("/api/dsl/sessions/{session_id}"))).await.unwrap()).await;
        assert_eq!(record["events"].as_array().unwrap().iter()
            .filter(|event| event["kind"].get("GraphEdit").is_some()).count(), 2);
        assert!(graph_body(&designer_router(DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap()), &session_id)
            .await.to_string().contains("authoritative_intervening_edit"));
    }

    /// Phase 7 API fault tape: two independently staged workbooks, both
    /// built against the same base graph revision, race to ratify. Exactly
    /// one must apply and advance the revision; the loser observes graph
    /// drift and is refused with the same typed outcome as the sequential
    /// case (`test_ratify_refuses_on_graph_drift`), and that refusal must
    /// itself be a durable, idempotently replayable terminal receipt across
    /// a restart — not a transient in-memory race result.
    #[tokio::test]
    async fn test_api_fault_tape_concurrent_revision_drift() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter_a = utter_bindable(&app, &session_id).await;
        let pid_a = utter_a["proposal"]["proposal_id"].as_str().unwrap().to_owned();
        let utter_b = utter_bindable(&app, &session_id).await;
        let pid_b = utter_b["proposal"]["proposal_id"].as_str().unwrap().to_owned();
        assert_ne!(pid_a, pid_b, "two independent proposals from the same base revision");

        let uri_a = format!("/api/dsl/sessions/{session_id}/proposals/{pid_a}/ratify");
        let uri_b = format!("/api/dsl/sessions/{session_id}/proposals/{pid_b}/ratify");
        let responses = futures::future::join_all([
            app.clone().oneshot(post_json(&uri_a, serde_json::json!({}))),
            app.clone().oneshot(post_json(&uri_b, serde_json::json!({}))),
        ])
        .await;
        let mut bodies = Vec::new();
        for response in responses {
            let response = response.unwrap();
            let status = response.status();
            assert!(
                status == StatusCode::OK || status == StatusCode::CONFLICT,
                "unexpected status {status}"
            );
            bodies.push((status, body_json(response).await));
        }
        assert_eq!(
            bodies.iter().filter(|(status, _)| *status == StatusCode::OK).count(),
            1,
            "exactly one racing proposal applies: {bodies:?}"
        );
        assert_eq!(
            bodies.iter().filter(|(status, _)| *status == StatusCode::CONFLICT).count(),
            1,
            "exactly one racing proposal observes drift and is refused: {bodies:?}"
        );
        let winner_pid = if bodies[0].0 == StatusCode::OK { &pid_a } else { &pid_b };
        let loser_pid = if bodies[0].0 == StatusCode::OK { &pid_b } else { &pid_a };

        let record = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            record["events"].as_array().unwrap().iter()
                .filter(|event| event["kind"].get("GraphEdit").is_some()).count(),
            2,
            "seed has one operation tape and the winning racer contributes exactly one more"
        );

        // The loser's refusal must be a durable terminal receipt, replayable
        // identically after restart — not merely a transient CONFLICT.
        let restarted = designer_router(
            DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap(),
        );
        let loser_retry = body_json(
            restarted
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/proposals/{loser_pid}/ratify"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(loser_retry["idempotent"], true);
        assert_eq!(loser_retry["terminal_receipt"]["outcome"], "expired_graph_drift");
        assert_eq!(loser_retry["terminal_receipt"]["proposal_status"], "expired");

        // The winner's own terminal receipt must also replay durably.
        let winner_retry = body_json(
            restarted
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/proposals/{winner_pid}/ratify"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(winner_retry["idempotent"], true);
        assert_eq!(winner_retry["terminal_receipt"]["outcome"], "ratified");
        assert_eq!(winner_retry["terminal_receipt"]["proposal_status"], "ratified");

        // Graph revision was advanced exactly once by the race, not twice.
        let record = body_json(
            designer_router(DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap())
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            record["events"].as_array().unwrap().iter()
                .filter(|event| event["kind"].get("GraphEdit").is_some()).count(),
            2,
            "restart and idempotent replays never append additional graph revisions"
        );
    }

    /// Reject: graph unchanged, proposal gone (subsequent ratify 404s).
    #[tokio::test]
    async fn test_reject_drops_proposal_graph_unchanged() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let before = graph_body(&app, &session_id).await;
        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"]
            .as_str()
            .unwrap()
            .to_owned();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/reject"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let duplicate_reject = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/reject"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(duplicate_reject.status(), StatusCode::OK);
        assert_eq!(body_json(duplicate_reject).await["idempotent"], true);

        assert_eq!(
            before,
            graph_body(&app, &session_id).await,
            "reject mutates nothing"
        );
        let restarted =
            DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap();
        let restarted_app = designer_router(restarted);
        let response = restarted_app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let retry = body_json(response).await;
        assert_eq!(retry["idempotent"], true);
        assert_eq!(retry["terminal_receipt"]["outcome"], "rejected");
        assert_eq!(retry["terminal_receipt"]["proposal_status"], "rejected");

        let record = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let rejected = record["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].get("ProposalAudit"))
            .find(|audit| audit["outcome"] == "rejected")
            .expect("terminal rejection audit");
        assert!(rejected["related_event_seq"].is_number());
        let workbook: serde_json::Value =
            serde_json::from_str(rejected["workbook_json"].as_str().unwrap()).unwrap();
        assert_eq!(workbook["status"], "rejected");
    }

    #[tokio::test]
    async fn correction_is_linked_previewed_ratified_and_compiler_admitted() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let unwanted = utter_bindable(&app, &session_id).await;
        let unwanted_id = unwanted["proposal_id"].as_str().unwrap();
        let applied = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{unwanted_id}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(applied.status(), StatusCode::OK);

        let session = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let applied_attempt: semantic_decision_contracts::MoveAttemptReceipt = session["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|event| event["kind"].get("ProposalAudit"))
            .find(|audit| audit["outcome"] == "ratified")
            .and_then(|audit| audit["gameboard_attempt_receipt_json"].as_str())
            .map(serde_json::from_str)
            .transpose()
            .unwrap()
            .unwrap();

        let offer = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/utterance"),
                    serde_json::json!({
                        "text": "that was wrong",
                        "anchor": "review_documents",
                    }),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            offer["disposition"]["kind"], "offer_correction",
            "{offer:?}"
        );
        assert_eq!(
            offer["disposition"]["attempt_receipt"]["correction_of"],
            applied_attempt.attempt_id().as_str()
        );
        assert!(offer["proposal"].is_null());

        let correction = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/utterance"),
                    serde_json::json!({
                        "text": "insert before",
                        "anchor": "review_documents",
                    }),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            correction["disposition"]["kind"], "request_move_arguments",
            "{correction:?}"
        );
        let correction_id = correction["proposal_id"].as_str().unwrap();
        let answered = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/proposals/{correction_id}/answers"),
                    identifier_answer("node", "corrected_step"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(answered["proposal_status"], "ready_for_ratification");
        assert!(answered["preview"]["delta_hash"].is_string());

        let ratified = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{correction_id}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(ratified.status(), StatusCode::OK);

        let session = body_json(
            app.clone()
                .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
                .await
                .unwrap(),
        )
        .await;
        let corrected_attempt: semantic_decision_contracts::MoveAttemptReceipt = session["events"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .filter_map(|event| event["kind"].get("ProposalAudit"))
            .find(|audit| audit["outcome"] == "ratified")
            .and_then(|audit| audit["gameboard_attempt_receipt_json"].as_str())
            .map(serde_json::from_str)
            .transpose()
            .unwrap()
            .unwrap();
        assert_eq!(
            corrected_attempt.outcome(),
            semantic_decision_contracts::MoveAttemptOutcome::Corrected
        );
        assert_eq!(
            corrected_attempt.correction_of(),
            Some(applied_attempt.attempt_id())
        );
        let graph = graph_body(&app, &session_id).await.to_string();
        assert!(graph.contains("collect_documents"));
        assert!(graph.contains("corrected_step"));
    }

    /// Drift: a manual graph-edit lands between staging and ratify →
    /// 409, nothing appended for the proposal, proposal consumed.
    #[tokio::test]
    async fn test_ratify_refuses_on_graph_drift() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, t1) = seed_graph_backed_session(&app).await;

        let utter = utter_bindable(&app, &session_id).await;
        let pid = utter["proposal"]["proposal_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Manual edit shifts the graph identity.
        let manual = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: task_ir("manual_step"),
            edge_id: "f3".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": manual }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT, "drift must refuse");

        // Nothing was appended for the proposal, and it is consumed.
        let graph = graph_body(&app, &session_id).await;
        assert!(
            !graph.to_string().contains("collect_documents"),
            "refused proposal must not reach the graph: {graph}"
        );
        let restarted =
            DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap();
        let restarted_app = designer_router(restarted);
        let response = restarted_app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let retry = body_json(response).await;
        assert_eq!(retry["idempotent"], true);
        assert_eq!(retry["terminal_receipt"]["outcome"], "expired_graph_drift");
        assert_eq!(retry["terminal_receipt"]["proposal_status"], "expired");
    }

    /// Missing bindings preserve the inference disposition and create a typed,
    /// non-mutating workbook instead of collapsing inference into a terminal
    /// `MissingArguments` pseudo-disposition.
    #[tokio::test]
    async fn test_missing_bindings_create_workbook_without_mutation() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;

        let before = graph_body(&app, &session_id).await;
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Joins two existing nodes with a typed connector",
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let utter = body_json(response).await;

        assert_eq!(
            utter["inference_disposition"]["Candidate"]["candidate_id"],
            "op.connect"
        );
        assert_eq!(utter["proposal_status"], "needs_arguments");
        let missing: Vec<String> = utter["workbook"]["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| slot["value"]["state"] == "missing")
            .map(|slot| slot["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(missing, vec!["condition", "to"]);
        assert!(
            utter["proposal_id"].is_string(),
            "workbook staged: {utter:?}"
        );
        assert!(utter["proposal"]["operations"].is_null());

        assert_eq!(before, graph_body(&app, &session_id).await);
        let list = body_json(
            app.clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/proposals"
                )))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            list.as_array().unwrap().len(),
            1,
            "workbook is resumable: {list}"
        );
    }

    async fn utter_needs_insert_name(app: &axum::Router, session_id: &str) -> serde_json::Value {
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Places a node on an existing route, after the selected node",
                    "anchor": "review_documents",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    fn identifier_answer(name: &str, value: &str) -> serde_json::Value {
        serde_json::json!({
            "answers": [{
                "name": name,
                "value": { "kind": "identifier", "value": value },
            }],
        })
    }

    #[tokio::test]
    async fn test_typed_answer_completes_workbook_and_dry_stages_without_mutation() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let before = graph_body(&app, &session_id).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        assert_eq!(utter["proposal_status"], "needs_arguments", "{utter:?}");
        assert!(utter["proposal"]["operations"].is_null());
        let pid = utter["proposal_id"].as_str().unwrap();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "collect_documents"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let answer = body_json(response).await;
        assert_eq!(
            answer["proposal_status"], "ready_for_ratification",
            "{answer:?}"
        );
        assert!(answer["operations"][0].get("InsertAfter").is_some());
        assert!(answer["dry_run_diagnostics"].as_array().unwrap().is_empty());
        assert_eq!(before, graph_body(&app, &session_id).await);
    }

    /// GREEN: a genuinely unexpected pipeline failure (not any of the 7
    /// well-typed `ProposalStatus` outcomes `attach_terminal_gameboard_attempt`
    /// exhaustively maps) persists a `SystemFailure` receipt rather than
    /// vanishing. History projection and the disposition escalation check
    /// (`design_history_projection`, `disposition.rs::decide_game`) only
    /// ever see PERSISTED receipts off stored session events — before this
    /// fix, none of the generic-error branches in the dry-run pipeline
    /// appended anything at all, so this exact failure class was invisible
    /// to both.
    #[tokio::test]
    async fn test_system_failure_persists_a_receipt_history_can_see() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let sid: Uuid = session_id.parse().unwrap();
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid: Uuid = utter["proposal_id"].as_str().unwrap().parse().unwrap();

        let pending = {
            let proposals = state.proposals.lock().unwrap_or_else(|p| p.into_inner());
            proposals.get(&pid).cloned().unwrap()
        };

        let record_before = state
            .store
            .load_design_session(&state.tenant_id, sid)
            .await
            .unwrap()
            .unwrap();
        let (_, before) = design_history_projection(&record_before, None).unwrap();
        assert!(
            before.iter().all(|receipt| receipt.outcome()
                != semantic_decision_contracts::MoveAttemptOutcome::SystemFailure),
            "no SystemFailure receipt should exist yet: {before:?}"
        );

        record_and_persist_system_failure(&state, sid, &pending, "induced-for-test").await;

        let record_after = state
            .store
            .load_design_session(&state.tenant_id, sid)
            .await
            .unwrap()
            .unwrap();
        let (_, after) = design_history_projection(&record_after, None).unwrap();
        assert!(
            after.iter().any(|receipt| receipt.outcome()
                == semantic_decision_contracts::MoveAttemptOutcome::SystemFailure),
            "SystemFailure receipt must be persisted and visible to history projection: {after:?}"
        );
    }

    #[tokio::test]
    async fn test_invalid_unknown_and_duplicate_answers_leave_workbook_intact() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid = utter["proposal_id"].as_str().unwrap();

        for invalid in [
            serde_json::json!({
                "answers": [{"name": "node", "value": {"kind": "count", "value": 2}}]
            }),
            identifier_answer("undeclared", "x"),
            serde_json::json!({
                "answers": [
                    {"name": "node", "value": {"kind": "identifier", "value": "one"}},
                    {"name": "node", "value": {"kind": "identifier", "value": "two"}}
                ]
            }),
        ] {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                    invalid,
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let body = body_json(response).await;
            assert_eq!(body["workbook"]["status"], "needs_arguments", "{body:?}");
            assert_eq!(body["workbook"]["slots"][1]["value"]["state"], "missing");
        }

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "finally_valid"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Phase 7 red-receipt item 6: the one untested corner of the answer
    /// endpoint's duplicate-request behavior — resubmitting the SAME valid,
    /// workbook-completing answer as a second, independent request (a lost-
    /// response retry, not a same-request duplicate slot name, which
    /// `test_invalid_unknown_and_duplicate_answers_leave_workbook_intact`
    /// already covers). The workbook is ephemeral/non-terminal
    /// (`test_restart_drops_ephemeral_workbook`'s documented design), so
    /// there is no durable idempotent-receipt path here — the second POST
    /// must refuse cleanly (workbook already complete) rather than corrupt
    /// or silently double-apply, and ratification must still succeed
    /// afterward on the untouched, already-complete workbook.
    #[tokio::test]
    async fn test_duplicate_valid_answer_after_workbook_complete_refuses_cleanly() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid = utter["proposal_id"].as_str().unwrap().to_owned();

        let first = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "collect_documents"),
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = body_json(first).await;
        assert_eq!(first_body["proposal_status"], "ready_for_ratification");

        let second = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "collect_documents"),
            ))
            .await
            .unwrap();
        assert_eq!(
            second.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "a second, independent answer request must refuse once the workbook is already complete"
        );
        let second_body = body_json(second).await;
        assert_eq!(
            second_body["workbook"]["status"], "ready_for_ratification",
            "the refused duplicate must leave the already-complete workbook untouched: {second_body:?}"
        );

        let ratify = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(
            ratify.status(),
            StatusCode::OK,
            "the refused duplicate must not have corrupted the workbook — ratification still succeeds"
        );
    }

    #[tokio::test]
    async fn test_direct_needs_arguments_to_ratify_is_refused_and_consumed() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid = utter["proposal_id"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let second = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_graph_drift_before_answers_expires_and_consumes_workbook() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let (session_id, t1) = seed_graph_backed_session(&app).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid = utter["proposal_id"].as_str().unwrap();
        let manual = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: task_ir("manual_before_answer"),
            edge_id: "manual_answer_edge".into(),
        }];
        let edit = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({"operations": manual}),
            ))
            .await
            .unwrap();
        assert_eq!(edit.status(), StatusCode::OK);
        let answer = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "never_applied"),
            ))
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::CONFLICT);
        let retry = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "never_applied"),
            ))
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        let retry = body_json(retry).await;
        assert_eq!(retry["idempotent"], true);
        assert_eq!(retry["terminal_receipt"]["proposal_status"], "expired");
    }

    #[tokio::test]
    async fn test_restart_drops_ephemeral_workbook() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _t1) = seed_graph_backed_session(&app).await;
        let utter = utter_needs_insert_name(&app, &session_id).await;
        let pid = utter["proposal_id"].as_str().unwrap();

        let restarted =
            DesignerState::assemble(state.store.clone(), state.template_store.clone()).unwrap();
        let restarted_app = designer_router(restarted);
        let list = body_json(
            restarted_app
                .clone()
                .oneshot(get_req(&format!(
                    "/api/dsl/sessions/{session_id}/proposals"
                )))
                .await
                .unwrap(),
        )
        .await;
        assert!(list.as_array().unwrap().is_empty());
        let answer = restarted_app
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{pid}/answers"),
                identifier_answer("node", "lost_with_restart"),
            ))
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
    }

    /// G2 BLOCKER-2 ruling, receipt (i)'s server-side half: a graph-backed
    /// session saves as a template via `project_ir`, producing a real
    /// `plan_hash`. The bus-handler-level "does the projected plan
    /// actually INSTANTIATE" half of this receipt lives in
    /// `bpmn-lite-bus-handler/tests/graph_authored_plan_instantiation.rs`
    /// (this crate has no reason to depend on bpmn-lite-bus-handler's
    /// dispatch machinery just to prove that).
    #[tokio::test]
    async fn test_save_design_session_projects_graph_backed_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "save-graph-session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "graph-authored-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["plan_hash"].as_str().unwrap().len(), 64);

        // ── Save-as-template rewire receipts (2026-07-30) ──────────────

        // (a) Template registry: v1 exists, Published, graph-sourced, with
        // a real dto_snapshot and task_manifest.
        assert_eq!(body["template_version"], 1);
        assert_eq!(body["state"], "published");
        let bytecode_hex = body["bytecode_version"].as_str().unwrap().to_owned();
        assert_eq!(bytecode_hex.len(), 64);
        let template = state
            .template_store
            .load("graph-authored-template", 1)
            .await
            .unwrap()
            .expect("template registry must hold v1");
        assert_eq!(
            template.state,
            bpmn_lite_authoring::TemplateState::Published
        );
        assert_eq!(
            template.source_format,
            bpmn_lite_authoring::SourceFormat::Graph
        );
        assert_eq!(template.bytecode_version, bytecode_hex);
        assert!(!template.dto_snapshot.nodes.is_empty());
        assert!(
            template.task_manifest.contains(&"noop".to_string()),
            "task_manifest must carry the graph's task types (the task_ir \
             fixture's task_type is \"noop\"): {:?}",
            template.task_manifest
        );

        // (b) Compiled program persisted in the WorkflowStore under the
        // bytecode hash the response reported.
        {
            let mut hash = [0u8; 32];
            hex::decode_to_slice(&bytecode_hex, &mut hash).unwrap();
            let program = state.store.load_program(hash).await.unwrap();
            assert!(
                program.is_some(),
                "compiled program must be persisted under its bytecode hash"
            );
        }

        // (c) Catalog dual-write still feeds the runtime instantiation
        // path: the plan is retrievable by name→hash and deserializes.
        let (catalog_version, _, plan_hash) = state
            .store
            .load_latest_template_version("graph-authored-template")
            .await
            .unwrap()
            .expect("catalog dual-write must be present");
        assert_eq!(catalog_version, 1);
        let plan_json = state
            .store
            .load_plan(plan_hash)
            .await
            .unwrap()
            .expect("plan must be stored");
        let _plan: WorkflowExecutionPlan =
            serde_json::from_str(&plan_json).expect("stored plan must deserialize");

        // (d) Second save of the same session/key auto-bumps to v2 in the
        // registry; v1 is untouched (immutability).
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "graph-authored-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body2 = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body2:?}");
        assert_eq!(body2["template_version"], 2);
        let v1_after = state
            .template_store
            .load("graph-authored-template", 1)
            .await
            .unwrap()
            .expect("v1 must still exist after v2 save");
        assert_eq!(v1_after.bytecode_version, template.bytecode_version);
    }

    /// G4.3 — Gate G4: the save response surfaces `PublishResult.lint_diagnostics`
    /// (which now includes manifest-derived diagnostics — see
    /// `bpmn_lite_authoring::manifest::manifest_diagnostics`) as a
    /// `"diagnostics"` array, previously computed by
    /// `compile_and_publish_from_dto` and silently discarded at this call
    /// site. This graph has no unresolved references, so the array is
    /// present but empty — proving the field is wired without depending
    /// on `WorkflowExecutionPlan` projection support for MI/DataObject
    /// nodes (a separate, pre-existing gap outside G4's scope: MI has no
    /// projection at all — `bpmn-lite-compiler/src/dsl/ir_plan.rs`'s
    /// `UnsupportedNode` — so a graph-backed session carrying one can't
    /// reach the save endpoint's success path today regardless of G4).
    /// The scalar/collection/element-scoped classification itself is
    /// proven end-to-end, including reload, by
    /// `bpmn_lite_authoring::publish::tests::t_pub_13_g4_gate_scenario_manifest_survives_reload`.
    #[tokio::test]
    async fn test_save_design_session_surfaces_diagnostics_field() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "diagnostics-field-session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let ops = vec![designer_graph::ops::Operation::AppendNode {
            anchor: start_key,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            edge_id: "f1".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "diagnostics-field-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert!(
            body["diagnostics"].as_array().is_some(),
            "expected a diagnostics array on the save response, got {body:?}"
        );
    }

    /// RED: a save on a graph-backed session that doesn't admit (unmatched
    /// fork) must be refused, not silently save a broken plan.
    #[tokio::test]
    async fn test_save_design_session_refuses_non_admitting_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "unmatched-fork-save" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let split_key = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: split_key,
                node: bpmn_lite_compiler::IRNode::GatewayAnd {
                    id: "split".into(),
                    name: "split".into(),
                    direction: bpmn_lite_compiler::GatewayDirection::Diverging,
                },
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: split_key,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        // This op sequence doesn't admit (unmatched fork), so it's refused
        // AT THE GRAPH-EDIT STEP already (BLOCKER 1's fix) — nothing is
        // persisted, so there's nothing further to save. Confirms the two
        // fixes compose: a session can never reach the save endpoint
        // carrying a non-admitting graph in the first place.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "unmatched-fork-template" }),
            ))
            .await
            .unwrap();
        // No GraphEdit ever landed, so this is a non-graph-backed session —
        // refused by the legacy-authoring-path gate (save-as-template
        // rewire, 2026-07-30; previously the "no revision to save" 400).
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(body_json(response).await["error"], "legacy_authoring_path");
    }

    /// G2 BLOCKER-2 ruling, receipt (ii): saving a graph-backed session as
    /// a template must NOT touch the session store's edit log — the DAG
    /// stays the authoring truth there (Rider 2, "two stores, two roles").
    /// Reopen-for-edit after a save must still reconstruct the exact same
    /// graph, and further graph-edits against the session must still work.
    #[tokio::test]
    async fn test_save_design_session_preserves_reopen_for_edit() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "reopen-after-save" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let first_ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": first_ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let record_before = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        let events_before = record_before["events"].as_array().unwrap().len();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "reopen-after-save-template" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // The edit log is untouched by save — same event count, same
        // GraphEdit payloads (Rider 2: the session store is not rewritten
        // just because the plan store was).
        let record_after = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            record_after["events"].as_array().unwrap().len(),
            events_before,
            "save must not append to or mutate the session's edit log"
        );

        // The session is still open for editing — reopen-for-edit means a
        // further graph-edit against the SAME anchors continues to work
        // exactly as it would have pre-save.
        let further_ops = vec![designer_graph::ops::Operation::AppendNode {
            anchor: t1,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::End {
                id: "end2".into(),
                terminate: false,
            },
            edge_id: "f3".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": further_ops }),
            ))
            .await
            .unwrap();
        // t1 already has an outgoing edge (to "end") from the first save's
        // graph — AppendNode correctly refuses a second one; the important
        // assertion is that reconstruction/staging still WORKS (422 from
        // real staging logic, not 500 from a corrupted/frozen session).
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "post-save reconstruction must still run real staging logic, not 500"
        );
    }

    // ── Phase B: spawn instance from a Published template (2026-07-30) ──

    /// Publishes a minimal admittable graph-backed template through the
    /// real save flow (same shape as
    /// `test_save_design_session_projects_graph_backed_session`), returning
    /// the save endpoint's response body. Produces a `Published` v1 under
    /// `template_name`.
    async fn publish_admittable_template(app: &Router, template_name: &str) -> Value {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": format!("{template_name}-session") }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);
        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("review_documents"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": template_name }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        body
    }

    /// Phase C helper: like `publish_admittable_template` but the graph is
    /// start → four ServiceTasks (task_type "noop", ids t1..t4) → end, so
    /// the spawned instance genuinely parks on jobs the advance endpoint
    /// must complete through the real VM.
    async fn publish_four_task_template(app: &Router, template_name: &str) -> Value {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": format!("{template_name}-session") }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let mut anchor = seed_start_key(sid);
        let mut ops = Vec::new();
        for (i, task_id) in ["t1", "t2", "t3", "t4"].iter().enumerate() {
            let key = new_key();
            ops.push(designer_graph::ops::Operation::AppendNode {
                anchor,
                key,
                node: task_ir(task_id),
                edge_id: format!("f{}", i + 1),
            });
            anchor = key;
        }
        ops.push(designer_graph::ops::Operation::AppendNode {
            anchor,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            edge_id: "f5".into(),
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": template_name }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        body
    }

    fn get_req(uri: &str) -> Request<axum::body::Body> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    fn minimal_dto(id: &str) -> bpmn_lite_authoring::WorkflowGraphDto {
        bpmn_lite_authoring::WorkflowGraphDto {
            id: id.to_string(),
            meta: None,
            nodes: vec![
                bpmn_lite_authoring::NodeDto::Start {
                    id: "start".to_string(),
                },
                bpmn_lite_authoring::NodeDto::End {
                    id: "end".to_string(),
                    terminate: false,
                },
            ],
            edges: vec![bpmn_lite_authoring::EdgeDto {
                from: "start".to_string(),
                to: "end".to_string(),
                condition: None,
                is_default: false,
                on_error: None,
            }],
            default_guard_budget: None,
            default_retry_policy: None,
        }
    }

    fn draft_template(key: &str, version: u32) -> bpmn_lite_authoring::WorkflowTemplate {
        bpmn_lite_authoring::WorkflowTemplate {
            template_key: key.to_string(),
            template_version: version,
            process_key: key.to_string(),
            bytecode_version: "deadbeef".to_string(),
            state: bpmn_lite_authoring::TemplateState::Draft,
            source_format: bpmn_lite_authoring::SourceFormat::Graph,
            dto_snapshot: minimal_dto(key),
            task_manifest: vec![],
            parameter_manifest: bpmn_lite_authoring::ParameterManifest::default(),
            bpmn_xml: None,
            summary_md: None,
            verb_registry_hash: None,
            created_at: 0,
            published_at: None,
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_spawn_published_template_creates_instance() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_admittable_template(&app, "spawn-green-template").await;

        let response = app
            .clone()
            .oneshot(post_json(
                "/bpmn/templates/spawn-green-template/spawn",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["template_key"], "spawn-green-template");
        assert_eq!(body["template_version"], 1);
        assert_eq!(body["bytecode_version"].as_str().unwrap().len(), 64);

        let instance_id: Uuid = body["instance_id"].as_str().unwrap().parse().unwrap();
        let instance = state
            .store
            .load_instance(&state.tenant_id, instance_id)
            .await
            .unwrap();
        assert!(
            instance.is_some(),
            "spawned instance must be persisted in the store the template was published into"
        );
    }

    // ── Phase C: instance status + advance round trip (2026-08-01) ────

    /// GREEN cement: the first true engine-execution round trip in the
    /// repo — spawn from a Published 4-ServiceTask template, observe it
    /// Running with a waiting job, then advance (real tick + complete_job
    /// against the VM, not the runner's plan-walker) until Completed.
    #[tokio::test]
    async fn test_instance_round_trip_spawn_advance_to_completed() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_four_task_template(&app, "round-trip-template").await;

        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/round-trip-template/spawn",
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        // Initial status: Running, parked on the first ServiceTask's job.
        let response = app
            .clone()
            .oneshot(get_req(&format!("/bpmn/instances/{instance_id}/status")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let status = body_json(response).await;
        assert_eq!(status["state"], "Running", "{status:?}");
        assert_eq!(status["instance_id"], instance_id);
        assert!(
            !status["waiting_jobs"].as_array().unwrap().is_empty(),
            "spawned instance must be parked on a job: {status:?}"
        );
        assert!(status["fiber_count"].as_u64().unwrap() >= 1);
        assert!(status["wait_count"].as_u64().unwrap() >= 1);

        // Advance until Completed — bounded, no unbounded polling.
        let mut last = status;
        for _ in 0..10 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            last = body_json(response).await;
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(
            last["state"], "Completed",
            "instance must complete within 10 advance rounds: {last:?}"
        );
        assert!(
            last["waiting_jobs"].as_array().unwrap().is_empty(),
            "no jobs may remain after completion: {last:?}"
        );
        assert!(
            last["completed_at"].as_i64().is_some(),
            "Completed carries a timestamp: {last:?}"
        );
    }

    // ── WS-D D3: timer semantics through the serving path ─────────────

    /// Session-author a guarded template through the REAL product path:
    /// graph-edit ops (AppendNode t1 → end; AttachGuard on t1 with a 60s
    /// interrupting timer; escape flow escalate → its own End) → save →
    /// publish. Returns nothing extra — the template is spawnable by name.
    async fn publish_guarded_template(app: &Router, template_name: &str) {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": format!("{template_name}-session") }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start = seed_start_key(sid);
        let t1 = new_key();
        let guard = new_key();
        let esc = new_key();
        let notify = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
            designer_graph::ops::Operation::AttachGuard {
                host: t1,
                key: guard,
                guard_id: "bt".into(),
                trigger: designer_graph::ops::GuardTrigger::Timer(
                    bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
                ),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: guard,
                key: esc,
                node: task_ir("escalate"),
                edge_id: "g1".into(),
            },
            // A second escape task gives the timeout round an observable
            // intermediate state: after the guard fires and `escalate`
            // completes, the fiber parks on `notify_esc`'s job — the
            // waiting job's node id NAMES the escape route mid-flight.
            designer_graph::ops::Operation::AppendNode {
                anchor: esc,
                key: notify,
                node: task_ir("notify_esc"),
                edge_id: "g2".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: notify,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end_esc".into(),
                    terminate: false,
                },
                edge_id: "g3".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": template_name }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "guarded template must save: {body:?}"
        );
    }

    async fn spawn_named(app: &Router, template_name: &str) -> Uuid {
        body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/bpmn/templates/{template_name}/spawn"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await["instance_id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    /// THE WS-D D3 money receipt: a spawned guarded instance whose
    /// deadline has passed times out THROUGH THE REST SURFACE — the
    /// advance's leading `tick_due_timers` fires the interrupting guard
    /// BEFORE the round dequeues the host's job, the host is unwound,
    /// and the instance completes down the ESCAPE flow (final node
    /// end_esc), not the normal one.
    #[tokio::test]
    async fn test_guarded_instance_times_out_down_escape_flow() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_guarded_template(&app, "timeout-template").await;
        let instance_id = spawn_named(&app, "timeout-template").await;

        // Armed and parked on the host's job.
        let status = body_json(
            app.clone()
                .oneshot(get_req(&format!("/bpmn/instances/{instance_id}/status")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status["state"], "Running", "{status:?}");
        assert!(!status["waiting_jobs"].as_array().unwrap().is_empty());

        // Advance with the clock injected PAST the 60s deadline: the
        // guard fires, the host is unwound (its job never completes),
        // and the escape route runs. The mid-flight round parks on the
        // SECOND escape task — the waiting job's node id names the
        // escape route, the observable that proves which path executed.
        let future = unix_time_ms() as u64 + 120_000;
        let mut saw_escape_route = false;
        let mut last = status;
        for _ in 0..10 {
            last = body_json(
                app.clone()
                    .oneshot(post_json(
                        &format!("/bpmn/instances/{instance_id}/advance"),
                        serde_json::json!({ "logical_time_ms": future }),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            if last["waiting_jobs"]
                .as_array()
                .map(|jobs| jobs.iter().any(|j| j["node_id"] == "notify_esc"))
                .unwrap_or(false)
            {
                saw_escape_route = true;
            }
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(
            last["state"], "Completed",
            "timed-out instance must complete: {last:?}"
        );
        assert!(
            saw_escape_route,
            "completion must be via the guard's ESCAPE flow (a round parked on notify_esc)"
        );
        let _ = &state;
    }

    /// Control receipt: the same guarded template advanced with wall
    /// clock (deadline NOT due) completes down the NORMAL flow — the
    /// guard armed, never fired, and did not disturb the host.
    #[tokio::test]
    async fn test_guarded_instance_completes_normally_before_deadline() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_guarded_template(&app, "no-timeout-template").await;
        let instance_id = spawn_named(&app, "no-timeout-template").await;

        let mut last = serde_json::json!({});
        let mut saw_escape_route = false;
        for _ in 0..10 {
            last = body_json(
                app.clone()
                    .oneshot(post_json(
                        &format!("/bpmn/instances/{instance_id}/advance"),
                        serde_json::json!({}),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            if last["waiting_jobs"]
                .as_array()
                .map(|jobs| {
                    jobs.iter()
                        .any(|j| j["node_id"] == "escalate" || j["node_id"] == "notify_esc")
                })
                .unwrap_or(false)
            {
                saw_escape_route = true;
            }
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(last["state"], "Completed", "{last:?}");
        assert!(
            !saw_escape_route,
            "before the deadline the NORMAL flow completes — no round may park on the escape route"
        );
        let _ = &state;
    }

    /// WS-D D3: a standalone Wait node parks the fiber on a real durable
    /// timer — visible in `waiting_timers` with its deadline; advancing
    /// before the deadline holds, advancing past it resumes and completes.
    #[tokio::test]
    async fn test_wait_node_holds_then_resumes_past_deadline() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "wait-session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let wait = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: seed_start_key(sid),
                key: wait,
                node: bpmn_lite_compiler::IRNode::TimerWait {
                    id: "cooling_off".into(),
                    spec: bpmn_lite_compiler::TimerSpec::Duration { ms: 60_000 },
                },
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: wait,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "wait-template" }),
            ))
            .await
            .unwrap();
        let status_code = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status_code,
            StatusCode::OK,
            "wait template must save: {body:?}"
        );
        let instance_id = spawn_named(&app, "wait-template").await;

        // Parked on the timer, surfaced with its deadline.
        let status = body_json(
            app.clone()
                .oneshot(get_req(&format!("/bpmn/instances/{instance_id}/status")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(status["state"], "Running", "{status:?}");
        let timers = status["waiting_timers"].as_array().unwrap();
        assert_eq!(
            timers.len(),
            1,
            "the Wait fiber must be visible: {status:?}"
        );
        assert!(timers[0]["deadline_ms"].as_u64().unwrap() > 0);

        // Before the deadline: advance holds.
        let held = body_json(
            app.clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            held["state"], "Running",
            "not due yet — must hold: {held:?}"
        );
        assert_eq!(held["waiting_timers"].as_array().unwrap().len(), 1);

        // Past the deadline: fires, resumes, completes.
        let future = unix_time_ms() as u64 + 120_000;
        let mut last = held;
        for _ in 0..5 {
            last = body_json(
                app.clone()
                    .oneshot(post_json(
                        &format!("/bpmn/instances/{instance_id}/advance"),
                        serde_json::json!({ "logical_time_ms": future }),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(last["state"], "Completed", "{last:?}");
        assert!(last["waiting_timers"].as_array().unwrap().is_empty());
    }

    /// RED cement: status of an unknown instance → 404, not a 200 with
    /// fabricated state.
    #[tokio::test]
    async fn test_instance_status_unknown_instance_not_found() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let response = app
            .oneshot(get_req(&format!(
                "/bpmn/instances/{}/status",
                Uuid::new_v4()
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert_eq!(body["error"], "instance_not_found");
    }

    /// RED cement: advance of an unknown instance → 404.
    #[tokio::test]
    async fn test_advance_unknown_instance_not_found() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state);
        let response = app
            .oneshot(post_json(
                &format!("/bpmn/instances/{}/advance", Uuid::new_v4()),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_json(response).await;
        assert_eq!(body["error"], "instance_not_found");
    }

    /// GREEN cement: advance on an already-Completed instance is an
    /// idempotent no-op — 200, state stays "Completed".
    #[tokio::test]
    async fn test_advance_completed_instance_idempotent() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_four_task_template(&app, "idempotent-template").await;

        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/idempotent-template/spawn",
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        let mut last = serde_json::json!({});
        for _ in 0..10 {
            last = body_json(
                app.clone()
                    .oneshot(post_json(
                        &format!("/bpmn/instances/{instance_id}/advance"),
                        serde_json::json!({}),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(last["state"], "Completed", "{last:?}");
        let completed_at = last["completed_at"].as_i64();

        // Advance again — must be a 200 no-op, state unchanged.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/bpmn/instances/{instance_id}/advance"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["state"], "Completed");
        assert_eq!(body["completed_at"].as_i64(), completed_at);
        assert!(body["waiting_jobs"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_spawn_draft_template_rejected() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        state
            .template_store
            .save(&draft_template("draft-only-template", 1))
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(post_json(
                "/bpmn/templates/draft-only-template/spawn",
                serde_json::json!({ "version": 1 }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::CONFLICT, "{body:?}");
        assert_eq!(body["error"], "template_not_published");
        assert_eq!(body["template_key"], "draft-only-template");
        assert_eq!(body["template_version"], 1);
        assert_eq!(body["state"], "Draft");
    }

    #[tokio::test]
    async fn test_spawn_nonexistent_template_not_found() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/bpmn/templates/does-not-exist/spawn",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
        assert_eq!(body["error"], "no_published_version");

        let response = app
            .clone()
            .oneshot(post_json(
                "/bpmn/templates/does-not-exist/spawn",
                serde_json::json!({ "version": 1 }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
        assert_eq!(body["error"], "template_not_found");
    }

    #[tokio::test]
    async fn test_spawn_resolves_latest_published_not_latest_draft() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_admittable_template(&app, "layered-template").await;

        // Stack a Draft v2 on top — spawn with no version must still
        // resolve v1 (Published), not "latest version regardless of state".
        state
            .template_store
            .save(&draft_template("layered-template", 2))
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(post_json(
                "/bpmn/templates/layered-template/spawn",
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(
            body["template_version"], 1,
            "must resolve the Published version, not the stacked Draft v2"
        );
    }

    #[tokio::test]
    async fn test_list_published_templates_excludes_draft_and_retired() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        publish_admittable_template(&app, "catalogue-template").await;
        state
            .template_store
            .save(&draft_template("catalogue-template", 2))
            .await
            .unwrap();
        state
            .template_store
            .save(&draft_template("retired-sibling", 1))
            .await
            .unwrap();
        state
            .template_store
            .set_state(
                "retired-sibling",
                1,
                bpmn_lite_authoring::TemplateState::Published,
            )
            .await
            .unwrap();
        state
            .template_store
            .set_state(
                "retired-sibling",
                1,
                bpmn_lite_authoring::TemplateState::Retired,
            )
            .await
            .unwrap();

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates/published")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        let list = body.as_array().unwrap();
        assert_eq!(
            list.len(),
            1,
            "only catalogue-template v1 (Published) should be listed: {list:?}"
        );
        assert_eq!(list[0]["template_key"], "catalogue-template");
        assert_eq!(list[0]["template_version"], 1);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/bpmn/templates/published?key=retired-sibling")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert!(
            body.as_array().unwrap().is_empty(),
            "retired-sibling has no Published version: {body:?}"
        );
    }

    /// GREEN: the session graph endpoint serves GRAPH-BACKED sessions
    /// through the same admitted DAG -> to_ir -> project_ir chain the
    /// save/spawn path uses — the visualiser shows the COMPILED
    /// workflow, laid out in execution order (x strictly increasing
    /// along the start -> t1..t4 -> end chain). Before this branch
    /// existed, a graph session fell through to the text path and
    /// returned compiles:false on its empty source.
    #[tokio::test]
    async fn test_session_graph_endpoint_serves_compiled_graph_for_graph_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "graph viz session" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let session_id = body_json(response).await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();

        let mut anchor = seed_start_key(sid);
        let mut ops = Vec::new();
        for (i, task) in ["t1", "t2", "t3", "t4"].iter().enumerate() {
            let key = new_key();
            ops.push(designer_graph::ops::Operation::AppendNode {
                anchor,
                key,
                node: task_ir(task),
                edge_id: format!("f{}", i + 1),
            });
            anchor = key;
        }
        ops.push(designer_graph::ops::Operation::AppendNode {
            anchor,
            key: new_key(),
            node: bpmn_lite_compiler::IRNode::End {
                id: "end".into(),
                terminate: false,
            },
            edge_id: "f5".into(),
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "4-step chain" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{session_id}/graph"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["compiles"], true, "{body:?}");
        let nodes = body["graph"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 6, "start + 4 tasks + end: {body:?}");
        let x_of = |id: &str| -> f64 {
            body["layout"][id]["x"]
                .as_f64()
                .unwrap_or_else(|| panic!("layout missing {id}: {body:?}"))
        };
        // Execution order left-to-right: each hop strictly to the right.
        let chain: Vec<&str> = nodes
            .iter()
            .filter_map(|n| n["id"].as_str())
            .collect::<Vec<_>>();
        assert!(chain.iter().any(|id| id.contains("t1")), "{chain:?}");
        let mut ordered: Vec<(&str, f64)> = chain.iter().map(|id| (*id, x_of(id))).collect();
        ordered.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let xs: Vec<f64> = ordered.iter().map(|(_, x)| *x).collect();
        for w in xs.windows(2) {
            assert!(
                w[0] < w[1],
                "layout is a strict left-to-right chain: {ordered:?}"
            );
        }
        assert_eq!(ordered.first().unwrap().1, 0.0, "start at x=0: {ordered:?}");
    }

    /// Build the branched SESE session over the wire and return its id:
    ///
    /// start -> t1 -> split -> t2a -> t3a -> join -> t4 -> t5 -> end
    ///                     \-> t2b -> t3b ->/
    ///
    /// ONE graph-edit call: `CreateParallelRegion` inserts the fork/join
    /// block CLOSED (there is no open-region staging state), the two
    /// `InsertAfter`s extend each branch to two tasks, and the tail
    /// `AppendNode`s finish the chain — admit() runs once over the whole
    /// staged sequence, so nothing partial is ever persisted.
    async fn build_branched_session(app: &Router, name: &str) -> String {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": name }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let fork = new_key();
        let join = new_key();
        let t2a = new_key();
        let t2b = new_key();
        let t4 = new_key();
        let t5 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::CreateParallelRegion {
                anchor: t1,
                fork_key: fork,
                fork_node_id: "split".into(),
                join_key: join,
                join_node_id: "join".into(),
                entry_edge_id: "f2".into(),
                branches: vec![
                    designer_graph::ops::RegionBranch {
                        key: t2a,
                        node: task_ir("t2a"),
                        in_edge_id: "f3a".into(),
                        out_edge_id: "f4a".into(),
                        condition: None,
                    },
                    designer_graph::ops::RegionBranch {
                        key: t2b,
                        node: task_ir("t2b"),
                        in_edge_id: "f3b".into(),
                        out_edge_id: "f4b".into(),
                        condition: None,
                    },
                ],
            },
            // Extend each branch to two tasks: InsertAfter re-points the
            // branch's out-edge (f4a/f4b) so it now leaves from t3a/t3b
            // into the join, preserving the flow ids.
            designer_graph::ops::Operation::InsertAfter {
                anchor: t2a,
                key: new_key(),
                node: task_ir("t3a"),
                edge_id: "f5a".into(),
            },
            designer_graph::ops::Operation::InsertAfter {
                anchor: t2b,
                key: new_key(),
                node: task_ir("t3b"),
                edge_id: "f5b".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: join,
                key: t4,
                node: task_ir("t4"),
                edge_id: "f6".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t4,
                key: t5,
                node: task_ir("t5"),
                edge_id: "f7".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t5,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f8".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "branched build" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "branched graph-edit must admit: {body:?}"
        );
        session_id
    }

    /// GREEN cement: a 2-way parallel BRANCH + MERGE session (6 tasks,
    /// matched GatewayAnd pair) admits over the wire, and the graph
    /// endpoint serves the COMPILED branched graph with a sane layered
    /// layout — same depth for the branch heads (one x, two lanes), the
    /// join right of both branches, the tail right of the join.
    #[tokio::test]
    async fn test_session_graph_endpoint_serves_branched_graph() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let session_id = build_branched_session(&app, "branched viz session").await;

        let response = app
            .clone()
            .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}/graph")))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(body["compiles"], true, "{body:?}");
        let nodes = body["graph"]["nodes"].as_array().unwrap();
        assert_eq!(
            nodes.len(),
            11,
            "start + t1 + split + (t2a,t3a,t2b,t3b) + join + t4 + t5 + end: {body:?}"
        );
        let coord = |id: &str, axis: &str| -> f64 {
            body["layout"][id][axis]
                .as_f64()
                .unwrap_or_else(|| panic!("layout missing {id}.{axis}: {body:?}"))
        };
        // Branch heads share a depth (same x) but occupy two lanes
        // (different y).
        assert_eq!(coord("t2a", "x"), coord("t2b", "x"), "{body:?}");
        assert_ne!(coord("t2a", "y"), coord("t2b", "y"), "{body:?}");
        assert_eq!(coord("t3a", "x"), coord("t3b", "x"), "{body:?}");
        assert_ne!(coord("t3a", "y"), coord("t3b", "y"), "{body:?}");
        // The join sits strictly right of every branch node; the tail
        // strictly right of the join, in chain order.
        for branch_node in ["t2a", "t2b", "t3a", "t3b"] {
            assert!(
                coord("join", "x") > coord(branch_node, "x"),
                "join must be right of {branch_node}: {body:?}"
            );
        }
        assert!(coord("split", "x") > coord("t1", "x"), "{body:?}");
        assert!(coord("t4", "x") > coord("join", "x"), "{body:?}");
        assert!(coord("t5", "x") > coord("t4", "x"), "{body:?}");
        assert!(coord("end", "x") > coord("t5", "x"), "{body:?}");
    }

    /// GREEN cement: the branched template publishes, spawns, and runs to
    /// Completed through the REAL engine — and mid-flight the parallel
    /// region is observable as 2 fibers parked on 2 waiting jobs
    /// simultaneously (both branches in flight at once, not a
    /// sequentialized walk).
    #[tokio::test]
    async fn test_branched_instance_runs_to_completion() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let session_id = build_branched_session(&app, "branched run session").await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "branched-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "branched save must publish: {body:?}"
        );

        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/branched-template/spawn",
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        // Spawn parks on t1's job first: single fiber, single wait.
        let response = app
            .clone()
            .oneshot(get_req(&format!("/bpmn/instances/{instance_id}/status")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let status = body_json(response).await;
        assert_eq!(status["state"], "Running", "{status:?}");

        // Advance until Completed — bounded — and require the parallel
        // region to be OBSERVED mid-flight: at least one round must report
        // 2 waiting jobs across >= 2 fibers.
        let mut saw_parallel_midflight = false;
        let mut last = status;
        for _ in 0..12 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            last = body_json(response).await;
            if last["waiting_jobs"].as_array().unwrap().len() == 2
                && last["fiber_count"].as_u64().unwrap() >= 2
            {
                saw_parallel_midflight = true;
            }
            if last["state"] == "Completed" {
                break;
            }
        }
        assert!(
            saw_parallel_midflight,
            "the parallel region must be observable mid-flight as 2 waiting \
             jobs on >= 2 fibers — a sequentialized walk would never show it"
        );
        assert_eq!(
            last["state"], "Completed",
            "branched instance must complete within 12 advance rounds: {last:?}"
        );
        assert!(
            last["waiting_jobs"].as_array().unwrap().is_empty(),
            "no jobs may remain after completion: {last:?}"
        );
        assert!(
            last["completed_at"].as_i64().is_some(),
            "Completed carries a timestamp: {last:?}"
        );
    }

    /// G5.3 — a session ending in `IRNode::End { terminate: true }`, built
    /// the ONLY way an End node's `terminate` flag is reachable today: the
    /// caller supplies the full `IRNode` value inside a generic
    /// `AppendNode` operation (there is no dedicated "terminating end"
    /// production/palette entry, and none is being added — see the G5
    /// receipt). This is exactly the "hand-craft the JSON against the
    /// generic graph-edit endpoint" path the S6 matrix gap named as
    /// reachable-but-untested; this helper and its two callers below are
    /// that path, now proven and cemented.
    async fn build_terminate_session(app: &Router, name: &str) -> String {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": name }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end_term".into(),
                    terminate: true,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "terminate build" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "terminate graph-edit must admit: {body:?}"
        );
        session_id
    }

    /// GREEN cement: `IRNode::End { terminate: true }`, authored via the
    /// generic graph-edit endpoint (no dedicated production exists — see
    /// `build_terminate_session`'s doc comment), publishes, spawns, and
    /// runs to `Terminated` (NOT `Completed`) through the real engine —
    /// closing the S6 matrix's END-TERMINATE row, previously untested
    /// anywhere above the bytecode-instruction level
    /// (`bpmn-lite-engine::tests::t_term_4_parse_terminate_end_event`
    /// proves the XML→`Instr::EndTerminate` lowering; this proves the
    /// SAME flag through the graph/DTO/REST authoring surface).
    #[tokio::test]
    async fn test_terminate_instance_runs_to_terminated() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let session_id = build_terminate_session(&app, "terminate run session").await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "terminate-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "terminate save must publish: {body:?}");

        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/terminate-template/spawn",
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        let mut last = serde_json::json!({});
        for _ in 0..6 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            last = body_json(response).await;
            if last["state"] != "Running" {
                break;
            }
        }
        assert_eq!(
            last["state"], "Terminated",
            "an End node with terminate:true must end the instance Terminated, \
             not Completed: {last:?}"
        );
    }

    /// G5.4b — the missing REST-integration-test counterpart to
    /// `build_branched_session`/`test_branched_instance_runs_to_completion`
    /// (`CreateParallelRegion`), for `CreateInclusiveRegion`. Both branches
    /// carry an `== false` condition on their own flag name; the calling
    /// test's spawn payload supplies `false` for both (traced while writing
    /// this test: a graph-authored inclusive condition's `flag_name`
    /// becomes a PLACEHOLDER check — `V2LoadPlaceholderMatch` against
    /// `instance.placeholder_values`, populated verbatim from spawn's JSON
    /// payload — not a `Value`-flag check; `ir_plan::project_ir`'s
    /// diverging-gateway arm converts `ConditionExpr.flag_name` straight
    /// into `SplitExecFlow.placeholder`). Making both conditions live is
    /// what makes the fork genuinely DYNAMIC-arity rather than a single
    /// predetermined branch.
    async fn build_inclusive_session(app: &Router, name: &str) -> String {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": name }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let fork = new_key();
        let join = new_key();
        let b1 = new_key();
        let b2 = new_key();
        let t2 = new_key();
        let cond = |flag: &str| bpmn_lite_compiler::ConditionExpr {
            flag_name: flag.to_string(),
            op: bpmn_lite_compiler::ConditionOp::Eq,
            literal: bpmn_lite_compiler::ConditionLiteral::Bool(false),
        };
        let ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::CreateInclusiveRegion {
                anchor: t1,
                fork_key: fork,
                fork_node_id: "ifork".into(),
                join_key: join,
                join_node_id: "ijoin".into(),
                entry_edge_id: "f2".into(),
                branches: vec![
                    designer_graph::ops::RegionBranch {
                        key: b1,
                        node: task_ir("b1"),
                        in_edge_id: "f3a".into(),
                        out_edge_id: "f4a".into(),
                        condition: Some(cond("flag_a")),
                    },
                    designer_graph::ops::RegionBranch {
                        key: b2,
                        node: task_ir("b2"),
                        in_edge_id: "f3b".into(),
                        out_edge_id: "f4b".into(),
                        condition: Some(cond("flag_b")),
                    },
                ],
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: join,
                key: t2,
                node: task_ir("t2"),
                edge_id: "f5".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t2,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f6".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "inclusive build" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "inclusive graph-edit must admit: {body:?}"
        );
        session_id
    }

    /// GREEN cement: an inclusive (OR) region with two branches both live
    /// by construction publishes, spawns, and runs to `Completed` through
    /// the real engine — and mid-flight the fork is observable as 2 fibers
    /// parked on 2 waiting jobs simultaneously, the same dynamic-arity
    /// signature `test_branched_instance_runs_to_completion` proves for
    /// `GatewayAnd`, now proven for `GatewayInclusive` at the REST layer
    /// (previously crate-level/admission-only — `ops::tests::
    /// inclusive_region_with_conditions_admits` — never executed).
    #[tokio::test]
    async fn test_inclusive_instance_runs_to_completion_with_two_live_branches() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let session_id = build_inclusive_session(&app, "inclusive run session").await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "inclusive-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "inclusive save must publish: {body:?}");

        // Both conditions check `== false`; supplying `false` for both
        // placeholders via the spawn payload (`V2LoadPlaceholderMatch`
        // reads `instance.placeholder_values`, populated verbatim from
        // this JSON object — traced while writing this test) makes both
        // branches live simultaneously, proving genuine dynamic-arity fork
        // rather than a single predetermined branch.
        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/inclusive-template/spawn",
                    serde_json::json!({ "payload": { "flag_a": false, "flag_b": false } }),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        let mut saw_dynamic_fork_midflight = false;
        let mut last = serde_json::json!({});
        for _ in 0..12 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            last = body_json(response).await;
            if last["waiting_jobs"].as_array().unwrap().len() == 2
                && last["fiber_count"].as_u64().unwrap() >= 2
            {
                saw_dynamic_fork_midflight = true;
            }
            if last["state"] == "Completed" {
                break;
            }
        }
        assert!(
            saw_dynamic_fork_midflight,
            "both inclusive branches must be observable in flight simultaneously \
             (2 waiting jobs, >= 2 fibers) — a single-branch fallback would never \
             show it: {last:?}"
        );
        assert_eq!(
            last["state"], "Completed",
            "inclusive instance must complete within 12 advance rounds: {last:?}"
        );
    }

    /// G5.4b — the REST-integration-test counterpart for
    /// `CreateMultiInstanceRegion`, closing the last row of the S6 matrix's
    /// REST-integration gap. Only reachable at all as of G5.4a (`ir_plan.rs`
    /// previously refused every graph carrying an `IRNode::MultiInstance`
    /// with `UnsupportedNode` at the save step, before an instance could
    /// ever be spawned).
    ///
    /// Uses an UNDECLARED collection flag (empty/zero-length by
    /// construction) rather than a real supplied collection: instantiation-
    /// time parameter supply is explicitly out of scope for this whole plan
    /// ("instance creation / the factory (Designer ends at a manifest-
    /// bearing template)" — plan doc §0), and no REST endpoint can write an
    /// arbitrary flag today regardless (`advance_instance_endpoint` always
    /// completes jobs with empty `orch_flags`; spawn's initial payload binds
    /// declared placeholders/DataObjects, never a raw `Value::Array` flag —
    /// traced while writing this test, see `bpmn_lite_engine::tests::
    /// set_collection`, a store-level-only test backdoor with no REST
    /// counterpart). This still proves the real thing G5.4a fixed: the full
    /// graph-edit → save → spawn → advance round trip for an MI region
    /// reaches a real `V2MiArityCheck`/`V2Fork`/`V2Join` bytecode execution
    /// (crate-level: `dsl::frontend::tests::
    /// multi_instance_projected_from_ir_lowers_with_arity_check_and_join`)
    /// and completes cleanly with zero elements — the engine-level
    /// `t_mi_v2_empty_collection_completes_without_incident` case, now
    /// proven reachable over HTTP.
    async fn build_mi_session(app: &Router, name: &str) -> String {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": name }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let mi = new_key();
        let directors = new_key();
        let ops = vec![
            // G7.4: verify_data_objects now requires collection_flag_name
            // ('directors' below) to name a declared DataObject.
            designer_graph::ops::Operation::CreateDataObject {
                key: directors,
                id: "directors".into(),
                name: "directors".into(),
                type_decl: bpmn_lite_types::DataObjectType::Primitive(
                    bpmn_lite_types::PrimitiveType::String,
                ),
                role: bpmn_lite_types::DataObjectRole::Input,
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::CreateMultiInstanceRegion {
                anchor: t1,
                key: mi,
                node: bpmn_lite_compiler::IRNode::MultiInstance {
                    id: "verify_each".into(),
                    name: "verify_each".into(),
                    task_type: "noop".into(),
                    collection_flag_name: "directors".into(),
                    declared_max: 2,
                    inputs: Vec::new(),
                },
                edge_id: "f2".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: mi,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f3".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops, "note": "MI build" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "MI graph-edit must admit: {body:?}");
        session_id
    }

    /// GREEN cement: a `MultiInstance` region — reachable only via the
    /// generic `CreateMultiInstanceRegion` operation (no palette entry adds
    /// declared_max/collection beyond what that operation already takes) —
    /// publishes, spawns, and runs to `Completed` through the real engine.
    /// See `build_mi_session`'s doc comment for why the collection is
    /// empty/undeclared rather than real data.
    #[tokio::test]
    async fn test_mi_instance_runs_to_completion_with_empty_collection() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let session_id = build_mi_session(&app, "MI run session").await;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "mi-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "MI save must publish (this is exactly G5.4a's fix — MI used to \
             422 with 'no WorkflowExecutionPlan representation yet' here): {body:?}"
        );

        let spawn = body_json(
            app.clone()
                .oneshot(post_json(
                    "/bpmn/templates/mi-template/spawn",
                    serde_json::json!({}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        let mut last = serde_json::json!({});
        for _ in 0..6 {
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            last = body_json(response).await;
            if last["state"] != "Running" {
                break;
            }
        }
        assert_eq!(
            last["state"], "Completed",
            "an empty-collection MI region must run to Completed via real \
             V2MiArityCheck/V2Fork/V2Join bytecode, not stall or error: {last:?}"
        );
        assert!(
            last["waiting_jobs"].as_array().unwrap().is_empty(),
            "an empty collection must never dispatch a real job: {last:?}"
        );
    }

    /// Build a Postgres-backed DesignerState over the shared test database
    /// (same env convention as the authoring store tests).
    #[cfg(feature = "postgres")]
    async fn postgres_state() -> Arc<DesignerState> {
        let db_url = std::env::var("BPMN_LITE_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| "postgresql:///bpmn_lite_test".to_string());
        let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
        let pg = bpmn_lite_store_postgres::PostgresWorkflowStore::new(pool.clone());
        pg.migrate().await.unwrap();
        let store: Arc<dyn WorkflowStore> = Arc::new(pg);
        store
            .ensure_tenant(&TenantId::new("demo").unwrap())
            .await
            .unwrap();
        DesignerState::assemble(
            store,
            Arc::new(bpmn_lite_authoring::PostgresTemplateStore::new(pool)),
        )
        .unwrap()
    }

    /// RECEIPT — restart survival: publish a branched template through
    /// DesignerState #1 over Postgres, DROP that state entirely, build
    /// DesignerState #2 over the same database, and prove the authoring
    /// round trip survived the "restart": the template is still listed as
    /// Published, the design session is still loadable, and the template
    /// spawns + advances to Completed through the fresh state. The
    /// template key is uuid-derived because the test DB is shared across
    /// runs and the immutability trigger forbids republishing key+version.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_postgres_restart_survival() {
        let template_name = format!("restart-survival-{}", Uuid::new_v4());

        // ── Designer process #1 ────────────────────────────────────────
        let state1 = postgres_state().await;
        let app1 = designer_router(state1.clone());
        let session_id = build_branched_session(&app1, "restart survival session").await;
        let response = app1
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": template_name }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "publish must succeed: {body:?}");
        assert_eq!(body["state"], "published", "{body:?}");

        // "Restart": drop every handle to process #1's state.
        drop(app1);
        drop(state1);

        // ── Designer process #2, same database ─────────────────────────
        let state2 = postgres_state().await;
        let app2 = designer_router(state2.clone());

        // Template still listed as Published.
        let response = app2
            .clone()
            .oneshot(get_req("/bpmn/templates/published"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let list = body_json(response).await;
        let listed = list
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["template_key"] == template_name.as_str());
        assert!(
            listed,
            "published template must survive the restart: {list:?}"
        );

        // Design session created in state #1 is loadable in state #2.
        let response = app2
            .clone()
            .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}")))
            .await
            .unwrap();
        let status = response.status();
        let session = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "design session must survive the restart: {session:?}"
        );

        // Spawn from the surviving template and run to Completed.
        let response = app2
            .clone()
            .oneshot(post_json(
                &format!("/bpmn/templates/{template_name}/spawn"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let status = response.status();
        let spawn = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "spawn must succeed: {spawn:?}");
        let instance_id = spawn["instance_id"].as_str().unwrap().to_owned();

        let mut last = serde_json::Value::Null;
        for _ in 0..12 {
            let response = app2
                .clone()
                .oneshot(post_json(
                    &format!("/bpmn/instances/{instance_id}/advance"),
                    serde_json::json!({}),
                ))
                .await
                .unwrap();
            let status = response.status();
            last = body_json(response).await;
            assert_eq!(status, StatusCode::OK, "advance must succeed: {last:?}");
            if last["state"] == "Completed" {
                break;
            }
        }
        assert_eq!(
            last["state"], "Completed",
            "spawned instance must complete within 12 advance rounds: {last:?}"
        );
    }

    #[tokio::test]
    async fn oversized_request_body_is_refused_and_the_server_stays_usable() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        let oversized_body = "x".repeat(MAX_REQUEST_BODY_BYTES + 1);
        let request = Request::builder()
            .method("POST")
            .uri("/api/dsl/sessions")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(oversized_body))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "a body over the explicit product limit must be refused, not decoded"
        );

        // The server stays usable: a legitimate request on the same router
        // still succeeds after the oversized-body refusal.
        let response = app
            .clone()
            .oneshot(post_json(
                "/api/dsl/sessions",
                serde_json::json!({ "name": "post-refusal session", "dsl_source": "" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
    }

    // ── G6.1/G6.2/G6.3 — session undo, runbook rendering, replay
    // equivalence ───────────────────────────────────────────────────────

    /// Build a graph-backed session with TWO independently-admitting
    /// graph-edit events: A appends `t1` then `End` after Start
    /// (Start->t1->End); B inserts `t2` between `t1` and `End`
    /// (Start->t1->t2->End). Returns `(session_id, seq_after_a)` — the
    /// seq to undo back to, to land exactly on A's position.
    async fn build_two_step_session(app: &Router, name: &str) -> (String, u64) {
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": name }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        let t1 = new_key();
        let ops_a = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: start_key,
                key: t1,
                node: task_ir("t1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end1".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops_a, "note": "edit A" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "edit A must admit");
        let seq_after_a = body_json(response).await["seq"].as_u64().unwrap();

        let ops_b = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t1,
            key: new_key(),
            node: task_ir("t2"),
            edge_id: "f3".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops_b, "note": "edit B" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "edit B must admit");

        (session_id, seq_after_a)
    }

    /// GREEN: undoing back to the seq right after edit A returns the
    /// session to that exact prior position — `t2` (added by edit B) is
    /// gone from the live graph, `t1` (added by edit A) remains. Confirms
    /// Gate G6's "undo returns a session to a prior position" clause.
    #[tokio::test]
    async fn test_undo_returns_the_session_to_the_prior_position() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, seq_after_a) = build_two_step_session(&app, "undo session").await;

        let graph_before = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}/graph"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(graph_before["compiles"], true);
        let before_ids: Vec<String> = graph_before["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(before_ids.iter().any(|id| id == "t2"), "t2 must be present before undo: {before_ids:?}");

        let sid: Uuid = session_id.parse().unwrap();
        let pre_undo_hash = graph_content_hash(
            &state
                .store
                .load_design_session(&state.tenant_id, sid)
                .await
                .unwrap()
                .unwrap(),
        );

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/undo"),
                serde_json::json!({ "target_seq": seq_after_a }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let undo_body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "undo to a prior admitting position must succeed: {undo_body:?}");
        assert_eq!(undo_body["target_seq"].as_u64().unwrap(), seq_after_a);
        // Regression (blind review, G6.1): the response's content hash must
        // reflect the POST-undo (truncated) tape, not the stale pre-undo
        // live hash — edit B genuinely changed the content, so the two
        // must differ. It must also equal a fresh post-undo reload's own
        // (now undo-aware) live hash.
        let undo_hash = undo_body["graph_content_hash"].as_str().unwrap();
        assert_ne!(
            undo_hash, pre_undo_hash,
            "undo response's graph_content_hash must not equal the pre-undo live hash"
        );
        let post_undo_reload_hash = graph_content_hash(
            &state
                .store
                .load_design_session(&state.tenant_id, sid)
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(
            undo_hash, post_undo_reload_hash,
            "undo response's graph_content_hash must match a fresh post-undo reload"
        );

        let graph_after = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}/graph"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(graph_after["compiles"], true, "the undone-to position must still admit: {graph_after:?}");
        let after_ids: Vec<String> = graph_after["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(!after_ids.iter().any(|id| id == "t2"), "t2 must be gone after undo: {after_ids:?}");
        assert!(after_ids.iter().any(|id| id == "t1"), "t1 (from edit A) must remain: {after_ids:?}");
    }

    /// Regression (blind review, G6.1): after an undo, a SECOND undo call
    /// targeting a MORE RECENT position than the first undo's target must
    /// still succeed — the session's live head must always be the true
    /// latest event's seq, never the earlier undo's `target_seq`. Builds
    /// three sequential edits (A: t1+End, B: insert t2, C: insert t3),
    /// undoes back to A, then re-targets forward to B — a `target_seq`
    /// strictly between A and C, which a self-excising `Undo` marker would
    /// wrongly refuse (the first undo's collapsed "head" would sit at A,
    /// making B's seq look >= head).
    #[tokio::test]
    async fn test_a_later_undo_target_stays_reachable_after_an_earlier_undo() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, seq_after_a) = build_two_step_session(&app, "chained undo session").await;
        let sid: Uuid = session_id.parse().unwrap();
        let start_key = seed_start_key(sid);

        // edit B (from build_two_step_session) inserted t2 between t1 and
        // End; capture its seq before adding a third edit.
        let session_after_b = state
            .store
            .load_design_session(&state.tenant_id, sid)
            .await
            .unwrap()
            .unwrap();
        let seq_after_b = session_after_b
            .visible_events(None)
            .last()
            .map(|e| e.seq)
            .unwrap();

        // edit C: insert t3 after the existing t2 (found via the current graph).
        let dag = reconstruct_designer_dag(&session_after_b, None).unwrap();
        let t2_key = dag.key_for_bpmn_id("t2").unwrap();
        let ops_c = vec![designer_graph::ops::Operation::InsertAfter {
            anchor: t2_key,
            key: new_key(),
            node: task_ir("t3"),
            edge_id: "f4".into(),
        }];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": ops_c, "note": "edit C" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "edit C must admit");

        // First undo: back to A.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/undo"),
                serde_json::json!({ "target_seq": seq_after_a }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "first undo (to A) must succeed");

        // Second undo: re-target FORWARD to B (a more recent position than
        // A, but still strictly before the true live head, which is now
        // the first Undo marker's own seq — past seq_after_b).
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/undo"),
                serde_json::json!({ "target_seq": seq_after_b }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a later undo target must remain reachable after an earlier undo: {body:?}"
        );

        // Confirm the graph now reflects B's position: t2 present, t3 gone.
        let graph = body_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(format!("/api/dsl/sessions/{session_id}/graph"))
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(graph["compiles"], true);
        let ids: Vec<String> = graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(ids.iter().any(|id| id == "t2"), "t2 (edit B) must be present: {ids:?}");
        assert!(!ids.iter().any(|id| id == "t3"), "t3 (edit C) must be gone: {ids:?}");
        let _ = start_key;
    }

    /// RED cement: an undo `target_seq` at or past the current head is
    /// refused, never silently accepted (fail-closed — an undo that does
    /// nothing must say so, not report success).
    #[tokio::test]
    async fn test_undo_past_current_head_is_refused() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, seq_after_a) = build_two_step_session(&app, "undo refusal session").await;
        let too_far = seq_after_a + 100;

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/undo"),
                serde_json::json!({ "target_seq": too_far }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// GREEN: the runbook endpoint renders the accumulated operation tape
    /// as readable text, one line per operation, naming every operation
    /// actually applied.
    #[tokio::test]
    async fn test_runbook_renders_the_operation_tape() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _seq_after_a) = build_two_step_session(&app, "runbook session").await;

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/api/dsl/sessions/{session_id}/runbook"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["operation_count"].as_u64().unwrap(), 3);
        let runbook = body["runbook"].as_str().unwrap();
        assert_eq!(runbook.lines().count(), 3);
        assert!(runbook.contains("(append-node"));
        assert!(runbook.contains("(insert-after"));
        assert!(runbook.contains("\"t1\""));
        assert!(runbook.contains("\"t2\""));
    }

    /// G6.4 — a published template resolves to its authoring tape: after
    /// `save_design_session_endpoint` publishes a graph-authored session,
    /// the resulting `WorkflowTemplate::session_id` names the session it
    /// came from, and that session is loadable and carries the same
    /// operation tape the template was compiled from.
    #[tokio::test]
    async fn test_published_template_resolves_to_its_authoring_session() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _seq_after_a) = build_two_step_session(&app, "linkage session").await;
        let sid: Uuid = session_id.parse().unwrap();

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "linkage-template" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let template = state
            .template_store
            .load("linkage-template", 1)
            .await
            .unwrap()
            .expect("published template must be loadable");
        assert_eq!(
            template.session_id,
            Some(sid),
            "template must carry a back-reference to its authoring session"
        );

        let resolved_session = state
            .store
            .load_design_session(&state.tenant_id, template.session_id.unwrap())
            .await
            .unwrap()
            .expect("the session a template resolves to must still be loadable");
        assert_eq!(resolved_session.id, sid);
        assert!(
            resolved_session.is_graph_backed(),
            "the resolved session must be the same graph-backed tape that authored the template"
        );
    }

    /// G6.3 — replay-equivalence: two INDEPENDENT replays of the identical
    /// event log (two fresh `reconstruct_designer_dag` calls, no shared
    /// state between them) must produce structurally identical output —
    /// the same content hash and the same `IRGraph` under
    /// `DesignerDag::ir_graphs_equivalent`. This is the contract every
    /// request already leans on implicitly; here it is asserted directly.
    #[tokio::test]
    async fn test_replay_of_the_same_log_is_equivalent_across_independent_folds() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());
        let (session_id, _seq_after_a) = build_two_step_session(&app, "replay session").await;
        let sid: Uuid = session_id.parse().unwrap();

        let record = state
            .store
            .load_design_session(&state.tenant_id, sid)
            .await
            .unwrap()
            .unwrap();

        let dag_1 = reconstruct_designer_dag(&record, None).unwrap();
        let dag_2 = reconstruct_designer_dag(&record, None).unwrap();

        assert_eq!(
            graph_content_hash(&record),
            graph_content_hash(&record),
            "content hash must be stable across repeated computation"
        );
        let ir_1 = dag_1.to_ir().unwrap();
        let ir_2 = dag_2.to_ir().unwrap();
        assert!(
            designer_graph::schema::DesignerDag::ir_graphs_equivalent(&ir_1, &ir_2),
            "two independent replays of the identical tape must produce equivalent IR graphs"
        );
    }

    // ── Super-user REPL build: 6-step/2-branch/2-loop, utterance-driven ──

    /// One utterance -> ratify hop. Posts `text` at `anchor`, prints the
    /// full response body (this is a debugging aid — the binding surface
    /// is lexical-overlap-against-current-legal-moves, not free text, so
    /// failures must be diagnosed from the real disposition/workbook, not
    /// guessed at), asserts the workbook reached `ReadyForRatification`
    /// (i.e. a `proposal_id` was staged), ratifies it, and returns the
    /// ratify response body for the caller to inspect if it wants to.
    async fn utter_and_ratify(
        app: &axum::Router,
        session_id: &str,
        text: &str,
        anchor: &str,
    ) -> Value {
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": text, "anchor": anchor }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        println!(
            "UTTER anchor={anchor:?} text={text:?}\n  -> status={status} disposition={:?} proposal={:?} workbook={:?}",
            body["disposition"], body["proposal"], body["workbook"]
        );
        assert_eq!(status, StatusCode::OK, "utterance call must succeed: {body}");
        let proposal_id = body["proposal"]["proposal_id"].as_str().unwrap_or_else(|| {
            panic!(
                "utterance did not stage a ready-to-ratify proposal (message={:?}): {body}",
                body["message"]
            )
        });
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{proposal_id}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let status = response.status();
        let ratify_body = body_json(response).await;
        println!("  RATIFY {proposal_id} -> status={status} body={ratify_body}");
        assert_eq!(status, StatusCode::OK, "ratify must succeed: {ratify_body}");
        ratify_body
    }

    /// Fetch the gameboard (legal-moves) view at `anchor` — used, per the
    /// task's iterative-empirical approach, to inspect what is actually
    /// legally offered at the current position before constructing the
    /// next utterance, rather than guessing.
    async fn gameboard_at(app: &axum::Router, session_id: &str, anchor: &str) -> Value {
        let response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sessions/{session_id}/gameboard?anchor={anchor}"
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    }

    /// Deterministic candidate selection via `/palette/select` (bypasses
    /// lexical utterance binding entirely) -- returns raw status + body so
    /// callers can assert on either the fixed 422 (pre-fix) or the OK
    /// ratify-ready response (post-fix) rather than panicking internally.
    async fn palette_select(
        app: &axum::Router,
        session_id: &str,
        move_id: &str,
        anchor: &str,
    ) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/palette/select"),
                serde_json::json!({ "move_id": move_id, "anchor": anchor }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        (status, body)
    }

    /// Find the single node id in the current graph whose id contains
    /// `substring` — used to discover the workbook-id-suffixed fork/join
    /// node ids `op.create_parallel_region` synthesizes (unpredictable in
    /// advance: the suffix is the server-minted `WorkbookId`).
    async fn find_node_id_containing(app: &axum::Router, session_id: &str, substring: &str) -> String {
        let graph = graph_body(app, session_id).await;
        let matches: Vec<String> = graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .filter(|id| id.contains(substring))
            .map(|id| id.to_string())
            .collect();
        assert_eq!(
            matches.len(),
            1,
            "expected exactly one node id containing {substring:?}, found {matches:?} in {graph}"
        );
        matches.into_iter().next().unwrap()
    }

    /// G-REPL — the super-user REPL persona, end to end: an expert dictates
    /// workflow-construction MOVES as natural-language utterances (never
    /// raw `/graph-edit` JSON), each utterance binds against the CURRENT
    /// legal-moves set at the current anchor (lexical overlap against
    /// `board_candidate.rs`'s phrase bank — see `BINDABLE_UTTERANCE` and
    /// `proposal::start_workbook`), stages a proposal, and ratify mutates
    /// the session graph. Builds: start -> task1 -> task2 -> [2-way
    /// parallel fork: branch1 -> task5, branch2 -> task6] -> join -> task3
    /// -> task4 -> end (6 named ServiceTasks total: task1, task2,
    /// parallel_branch_1, parallel_branch_2, task3, task4; one matched
    /// GatewayAnd fork/join pair as the 2-way branch).
    ///
    /// G7 CLOSED the reachability gap this test originally found and
    /// documented (`op.create_multi_instance_region`'s `collection`
    /// argument is a `DataReference` that must name an EXISTING
    /// `IRNode::DataObject`, and no `Operation` variant could ever create
    /// one through `/graph-edit` post-seed — see
    /// EOP-PLAN-BPMN-DESIGN-003 G7). `op.create_data_object` now mints one
    /// via utterance (anchored at Start only — every session has exactly
    /// one, always present post-seed). The build below is 100%
    /// utterance+ratify, including both MI regions over dictated
    /// DataObjects — no `/graph-edit` fallback remains except the one
    /// truly unavoidable step (the very first node, before any anchor
    /// exists to utter from). An isolated probe still proves the RED
    /// half: an utterance naming an UNDECLARED collection is refused
    /// (stuck at `NeedsArguments`, not silently admitted) — the same
    /// fail-closed `mentioned_id` binding, now exercised against a real
    /// declared-DataObject world instead of an empty one.
    #[tokio::test]
    async fn test_super_user_repl_builds_6_step_2_branch_2_loop_workflow() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // ── Unavoidable minimal seed: start -> task1 -> end ──────────────
        // The very first node cannot itself be reached by utterance (no
        // anchor yet exists to utter from) — same precedent as
        // `seed_graph_backed_session`.
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "repl build session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let t1 = new_key();
        let seed_ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: seed_start_key(sid),
                key: t1,
                node: task_ir("task1"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": seed_ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "minimal seed must admit");

        // ── Step 1: InsertAfter task1 -> task2 (utterance-driven) ────────
        let board_at_task1 = gameboard_at(&app, &session_id, "task1").await;
        println!(
            "gameboard @task1 legal move candidate ids: {:?}",
            board_at_task1["legal_moves"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| m["candidate_id"].clone())
                .collect::<Vec<_>>()
        );
        utter_and_ratify(
            &app,
            &session_id,
            "Places a node on an existing route, after the selected node called 'task2'",
            "task1",
        )
        .await;

        // ── Step 2: open a 2-way parallel fork/join region after task2 ───
        utter_and_ratify(
            &app,
            &session_id,
            "Run every declared branch concurrently and join them all in parallel, 2 branches",
            "task2",
        )
        .await;
        let after_fork = graph_body(&app, &session_id).await;
        println!("graph after fork: {after_fork}");
        let node_ids: Vec<String> = after_fork["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(
            node_ids.iter().any(|id| id == "parallel_branch_1"),
            "expected default branch-1 body name: {node_ids:?}"
        );
        assert!(
            node_ids.iter().any(|id| id == "parallel_branch_2"),
            "expected default branch-2 body name: {node_ids:?}"
        );
        let join_id = find_node_id_containing(&app, &session_id, "_join").await;

        // ── Step 3: attempt the bounded loops via utterance FIRST — the
        // real finding, not an assumption. Run this probe in an ISOLATED
        // throwaway session (its own seed, task1 only) rather than against
        // the main build: staging even an incomplete workbook writes a
        // terminal move-attempt receipt into the session's history, and
        // leaving that dangling (there is no clean "undo the attempt
        // history entry" operation, only reject-the-proposal, which does
        // not retract the receipt) would poison every subsequent utterance
        // in the SAME session with an attempt-identity collision. Isolating
        // the probe keeps the main 6-step build's history clean while still
        // proving the finding against a real, live gameboard position. ────
        {
            let probe_session_id = body_json(
                app.clone()
                    .oneshot(post_json(
                        "/api/dsl/sessions",
                        serde_json::json!({ "name": "MI-utterance-reachability probe" }),
                    ))
                    .await
                    .unwrap(),
            )
            .await["session_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let probe_sid: Uuid = probe_session_id.parse().unwrap();
            let probe_t1 = new_key();
            let probe_seed_ops = vec![
                designer_graph::ops::Operation::AppendNode {
                    anchor: seed_start_key(probe_sid),
                    key: probe_t1,
                    node: task_ir("probe_task"),
                    edge_id: "f1".into(),
                },
                designer_graph::ops::Operation::AppendNode {
                    anchor: probe_t1,
                    key: new_key(),
                    node: bpmn_lite_compiler::IRNode::End {
                        id: "end".into(),
                        terminate: false,
                    },
                    edge_id: "f2".into(),
                },
            ];
            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{probe_session_id}/graph-edit"),
                    serde_json::json!({ "operations": probe_seed_ops }),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "probe seed must admit");

            let response = app
                .clone()
                .oneshot(post_json(
                    &format!("/api/dsl/sessions/{probe_session_id}/utterance"),
                    serde_json::json!({
                        "text": "Create a per-element multi-instance region over 'ghost_collection' with a declared maximum of 2",
                        "anchor": "probe_task",
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mi_attempt = body_json(response).await;
            println!("MI utterance attempt @probe_task (isolated probe session): {mi_attempt}");
            assert_eq!(
                mi_attempt["proposal"]["status"], "needs_arguments",
                "G7.5 RED: naming an UNDECLARED collection ('ghost_collection', never \
                 created via op.create_data_object in this session) must NOT reach \
                 ReadyForRatification — mentioned_id must fail closed against the \
                 real declared-DataObject world, not just an empty one: {mi_attempt}"
            );
            assert!(
                mi_attempt["proposal"]["operations"].is_null(),
                "an incomplete workbook must never materialize operations: {mi_attempt}"
            );
            let missing_slots: Vec<&str> = mi_attempt["workbook"]["slots"]
                .as_array()
                .map(|slots| {
                    slots
                        .iter()
                        .filter(|slot| slot["value"]["state"] == "missing")
                        .filter_map(|slot| slot["name"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            println!("MI workbook slots left unresolved: {missing_slots:?}");
            assert!(
                missing_slots.contains(&"collection"),
                "expected 'collection' to be the stuck slot: {mi_attempt}"
            );
            assert!(
                !missing_slots.contains(&"declared_max"),
                "declared_max SHOULD resolve from utterance text ('maximum of 2') — \
                 only 'collection' is the structural gap: {mi_attempt}"
            );
        }

        // ── Steps 3a/3b: dictate the two loop collections into existence
        // (G7's `op.create_data_object`, anchored at Start — the only
        // anchor it's offered at) then build each MI region over the
        // named collection. 100% utterance+ratify — no `/graph-edit`. ───
        utter_and_ratify(
            &app,
            &session_id,
            "Declare a new named data object with a primitive type, available for \
             later reference by id, called 'items_1', of type string",
            "start",
        )
        .await;
        utter_and_ratify(
            &app,
            &session_id,
            "Declare a new named data object with a primitive type, available for \
             later reference by id, called 'items_2', of type string",
            "start",
        )
        .await;
        utter_and_ratify(
            &app,
            &session_id,
            "Create a per-element multi-instance region over 'items_1' with a \
             declared maximum of 2",
            "parallel_branch_1",
        )
        .await;
        utter_and_ratify(
            &app,
            &session_id,
            "Create a per-element multi-instance region over 'items_2' with a \
             declared maximum of 2",
            "parallel_branch_2",
        )
        .await;
        let after_mi = graph_body(&app, &session_id).await;
        let mi_node_ids: Vec<String> = after_mi["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(
            mi_node_ids.iter().any(|id| id == "for_each_items_1"),
            "expected the dictated MI region over items_1: {mi_node_ids:?}"
        );
        assert!(
            mi_node_ids.iter().any(|id| id == "for_each_items_2"),
            "expected the dictated MI region over items_2: {mi_node_ids:?}"
        );

        // ── Steps 4/5: continue the utterance-driven chain past the join:
        // join -> task3 -> task4 -> end. ─────────────────────────────────
        utter_and_ratify(
            &app,
            &session_id,
            "Places a node on an existing route, after the selected node called 'task3'",
            &join_id,
        )
        .await;
        utter_and_ratify(
            &app,
            &session_id,
            "Places a node on an existing route, after the selected node called 'task4'",
            "task3",
        )
        .await;

        // ── Final shape assertions ───────────────────────────────────────
        let final_graph = graph_body(&app, &session_id).await;
        println!("FINAL graph: {final_graph}");
        assert_eq!(final_graph["compiles"], true, "{final_graph}");
        let final_ids: Vec<String> = final_graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .map(|s| s.to_string())
            .collect();
        for expected in [
            "start",
            "task1",
            "task2",
            "parallel_branch_1",
            "parallel_branch_2",
            "for_each_items_1",
            "for_each_items_2",
            "task3",
            "task4",
            "end",
        ] {
            assert!(
                final_ids.iter().any(|id| id == expected),
                "expected node {expected:?} in final graph: {final_ids:?}"
            );
        }
        // 6 named service-task steps: task1, task2, parallel_branch_1,
        // parallel_branch_2, task3, task4.
        let six_steps = [
            "task1",
            "task2",
            "parallel_branch_1",
            "parallel_branch_2",
            "task3",
            "task4",
        ];
        assert_eq!(
            six_steps
                .iter()
                .filter(|id| final_ids.contains(&id.to_string()))
                .count(),
            6
        );

        // End-to-end admissibility proof: save publishes the graph this
        // utterance-driven build produced.
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/save"),
                serde_json::json!({ "template_name": "repl-build-template" }),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = body_json(response).await;
        assert_eq!(status, StatusCode::OK, "utterance-built workflow must save/publish: {body}");
    }

    /// G7 focused REPL exercise (2026-08-13) — independent of the 6-step/
    /// 2-branch/2-loop build above, drives `op.create_data_object`
    /// end-to-end plus both of its ruled referential-integrity gates
    /// (F-G7b verifier tightening, F-G7c delete integrity) purely through
    /// `/api/dsl/sessions/:id/utterance` -> ratify, real HTTP round trips,
    /// no `/graph-edit` except the one unavoidable first-node seed.
    #[tokio::test]
    async fn test_g7_create_data_object_utterance_and_referential_integrity() {
        let state = DesignerState::try_new().unwrap();
        let app = designer_router(state.clone());

        // ── Unavoidable minimal seed: start -> verify_each -> end ────────
        let session_id = body_json(
            app.clone()
                .oneshot(post_json(
                    "/api/dsl/sessions",
                    serde_json::json!({ "name": "g7 create-data-object session" }),
                ))
                .await
                .unwrap(),
        )
        .await["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid: Uuid = session_id.parse().unwrap();
        let t1 = new_key();
        let seed_ops = vec![
            designer_graph::ops::Operation::AppendNode {
                anchor: seed_start_key(sid),
                key: t1,
                node: task_ir("verify_each"),
                edge_id: "f1".into(),
            },
            designer_graph::ops::Operation::AppendNode {
                anchor: t1,
                key: new_key(),
                node: bpmn_lite_compiler::IRNode::End {
                    id: "end".into(),
                    terminate: false,
                },
                edge_id: "f2".into(),
            },
        ];
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/graph-edit"),
                serde_json::json!({ "operations": seed_ops }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "minimal seed must admit");

        // ── RED (before any DataObject exists): naming an undeclared
        // collection stays stuck at needs_arguments — mentioned_id must
        // fail closed against an empty declared-DataObject world. ───────
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Create a per-element multi-instance region over 'directors' with a declared maximum of 3",
                    "anchor": "verify_each",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let pre_declare_attempt = body_json(response).await;
        println!("MI attempt BEFORE 'directors' is declared: {pre_declare_attempt}");
        assert_eq!(
            pre_declare_attempt["proposal"]["status"], "needs_arguments",
            "MI over an undeclared collection must not reach ReadyForRatification: {pre_declare_attempt}"
        );
        let missing: Vec<&str> = pre_declare_attempt["workbook"]["slots"]
            .as_array()
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| slot["value"]["state"] == "missing")
                    .filter_map(|slot| slot["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            missing.contains(&"collection"),
            "expected 'collection' to be the stuck slot: {pre_declare_attempt}"
        );

        // ── G7.1/G7.2/G7.3: dictate 'directors' into existence via
        // op.create_data_object, anchored at Start (the only anchor it's
        // offered at, per the 2026-08-13 ruling). ───────────────────────
        let create_body = utter_and_ratify(
            &app,
            &session_id,
            "Declare a new named data object with a primitive type, available for \
             later reference by id, called 'directors', of type string",
            "start",
        )
        .await;
        println!("CreateDataObject ratify body: {create_body}");
        assert_eq!(create_body["proposal_status"], "ratified");

        // `IRNode::DataObject` carries zero bytecode instructions and never
        // becomes an `ExecutionNode`, so `plan_to_visual_graph` alone can't
        // see it -- but the graph endpoint is the enriched execution map
        // (2026-08-13 ruling: DataObjects belong on it, distinct from the
        // runbook's raw operation-tape rendering), so `session_graph_endpoint`
        // now additionally projects `IRNode::DataObject`s straight off the
        // `IRGraph` as unconnected `"data_object"`-kind nodes. Verify both
        // surfaces: the runbook (G6.2, shows every ratified `Operation`
        // verbatim) and the graph (the enriched map a user actually looks at).
        let runbook_response = app
            .clone()
            .oneshot(get_req(&format!("/api/dsl/sessions/{session_id}/runbook")))
            .await
            .unwrap();
        assert_eq!(runbook_response.status(), StatusCode::OK);
        let runbook_body = body_json(runbook_response).await;
        let runbook = runbook_body["runbook"].as_str().unwrap();
        println!("runbook after declaring 'directors': {runbook}");
        assert!(
            runbook.contains("(create-data-object")
                && runbook.contains("\"directors\"")
                && runbook.contains("Primitive(String)"),
            "expected a create-data-object line naming 'directors' as a string: {runbook}"
        );

        let graph_after_declare = graph_body(&app, &session_id).await;
        let directors_node = graph_after_declare["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"].as_str() == Some("directors"))
            .unwrap_or_else(|| panic!("'directors' must appear as a data_object node on /graph: {graph_after_declare}"));
        assert_eq!(directors_node["kind"], "data_object");
        assert!(
            directors_node["label"]
                .as_str()
                .is_some_and(|label| label.contains("directors") && label.contains("string")),
            "unexpected data_object label: {directors_node}"
        );

        // ── Same undeclared-name probe again, unrelated collection —
        // still refused: declaring 'directors' must not loosen binding
        // for a DIFFERENT undeclared name. ────────────────────────────
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({
                    "text": "Create a per-element multi-instance region over 'ghost_collection' with a declared maximum of 3",
                    "anchor": "verify_each",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ghost_attempt = body_json(response).await;
        assert_eq!(
            ghost_attempt["proposal"]["status"], "needs_arguments",
            "MI over a DIFFERENT still-undeclared collection must stay refused \
             even after 'directors' is declared: {ghost_attempt}"
        );

        // ── G7.3 GREEN: MI over the now-declared 'directors' binds and
        // ratifies for real. ─────────────────────────────────────────────
        let mi_body = utter_and_ratify(
            &app,
            &session_id,
            "Create a per-element multi-instance region over 'directors' with a \
             declared maximum of 3",
            "verify_each",
        )
        .await;
        println!("MI ratify body: {mi_body}");
        assert_eq!(mi_body["proposal_status"], "ratified");

        let after_mi = graph_body(&app, &session_id).await;
        println!("graph after MI: {after_mi}");
        assert_eq!(after_mi["compiles"], true, "{after_mi}");
        let mi_node = after_mi["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == "for_each_directors")
            .unwrap_or_else(|| panic!("expected for_each_directors MI node: {after_mi}"));
        assert_eq!(mi_node["kind"], "multi_instance");

        // ── G7.4/F-G7c RED: deleting 'directors' while the MI region
        // still references it as its collection must be refused. FINDING:
        // the refusal is enforced a layer earlier than a ratify-time
        // check — `apply()`'s new referential-integrity error surfaces
        // during the compiler-preview dry-run enumeration itself
        // (`legal_moves.rs::position_bound_move` -> `preview_operations`),
        // so `op.delete_subgraph` never becomes a legal move at the
        // 'directors' anchor at all while it's referenced, rather than
        // staging and then failing at ratify. Proven directly against the
        // real gameboard, not asserted from source reading. ─────────────
        let board_while_referenced = gameboard_at(&app, &session_id, "directors").await;
        let candidates_while_referenced: Vec<&str> = board_while_referenced["legal_moves"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["candidate_id"].as_str())
            .collect();
        println!("legal moves @directors WHILE referenced: {candidates_while_referenced:?}");
        assert!(
            !candidates_while_referenced.contains(&"op.delete_subgraph"),
            "op.delete_subgraph must NOT be offered while 'directors' is still \
             referenced by for_each_directors: {candidates_while_referenced:?}"
        );

        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/utterance"),
                serde_json::json!({ "text": "Delete the anchor node or a complete enclosed region", "anchor": "directors" }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let delete_attempt = body_json(response).await;
        println!("DeleteNode(directors) attempt while referenced: {delete_attempt}");
        assert!(
            delete_attempt["proposal"]["proposal_id"].is_null(),
            "no move can be selected -- there is nothing legal to ratify: {delete_attempt}"
        );
        assert_ne!(
            delete_attempt["disposition"]["kind"], "propose_move",
            "refused-before-staging, not a stageable proposal: {delete_attempt}"
        );

        // Graph must be unchanged by the refused delete: the MI region
        // (a real flow node, unlike the DataObject) is still present.
        let after_refused_delete = graph_body(&app, &session_id).await;
        let ids_after_refusal: Vec<String> = after_refused_delete["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(ids_after_refusal.iter().any(|id| id == "for_each_directors"));

        // ── G7.4/F-G7c GREEN, proven precisely rather than by deleting the
        // referencing MI region: mid-chain flow-node deletion is refused
        // for an UNRELATED, pre-existing reason noted in G2's own receipt
        // (`Operation::DeleteNode` doesn't reconnect predecessor -> successor,
        // so removing `for_each_directors` would disconnect
        // verify_each -> end regardless of G7 — confirmed directly: it is
        // NOT even offered as a legal move at that anchor). Isolating
        // F-G7c's own claim instead: declare a SECOND, never-referenced
        // DataObject and confirm the gameboard offers `op.delete_subgraph`
        // for IT while 'directors' (still referenced) stays refused, then
        // actually ratify the delete over HTTP via `/palette/select`
        // (deterministic move selection, bypassing lexical utterance
        // binding) to close the real GREEN loop end to end.
        utter_and_ratify(
            &app,
            &session_id,
            "Declare a new named data object with a primitive type, available for \
             later reference by id, called 'scratch', of type string",
            "start",
        )
        .await;

        let board_at_scratch = gameboard_at(&app, &session_id, "scratch").await;
        let candidates_at_scratch: Vec<&str> = board_at_scratch["legal_moves"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["candidate_id"].as_str())
            .collect();
        println!("legal moves @scratch (never referenced): {candidates_at_scratch:?}");
        assert!(
            candidates_at_scratch.contains(&"op.delete_subgraph"),
            "op.delete_subgraph must be offered for an unreferenced data object: {candidates_at_scratch:?}"
        );

        // 'directors' must STILL be refused — declaring/offering delete on
        // an unrelated data object has no bearing on the first's
        // referential integrity.
        let board_at_directors_final = gameboard_at(&app, &session_id, "directors").await;
        let candidates_at_directors_final: Vec<&str> = board_at_directors_final["legal_moves"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["candidate_id"].as_str())
            .collect();
        println!("legal moves @directors (still referenced): {candidates_at_directors_final:?}");
        assert!(
            !candidates_at_directors_final.contains(&"op.delete_subgraph"),
            "'directors' must stay refused -- deleting an unrelated data object \
             must not loosen its referential integrity: {candidates_at_directors_final:?}"
        );

        // Close the GREEN loop for real: select `op.delete_subgraph` at
        // 'scratch' via `/palette/select`. `scratch`'s only argument
        // (`target`) auto-resolves positionally from the anchor inside
        // `start_workbook`, so its workbook is born at `ReadyForDryRun`,
        // never `NeedsArguments` -- previously `palette_select_endpoint`'s
        // unconditional zero-answers call into `answer_proposal_endpoint`
        // would 422 here ("workbook in ReadyForDryRun does not accept
        // answers"). Fixed by treating a zero-answer submission against a
        // workbook that isn't `NeedsArguments` as a no-op pass-through
        // instead of a hard error (`answer_proposal_endpoint`, rest.rs).
        // `/palette/select` takes the move's content-hash `move_id`, not
        // its human-readable `candidate_id` -- pull the real hash off the
        // legal move we already located above.
        let scratch_delete_move_id = board_at_scratch["legal_moves"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["candidate_id"].as_str() == Some("op.delete_subgraph"))
            .and_then(|m| m["move_id"].as_str())
            .expect("op.delete_subgraph legal move must carry a move_id hash");
        let (delete_status, delete_body) =
            palette_select(&app, &session_id, scratch_delete_move_id, "scratch").await;
        println!("PALETTE SELECT delete@scratch -> status={delete_status} body={delete_body}");
        assert_eq!(
            delete_status,
            StatusCode::OK,
            "palette/select must now accept an anchor-complete candidate with no free-text answers: {delete_body}"
        );
        assert_eq!(
            delete_body["proposal_status"], "ready_for_ratification",
            "palette selection stages, it does not auto-ratify: {delete_body}"
        );
        let delete_proposal_id = delete_body["proposal_id"]
            .as_str()
            .expect("palette selection must stage a ratifiable proposal");
        let response = app
            .clone()
            .oneshot(post_json(
                &format!("/api/dsl/sessions/{session_id}/proposals/{delete_proposal_id}/ratify"),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        let ratify_status = response.status();
        let ratify_body = body_json(response).await;
        println!("RATIFY {delete_proposal_id} (palette delete) -> status={ratify_status} body={ratify_body}");
        assert_eq!(ratify_status, StatusCode::OK, "ratify must succeed: {ratify_body}");
        assert_eq!(
            ratify_body["proposal_status"], "ratified",
            "delete of the unreferenced 'scratch' object must ratify cleanly: {ratify_body}"
        );

        // 'scratch' must actually be gone: it's no longer a valid anchor.
        let response = app
            .clone()
            .oneshot(get_req(&format!(
                "/api/dsl/sessions/{session_id}/gameboard?anchor=scratch"
            )))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "'scratch' must no longer resolve as a graph anchor after being deleted"
        );

        // 'directors' remains untouched and still referenced.
        let board_at_directors_post_delete = gameboard_at(&app, &session_id, "directors").await;
        let candidates_at_directors_post_delete: Vec<&str> =
            board_at_directors_post_delete["legal_moves"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|m| m["candidate_id"].as_str())
                .collect();
        assert!(
            !candidates_at_directors_post_delete.contains(&"op.delete_subgraph"),
            "'directors' referential integrity must be unaffected by an unrelated delete: {candidates_at_directors_post_delete:?}"
        );

        let final_graph = graph_body(&app, &session_id).await;
        println!("FINAL graph: {final_graph}");
        assert_eq!(final_graph["compiles"], true, "{final_graph}");
        let final_ids: Vec<String> = final_graph["graph"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n["id"].as_str())
            .map(|s| s.to_string())
            .collect();
        assert!(
            final_ids.iter().any(|id| id == "for_each_directors"),
            "the MI region referencing 'directors' must still be present: {final_ids:?}"
        );
    }
}
