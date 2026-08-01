//! DIR-002 Phase C's remaining step ("verify each [bundle] loads and
//! scores in Candle behind the SlmResult contract") + Phase D's real
//! uplift number: loads a fine-tuned bundle from `train_py/bundles/<key>/`
//! (produced by `train_py/train_slm.py`) into candle-transformers, and
//! scores it against the enriched eval set (`eval_enrich.rs`) so its
//! top-1 accuracy can sit next to the C5 baseline (tier-0 alone,
//! recall@8 = 0.9592 on this same eval slice — see
//! synthetic-v2-beta.eval_enriched.card.json).
//!
//! Key-layout note (why this ISN'T a copy of candle_loadability_probe.rs):
//! that probe loads a HF checkpoint's OWN weights via `ModernBert::load`/
//! `BertModel::load`/`XLMRobertaModel::new`, which expect each
//! architecture's real published key layout. `train_slm.py` exports a
//! DIFFERENT, uniform layout instead (`encoder.*` + `head.weight`/
//! `head.bias`, flat, chosen so one Python script serves four
//! architectures identically) — plain AutoModel-loaded encoders have no
//! "model."/"bert."/"roberta." prefix of their own, so:
//!   - ModernBert::load hardcodes an internal "model." segment regardless
//!     of the vb it's given -> strip "encoder." then ADD "model." back.
//!   - BertModel::load / XLMRobertaModel::new use the given vb's root
//!     directly (no hardcoded segment) -> strip "encoder.", add nothing.
//! Verified against the actual saved tensor names in a completed bundle
//! before writing this, not assumed.
//!
//! Run: `cargo run -p utterance-engine --example score_trained_bundle
//! --features candle-probe --release -- <base-key|all>`

use anyhow::{anyhow, Context, Result};
use candle_core::Device;
use utterance_engine::corpus_schema::Example;
use utterance_engine::trained_ranker::{Base, TrainedRanker};

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let arg = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    let bases: Vec<Base> = if arg == "all" {
        Base::ALL.to_vec()
    } else {
        vec![Base::from_key(&arg).ok_or_else(|| anyhow!("unknown base key '{arg}'"))?]
    };

    let eval_path = root.join("seed/corpus_v2/synthetic-v2-beta.eval_enriched.jsonl");
    let eval_records: Vec<Example> = std::fs::read_to_string(&eval_path)
        .with_context(|| format!("{eval_path:?} -- run eval_enrich first"))?
        .lines()
        .map(|l| serde_json::from_str(l).map_err(Into::into))
        .collect::<Result<_>>()?;

    // The apples-to-apples baseline (eval_enrich.rs's own receipt):
    // tier1_list[0] is the tier-0 retriever's OWN rank-ordered #1 pick.
    // recall@K is a much easier bar and is NEVER the uplift comparison.
    let card_path = root.join("seed/corpus_v2/synthetic-v2-beta.eval_enriched.card.json");
    let card: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&card_path)?)
        .with_context(|| format!("{card_path:?} -- run eval_enrich first"))?;
    let tier0_top1_accuracy = card["totals"]["tier0_top1_accuracy"]
        .as_f64()
        .context("card missing totals.tier0_top1_accuracy -- regenerate with the current eval_enrich.rs")?;
    println!("baseline: tier0_top1_accuracy (C5, tier-0 alone) = {tier0_top1_accuracy:.4}\n");

    let device = Device::Cpu;

    // Latency-vs-K probe (close-out suite): `<base-key> latency` times
    // the identical scoring path at three served-list sizes — the ruled
    // legacy trained K=8+NOTA (9), the ruled K=12+NOTA (13), and the full board
    // (upper bound) — over the whole eval set. Widened lists take the
    // tier-0 ranking's next candidates in rank order (exactly what a
    // widened tier1_list would contain); the full-board list is the
    // board in ranking order. CPU device deliberately: serving latency
    // is the CPU story, per the T3.4a criteria.
    if std::env::args().nth(2).as_deref() == Some("latency") {
        let base = bases[0];
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;
        for &(label, k) in &[("K=8+NOTA (legacy trained)", 9usize), ("K=12+NOTA (ruled 2026-08-01)", 13), ("full board", usize::MAX)] {
            let mut times_ms: Vec<f64> = Vec::with_capacity(eval_records.len());
            for record in &eval_records {
                // Widen from the board's candidate list in board order —
                // the enriched record stores candidates in the retriever's
                // canonical order via tier1_list for the first 9; beyond
                // that, take remaining board candidates not already listed.
                let mut ids: Vec<String> = record.tier1_list.clone();
                for c in &record.board.candidates {
                    if ids.len() >= k {
                        break;
                    }
                    if !ids.contains(&c.canonical_id) {
                        ids.push(c.canonical_id.clone());
                    }
                }
                let t = std::time::Instant::now();
                let _ = ranker.score_list(record, &ids, &device)?;
                times_ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
            let p95 = times_ms[(times_ms.len() as f64 * 0.95) as usize - 1];
            let actual_k = eval_records
                .first()
                .map(|r| {
                    let mut ids = r.tier1_list.len();
                    ids += r.board.candidates.iter().filter(|c| !r.tier1_list.contains(&c.canonical_id)).count();
                    ids.min(k)
                })
                .unwrap_or(0);
            println!(
                "  {} ({}): list_len={} mean={mean:.1}ms p95={p95:.1}ms over {} utterances",
                base.key(),
                label,
                actual_k,
                times_ms.len()
            );
        }
        return Ok(());
    }

    // Ambiguity-set score-separation suite (A2.5): `<base-key> ambiguity`.
    // For each constructed-ambiguous pair, softmax the two logits and
    // measure the winner-margin |p_a - p_b|; for the regular eval set,
    // softmax the full served list and measure top1-top2 margin. A model
    // that has learned real context-conditioning should show MARKEDLY
    // smaller margins on the constructed-ambiguous pairs (feeding the
    // disposition policy's clarification path) than on ordinary
    // examples — confidently splitting an unlabelable pair is the
    // failure mode this suite exists to catch. Raw logits, no
    // calibration: margins are compared within one model only.
    if std::env::args().nth(2).as_deref() == Some("ambiguity") {
        let base = bases[0];
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;

        let amb_path = root.join("seed/corpus_v2/synthetic-v2-beta.ambiguity_enriched.jsonl");
        let amb_records: Vec<Example> = std::fs::read_to_string(&amb_path)
            .with_context(|| format!("{amb_path:?} -- run eval_enrich first"))?
            .lines()
            .map(|l| serde_json::from_str(l).map_err(Into::into))
            .collect::<Result<_>>()?;

        let softmax_margin = |logits: &[f64]| -> f64 {
            let max = logits.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let exps: Vec<f64> = logits.iter().map(|l| (l - max).exp()).collect();
            let sum: f64 = exps.iter().sum();
            let mut probs: Vec<f64> = exps.iter().map(|e| e / sum).collect();
            probs.sort_by(|a, b| b.partial_cmp(a).unwrap());
            probs[0] - probs.get(1).copied().unwrap_or(0.0)
        };

        let mut amb_margins: Vec<f64> = Vec::new();
        for record in &amb_records {
            let result = ranker.score_list(record, &record.tier1_list, &device)?;
            let logits: Vec<f64> = result.ranking.iter().map(|rc| rc.score.get()).collect();
            amb_margins.push(softmax_margin(&logits));
        }
        let mut eval_margins: Vec<f64> = Vec::new();
        for record in &eval_records {
            let result = ranker.score(record, &device)?;
            let logits: Vec<f64> = result.ranking.iter().map(|rc| rc.score.get()).collect();
            eval_margins.push(softmax_margin(&logits));
        }
        let stats = |v: &mut Vec<f64>| -> (f64, f64) {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            (mean, v[v.len() / 2])
        };
        let (amb_mean, amb_median) = stats(&mut amb_margins);
        let (eval_mean, eval_median) = stats(&mut eval_margins);
        // Overlap check: what fraction of ambiguity pairs are MORE
        // confidently split than the median ordinary example?
        let confidently_split = amb_margins.iter().filter(|&&m| m > eval_median).count();
        println!("=== ambiguity-set score separation: {} ===", base.key());
        println!(
            "  ambiguity pairs (n={}): margin mean={amb_mean:.4} median={amb_median:.4}",
            amb_margins.len()
        );
        println!(
            "  ordinary eval  (n={}): margin mean={eval_mean:.4} median={eval_median:.4}",
            eval_margins.len()
        );
        println!(
            "  ambiguity pairs split more confidently than the ordinary-eval median: {}/{}",
            confidently_split,
            amb_margins.len()
        );
        let out = serde_json::json!({
            "base": base.key(),
            "bundle_identity": ranker.bundle_identity(),
            "note": "margins are softmax(top1)-softmax(top2) within one model; ambiguity pairs use the 2-candidate constructed pair, ordinary eval uses the full served list. No calibration applied (A4 calibration is a separate close-out item); cross-model margin comparison is NOT valid from this file.",
            "ambiguity": {"n": amb_margins.len(), "margin_mean": amb_mean, "margin_median": amb_median},
            "ordinary_eval": {"n": eval_margins.len(), "margin_mean": eval_mean, "margin_median": eval_median},
            "ambiguity_pairs_split_more_confidently_than_eval_median": confidently_split,
        });
        let out_path = root.join(format!("train_py/bundles/{}/ambiguity_scores.json", base.key()));
        std::fs::write(&out_path, serde_json::to_string_pretty(&out)? + "\n")?;
        println!("written: {out_path:?}");
        return Ok(());
    }

    let mut summary = Vec::new();
    for base in bases {
        let bundle_dir = root.join("train_py/bundles").join(base.key());
        if !bundle_dir.join("model.safetensors").exists() {
            println!("SKIP {} -- no bundle yet (train_slm.py hasn't finished this base)", base.key());
            continue;
        }
        println!("=== scoring {} ===", base.key());
        let ranker = TrainedRanker::load(base, &bundle_dir, &device)?;

        let mut end_to_end_correct = 0u32;
        let mut given_inclusion_correct = 0u32;
        let mut given_inclusion_total = 0u32;
        // Per-class accuracy: the position-invariance proxy (§10.7).
        // family_id is "class::label"; the class IS the graph position,
        // so a label that scores well at one anchor kind and collapses
        // at another shows up here as class-level variance.
        let mut per_class: std::collections::BTreeMap<String, (u32, u32)> =
            std::collections::BTreeMap::new();
        for record in &eval_records {
            let result = ranker.score(record, &device)?;
            let top1 = &result.ranking[0].candidate_id;
            let class = record
                .family_id
                .split("::")
                .next()
                .unwrap_or("unknown")
                .to_string();
            let slot = per_class.entry(class).or_insert((0, 0));
            slot.1 += 1;
            if *top1 == record.label {
                end_to_end_correct += 1;
                slot.0 += 1;
            }
            if record.gold_in_tier1 {
                given_inclusion_total += 1;
                if *top1 == record.label {
                    given_inclusion_correct += 1;
                }
            }
        }
        let per_class_json: std::collections::BTreeMap<String, serde_json::Value> = per_class
            .iter()
            .map(|(k, (hits, total))| {
                (
                    k.clone(),
                    serde_json::json!({"correct": hits, "total": total, "accuracy": *hits as f64 / (*total).max(1) as f64}),
                )
            })
            .collect();
        let n = eval_records.len().max(1) as f64;
        let end_to_end = end_to_end_correct as f64 / n;
        let given_inclusion = given_inclusion_correct as f64 / given_inclusion_total.max(1) as f64;
        let uplift_pp = (end_to_end - tier0_top1_accuracy) * 100.0;
        println!(
            "  {} : top1_end_to_end={end_to_end:.4} ({end_to_end_correct}/{}) top1_given_inclusion={given_inclusion:.4} ({given_inclusion_correct}/{given_inclusion_total}) uplift_vs_tier0_top1={uplift_pp:+.1}pp",
            base.key(),
            eval_records.len(),
        );
        summary.push(serde_json::json!({
            "base": base.key(),
            "bundle_identity": ranker.bundle_identity(),
            "n_eval": eval_records.len(),
            "tier0_top1_accuracy_baseline": tier0_top1_accuracy,
            "top1_end_to_end": end_to_end,
            "top1_end_to_end_correct": end_to_end_correct,
            "uplift_vs_tier0_top1_pp": uplift_pp,
            "top1_given_inclusion": given_inclusion,
            "top1_given_inclusion_correct": given_inclusion_correct,
            "top1_given_inclusion_total": given_inclusion_total,
            "per_class_accuracy": per_class_json,
        }));
    }

    let out_path = root.join("train_py/bundles/eval_scores.json");
    std::fs::write(&out_path, serde_json::to_string_pretty(&summary)? + "\n")?;
    println!("scores written: {out_path:?}");
    Ok(())
}
