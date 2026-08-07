//! Tier-0 retrieval interface + the interim lexical producer (WS-C
//! C-now item 4, first half).
//!
//! `Tier0Retriever` is THE seam every retrieval producer sits behind
//! (§10.6: tier-0 is high-recall retrieval; tier-1 ranks the retrieved
//! subset). Producers registered here supply EVIDENCE ONLY (I27) — the
//! output is an `SlmResult` fed to `policy::decide`, never a
//! disposition. The Candle embed-and-score producer (ruling E3, shared
//! embedder pinned at DSL `5ac7da7`, default-off) plugs in behind this
//! same trait; per the programme ruling, the retired
//! keyword gate's successor is THIS lexical producer, retired in turn
//! when the embedder lands.

use crate::board::InferenceBoard;
use crate::contract::{rank_canonically, FiniteScore, RankedCandidate, SlmResult};

/// THE tier-1 list constructor — Adam's ruling 2026-07-27 (spec
/// EOP-SPEC-SLM-TRAIN-001 finding 5): the list tier-1 scores is the
/// tier-0 K-prefix with `NONE_OF_THE_ABOVE` ALWAYS present (appended if
/// the prefix cut it). One function for the corpus generator today and
/// the serving path in Phase C — training-list shape = serving-list
/// shape by construction, not by discipline.
/// THE standing K for `tier1_list` — ONE definition, shared by the
/// corpus generator, the eval tools, and the serving path.
///
/// History: spec EOP-SPEC-SLM-TRAIN-001 §S5 ruled K=8 and the four
/// canonical bundles were TRAINED on K=8+NOTA lists; Adam widened K
/// 8→12 (EOP-DIR-BPMN-DESIGN-003-003, ratified 2026-08-01 in the plan
/// receipt) after the recall curve showed K=8's 4% miss floor was all
/// gold at ranks 9–11 (recall@12 = 100% on the eval set). The spec's
/// recorded K=8 is the historical trained configuration and is not
/// edited; THIS constant is the live configuration.
pub const TIER1_K: usize = 12;

pub fn tier1_list(result: &SlmResult, k: usize) -> Vec<String> {
    let mut ids: Vec<String> = result
        .ranking
        .iter()
        .take(k)
        .map(|rc| rc.candidate_id.clone())
        .collect();
    if !ids.iter().any(|i| i == crate::contract::NONE_OF_THE_ABOVE) {
        ids.push(crate::contract::NONE_OF_THE_ABOVE.to_owned());
    }
    ids
}
use anyhow::Result;

/// A tier-0 retrieval producer: raw utterance + exact board in,
/// scored ranking out. Implementations MUST be deterministic for a
/// given (utterance, board) pair and MUST score only board candidates.
pub trait Tier0Retriever {
    /// Sealed identity of this producer (stands in for the model
    /// bundle hash until a tier-1 bundle exists — §10.8).
    fn bundle_identity(&self) -> String;
    fn retrieve(&self, raw_utterance: &str, board: &dyn InferenceBoard) -> Result<SlmResult>;
}

/// Deterministic lexical overlap scorer — the interim tier-0.
///
/// Scoring: lowercase token overlap between the utterance and each
/// candidate's description + canonical-id words; exact phrase match of
/// a candidate's description pins 1.0 (the pgvector matcher's
/// exact-match pin, reimplemented designer-side per the C5 trace note).
/// NONE_OF_THE_ABOVE scores the complement of the best overlap so
/// abstention is a live hypothesis, not a constant.
pub struct LexicalTier0;

fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(str::to_owned)
        .collect()
}

impl Tier0Retriever for LexicalTier0 {
    fn bundle_identity(&self) -> String {
        "tier0.lexical.v1".to_owned()
    }

    fn retrieve(&self, raw_utterance: &str, board: &dyn InferenceBoard) -> Result<SlmResult> {
        let utter_tokens = tokens(raw_utterance);
        let utter_lower = raw_utterance.trim().to_lowercase();
        let mut best_overlap = 0.0f64;
        let candidates = board.inference_candidates();
        let mut ranking: Vec<RankedCandidate> = Vec::with_capacity(candidates.len());

        for c in &candidates {
            if c.canonical_id == crate::contract::NONE_OF_THE_ABOVE {
                continue; // scored after the loop from best_overlap
            }
            let mut cand_tokens = tokens(&c.description);
            cand_tokens.extend(tokens(&c.canonical_id.replace(['.', '_'], " ")));
            let hits = utter_tokens
                .iter()
                .filter(|t| cand_tokens.contains(t))
                .count();
            let denom = utter_tokens.len().max(1);
            let mut score = hits as f64 / denom as f64;
            if !utter_lower.is_empty() && utter_lower == c.description.trim().to_lowercase() {
                score = 1.0; // exact-match pin
            }
            best_overlap = best_overlap.max(score);
            ranking.push(RankedCandidate {
                candidate_id: c.canonical_id.clone(),
                score: FiniteScore::new(score)?,
            });
        }
        // Abstention hypothesis: strong when nothing overlaps.
        ranking.push(RankedCandidate {
            candidate_id: crate::contract::NONE_OF_THE_ABOVE.to_owned(),
            score: FiniteScore::new((1.0 - best_overlap).clamp(0.0, 0.99))?,
        });
        rank_canonically(&mut ranking);

        // retrieved_subset_hash: the exact ordered id list tier-1 would see.
        let mut pre = Vec::new();
        pre.extend_from_slice(b"subset.v1:");
        for rc in &ranking {
            pre.extend_from_slice(rc.candidate_id.len().to_string().as_bytes());
            pre.push(b':');
            pre.extend_from_slice(rc.candidate_id.as_bytes());
        }
        Ok(SlmResult {
            ranking,
            retrieved_subset_hash: blake3::hash(&pre).to_hex().to_string(),
            board_hash: board.board_hash().to_string(),
            model_bundle_hash: self.bundle_identity(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{build_board, EmptyUniverse, PolicyFilter};
    use crate::policy::{decide, DispositionConfig, ProposalDisposition};
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

    /// THE pipeline-in-loop receipt (G2's re-scoped item, first light):
    /// board → tier-0 → deterministic policy → I28 record, end to end,
    /// no step skipped. A gibberish utterance abstains (OutOfScope);
    /// determinism across runs.
    #[test]
    fn full_pipeline_board_tier0_policy_record() {
        let board =
            build_board(&AllLegal, None, Some("rev0"), &EmptyUniverse, &PolicyFilter::default())
                .unwrap();
        let t0 = LexicalTier0;
        let cfg = DispositionConfig::shadow_v1();

        let utter = "zzz qqq xxyzzy nothing matches this";
        let ev1 = t0.retrieve(utter, &board).unwrap();
        let ev2 = t0.retrieve(utter, &board).unwrap();
        assert_eq!(ev1.retrieved_subset_hash, ev2.retrieved_subset_hash, "determinism");

        let (d, rec) = decide(&cfg, &board, &ev1, &crate::context::minimal("pack.none", "g-test")).unwrap();
        assert_eq!(d, ProposalDisposition::OutOfScope, "gibberish must abstain: {d:?}");
        assert_eq!(rec.board_hash, board.board_hash);
        assert_eq!(rec.model_bundle_hash, "tier0.lexical.v1");
        assert!(!rec.disposition_policy_hash.is_empty());
        assert_eq!(rec.ranking.len(), board.candidates.len());
    }

    /// K-widening receipt (Adam's 2026-08-01 ruling, 8→12): at the ONE
    /// standing constant, `tier1_list` yields exactly K entries plus
    /// NOTA appended when the prefix cut it — 13 served candidates.
    #[test]
    fn tier1_list_at_standing_k12_serves_13_with_nota() {
        assert_eq!(TIER1_K, 12, "standing K is the ratified 12 (8→12, 2026-08-01)");
        // 20 on-ranking candidates, NOTA ranked dead last (cut by any prefix).
        let mut ranking: Vec<RankedCandidate> = (0..20)
            .map(|i| RankedCandidate {
                candidate_id: format!("op.cand_{i:02}"),
                score: FiniteScore::new(1.0 - i as f64 / 100.0).unwrap(),
            })
            .collect();
        ranking.push(RankedCandidate {
            candidate_id: crate::contract::NONE_OF_THE_ABOVE.to_owned(),
            score: FiniteScore::new(0.0).unwrap(),
        });
        rank_canonically(&mut ranking);
        let result = SlmResult {
            ranking,
            retrieved_subset_hash: "h".into(),
            board_hash: "b".into(),
            model_bundle_hash: "m".into(),
            evidence_trace: None,
            inference_evidence: None,
            move_evidence: Vec::new(),
        };
        let list = tier1_list(&result, TIER1_K);
        assert_eq!(list.len(), TIER1_K + 1, "K-prefix + appended NOTA");
        assert_eq!(&list[..TIER1_K], (0..TIER1_K).map(|i| format!("op.cand_{i:02}")).collect::<Vec<_>>().as_slice());
        assert_eq!(list[TIER1_K], crate::contract::NONE_OF_THE_ABOVE);
    }

    /// Evidence stays on the board (I15 upheld by construction) and the
    /// exact-match pin scores 1.0.
    #[test]
    fn lexical_scores_are_boarded_and_exact_match_pins() {
        let board =
            build_board(&AllLegal, None, None, &EmptyUniverse, &PolicyFilter::default()).unwrap();
        let t0 = LexicalTier0;
        let exact = "Joins two existing nodes with a typed connector";
        let ev = t0.retrieve(exact, &board).unwrap();
        for rc in &ev.ranking {
            assert!(board.contains(&rc.candidate_id), "off-board evidence: {}", rc.candidate_id);
        }
        let top = &ev.ranking[0];
        assert_eq!(top.candidate_id, "op.connect");
        assert_eq!(top.score.get(), 1.0, "exact description match pins 1.0");
    }
}

/// The Candle embed-and-score tier-0 producer (ruling E3): board
/// candidates embedded ON THE FLY, cosine in memory — the pgvector
/// ranking path is not used for Designer boards. Deterministic by
/// construction: BGE weights pinned to an immutable HF revision inside
/// the shared embedder crate, CPU device, no RNG/dropout, L2-normalised
/// outputs (so cosine = dot product).
#[cfg(feature = "embed")]
pub mod embed {
    use super::*;
    use semantic_embedder::CandleEmbedder;

    pub struct EmbedTier0 {
        embedder: CandleEmbedder,
        // Performance finding (plan §F, 2026-07-27 corpus-gen receipt):
        // every board's candidate set was re-embedded on EVERY retrieve()
        // call — a corpus run reuses the same small set of enumeration-
        // class boards across hundreds of utterances, so this was
        // O(entries * board_size) forward passes for what is really
        // O(distinct_descriptions). `embed_target` is a pure function of
        // its text (matcher contract), so caching by the description
        // string is exact, not approximate — never a staleness risk.
        // Keyed by description text, not candidate_id: correctness
        // follows the actual embedder input, robust to any future world
        // where two ids might (implausibly) share a description.
        target_cache: std::sync::Mutex<std::collections::HashMap<String, Vec<f32>>>,
    }

    impl EmbedTier0 {
        /// Loads (or fetches on first use) the pinned BGE model via
        /// hf-hub — network on cold cache; deterministic thereafter.
        pub fn new() -> Result<Self> {
            Ok(EmbedTier0 {
                embedder: CandleEmbedder::new()?,
                target_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            })
        }

        /// Cached target embedding: locks only around the map, never
        /// across the (slow) forward pass on a cache miss — two threads
        /// racing on the same miss both compute once and the second
        /// write is a harmless overwrite with an identical value
        /// (`embed_target` is pure), not a correctness hazard.
        fn embed_target_cached(&self, description: &str) -> Result<Vec<f32>> {
            if let Some(v) = self.target_cache.lock().unwrap().get(description) {
                return Ok(v.clone());
            }
            let v = self.embedder.embed_target(description)?;
            self.target_cache
                .lock()
                .unwrap()
                .insert(description.to_owned(), v.clone());
            Ok(v)
        }
    }

    fn dot(a: &[f32], b: &[f32]) -> f64 {
        a.iter().zip(b).map(|(x, y)| (*x as f64) * (*y as f64)).sum()
    }

    impl Tier0Retriever for EmbedTier0 {
        fn bundle_identity(&self) -> String {
            // Model + crate pin: the embedder crate pins the BGE weights
            // to an immutable HF commit; the consuming rev is pinned in
            // Cargo.toml. Both named so records are attributable.
            format!(
                "tier0.embed.{}@semantic-embedder:5ac7da7",
                self.embedder.model_name()
            )
        }

        fn retrieve(&self, raw_utterance: &str, board: &dyn InferenceBoard) -> Result<SlmResult> {
            let query = self.embedder.embed_query(raw_utterance)?;
            let utter_lower = raw_utterance.trim().to_lowercase();
            let mut best = 0.0f64;
            let candidates = board.inference_candidates();
            let mut ranking: Vec<RankedCandidate> = Vec::with_capacity(candidates.len());
            for c in &candidates {
                if c.canonical_id == crate::contract::NONE_OF_THE_ABOVE {
                    continue;
                }
                let target = self.embed_target_cached(&c.description)?;
                // L2-normalised vectors: cosine == dot; clamp FP noise.
                let mut score = dot(&query, &target).clamp(-1.0, 1.0).max(0.0);
                if !utter_lower.is_empty() && utter_lower == c.description.trim().to_lowercase() {
                    score = 1.0; // exact-match pin (C5 note)
                }
                best = best.max(score);
                ranking.push(RankedCandidate {
                    candidate_id: c.canonical_id.clone(),
                    score: FiniteScore::new(score)?,
                });
            }
            ranking.push(RankedCandidate {
                candidate_id: crate::contract::NONE_OF_THE_ABOVE.to_owned(),
                score: FiniteScore::new((1.0 - best).clamp(0.0, 0.99))?,
            });
            rank_canonically(&mut ranking);
            let mut pre = Vec::new();
            pre.extend_from_slice(b"subset.v1:");
            for rc in &ranking {
                pre.extend_from_slice(rc.candidate_id.len().to_string().as_bytes());
                pre.push(b':');
                pre.extend_from_slice(rc.candidate_id.as_bytes());
            }
            Ok(SlmResult {
                ranking,
                retrieved_subset_hash: blake3::hash(&pre).to_hex().to_string(),
                board_hash: board.board_hash().to_string(),
                model_bundle_hash: self.bundle_identity(),
                evidence_trace: None,
                inference_evidence: None,
                move_evidence: Vec::new(),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::board::{build_board, EmptyUniverse, PolicyFilter};
        use crate::policy::{decide, DispositionConfig};
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

        /// Network/weights required (hf-hub cold cache) — run with
        /// `cargo test --features embed -- --ignored`. Same pipeline
        /// receipt as the lexical producer: retrieve twice
        /// (determinism), decide, record closes.
        #[test]
        #[ignore = "downloads pinned BGE weights on cold cache"]
        fn embed_pipeline_end_to_end_and_deterministic() {
            let board = build_board(
                &AllLegal,
                None,
                Some("rev0"),
                &EmptyUniverse,
                &PolicyFilter::default(),
            )
            .unwrap();
            let t0 = EmbedTier0::new().expect("model load");
            let ev1 = t0.retrieve("connect the review task to the end", &board).unwrap();
            let ev2 = t0.retrieve("connect the review task to the end", &board).unwrap();
            assert_eq!(ev1.retrieved_subset_hash, ev2.retrieved_subset_hash);
            for rc in &ev1.ranking {
                assert!(board.contains(&rc.candidate_id));
            }
            let (d, rec) = decide(
                &DispositionConfig::shadow_v1(),
                &board,
                &ev1,
                &crate::context::minimal("pack.none", "g-test"),
            )
            .unwrap();
            assert!(!rec.model_bundle_hash.is_empty());
            // Disposition depends on live scores; assert only that the
            // pipeline completes and records — thresholds are shadow
            // placeholders, not semantics to cement here.
            let _ = d;
        }

        /// Batch-vs-single bit-identity probe (open-queue "batch
        /// embed_target" lever, pre-implementation receipt). The matcher
        /// exposes `embed_batch_targets`/`embed_batch_queries`, but the
        /// batch path pads every sequence to the batch max and runs one
        /// forward — the exact shape where masked-softmax residue and
        /// changed matmul shapes can perturb low-order bits vs the
        /// single-text path. Serving embeds one utterance at a time, so
        /// the corpus generator may adopt batching ONLY if batch output
        /// is bit-identical to single output; otherwise batching builds
        /// train/serve skew in silently.
        ///
        /// MEASURED 2026-07-28 on the pinned rev (ff3f12c): 361/384
        /// components of the first probe vector differ bitwise between
        /// batch and single. Batching is therefore REJECTED — the
        /// generator stays on the single-call path — and this test is
        /// the tripwire recording that ruling: it PASSES while the
        /// divergence exists. If a future matcher rev makes batch output
        /// bit-identical, this test FAILS, which is the signal to reopen
        /// the batching decision on evidence, not a regression.
        #[test]
        #[ignore = "downloads pinned BGE weights on cold cache"]
        fn embed_batch_diverges_from_single_so_batching_stays_rejected() {
            let t0 = EmbedTier0::new().expect("model load");
            // Varied lengths on purpose: padding only bites when the
            // batch mixes lengths. Include real candidate descriptions
            // (the actual production inputs) plus a long outlier that
            // forces heavy padding on the short ones.
            let texts: Vec<&str> = vec![
                "Connect two existing nodes with a typed sequence flow",
                "Insert a new node after the anchor node",
                "chase",
                "Non-interrupting bounded reminder cycle with an escalation continuation",
                "For each element of a bounded collection, under a mandatory declared maximum \
                 ceiling, run the declared per-element work and join when every instance \
                 completes or the ceiling refuses admission",
            ];
            let batch_t = t0.embedder.embed_batch_targets(&texts).expect("batch targets");
            let batch_q = t0.embedder.embed_batch_queries(&texts).expect("batch queries");
            let mut diverged_components = 0usize;
            for (i, text) in texts.iter().enumerate() {
                let single_t = t0.embedder.embed_target(text).expect("single target");
                let single_q = t0.embedder.embed_query(text).expect("single query");
                assert_eq!(batch_t[i].len(), single_t.len(), "dim mismatch on text {i}");
                diverged_components += batch_t[i]
                    .iter()
                    .zip(&single_t)
                    .filter(|(b, s)| b.to_bits() != s.to_bits())
                    .count();
                diverged_components += batch_q[i]
                    .iter()
                    .zip(&single_q)
                    .filter(|(b, s)| b.to_bits() != s.to_bits())
                    .count();
            }
            assert!(
                diverged_components > 0,
                "batch and single embeddings are now BIT-IDENTICAL on this matcher rev — \
                 the 2026-07-28 rejection of corpus-generator batching rested on their \
                 divergence and must be reopened (batching may now be admissible)"
            );
        }

        /// Cache correctness + performance receipt (plan §F, corpus-gen
        /// finding, 2026-07-27). Exactness: two retrieves over the SAME
        /// board (identical descriptions) must yield bit-identical
        /// rankings whether the second call hits a warm cache or not —
        /// caching must never perturb scores. Performance: with the
        /// cache warm, a repeat retrieve over the same board must be
        /// markedly faster than the first (cold) one — proves the cache
        /// is actually short-circuiting forward passes, not merely
        /// present in the type.
        #[test]
        #[ignore = "downloads pinned BGE weights on cold cache"]
        fn embed_target_cache_is_exact_and_faster_on_repeat() {
            let board = build_board(
                &AllLegal,
                None,
                Some("rev0"),
                &EmptyUniverse,
                &PolicyFilter::default(),
            )
            .unwrap();
            let t0 = EmbedTier0::new().expect("model load");

            let t_cold = std::time::Instant::now();
            let ev1 = t0.retrieve("connect the review task to the end", &board).unwrap();
            let cold_elapsed = t_cold.elapsed();

            // Cache now warm for every description on this board; a
            // DIFFERENT utterance still exercises embed_target_cached
            // (only embed_query differs — the expensive per-candidate
            // target passes are all cache hits).
            let t_warm = std::time::Instant::now();
            let ev2 = t0.retrieve("insert a step before this node", &board).unwrap();
            let warm_elapsed = t_warm.elapsed();

            assert!(
                warm_elapsed < cold_elapsed,
                "warm-cache retrieve ({warm_elapsed:?}) must beat cold ({cold_elapsed:?}) — \
                 cache is not short-circuiting target embedding"
            );

            // Exactness: re-run the FIRST utterance now that the cache is
            // warm; scores must match the cold run bit-for-bit.
            let ev1_warm = t0.retrieve("connect the review task to the end", &board).unwrap();
            assert_eq!(
                ev1.retrieved_subset_hash, ev1_warm.retrieved_subset_hash,
                "cached and uncached target embeddings must rank identically"
            );
            for (a, b) in ev1.ranking.iter().zip(ev1_warm.ranking.iter()) {
                assert_eq!(a.candidate_id, b.candidate_id);
                assert_eq!(a.score.get().to_bits(), b.score.get().to_bits(), "bit-exact score");
            }
            let _ = ev2;
        }
    }
}
