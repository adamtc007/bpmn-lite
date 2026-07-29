//! DIR-004 Phase 1.4 — adjudication tooling for `starter-seed-v1`'s
//! disputed items.
//!
//! Every disputed item in `seed/banks/starter_seed_v1.json` carries a
//! provisional hypothesis label plus a note naming the specific alternate
//! reading (EOP-DIR-BPMN-DESIGN-003-003 Phase 3). This binary replays
//! each disputed item -- utterance, category, the real board's legal
//! candidate options, current hypothesis, dispute note -- for a HUMAN
//! verdict. It never guesses one itself: run non-interactively (no TTY,
//! or `--list`), it only prints the disputed set and writes nothing.
//! There is no code path that fabricates or defaults a verdict.
//!
//! Adjudications are written to a SEPARATE file
//! (`seed/banks/starter_seed_v1_adjudications.json`, keyed by `seq`) —
//! the original human-authored bank is never rewritten by this tool.
//! `examples/starter_seed_eval.rs` merges this file in at run time
//! (provenance `adam-adjudicated` for adjudicated items, `disputed`
//! cleared), so scoring the suite after adjudication reflects Adam's
//! verdicts without a second, hand-edited copy of the bank.
//!
//! No feature flags needed -- board construction only, no tier-0/tier-1.
//!
//! Run:
//!   cargo run -p utterance-engine --example adjudicate_starter_seed
//!     --release                       interactive (TTY only)
//!   cargo run -p utterance-engine --example adjudicate_starter_seed
//!     --release -- --list             print disputed items, write
//!                                     nothing, works without a TTY

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use designer_graph::positional::PositionalLegality;
use utterance_engine::board::{build_board, EmptyUniverse, PolicyFilter};
use utterance_engine::fixtures::enumeration_classes;

#[derive(serde::Deserialize)]
struct StarterItem {
    seq: u32,
    category: String,
    class_id: String,
    label: String,
    text: String,
    disputed: bool,
    dispute_note: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Adjudication {
    seq: u32,
    adjudicated_label: String,
    /// Unix seconds — deliberately no chrono dependency for one field.
    adjudicated_at_unix: u64,
    note: String,
    provenance: String,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let bank_path = root.join("seed/banks/starter_seed_v1.json");
    let adj_path = root.join("seed/banks/starter_seed_v1_adjudications.json");

    let items: Vec<StarterItem> = serde_json::from_str(&std::fs::read_to_string(&bank_path)?)
        .with_context(|| format!("{bank_path:?}"))?;
    let disputed: Vec<&StarterItem> = items.iter().filter(|i| i.disputed).collect();

    let mut existing: Vec<Adjudication> = if adj_path.exists() {
        serde_json::from_str(&std::fs::read_to_string(&adj_path)?).with_context(|| format!("{adj_path:?}"))?
    } else {
        Vec::new()
    };
    let already: BTreeMap<u32, Adjudication> = existing.iter().map(|a| (a.seq, a.clone())).collect();

    // Real boards, same construction as starter_seed_eval.rs -- the
    // human sees ACTUAL legal candidates, not a hand-typed list that
    // could drift from the board.
    let classes = enumeration_classes()?;
    let mut boards: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new(); // class_id -> [(canonical_id, description)]
    for c in &classes {
        let oracle = PositionalLegality { dag: &c.dag };
        let anchor_pair = c.anchor_key.as_ref().map(|k| (k, c.anchor_id.unwrap()));
        let board = build_board(&oracle, anchor_pair, None, &EmptyUniverse, &PolicyFilter::default())?;
        boards.insert(
            c.class_id,
            board.candidates.iter().map(|cd| (cd.canonical_id.clone(), cd.description.clone())).collect(),
        );
    }

    let list_only = std::env::args().any(|a| a == "--list") || !std::io::stdin().is_terminal();

    println!(
        "starter-seed-v1 adjudication: {} disputed item(s), {} already adjudicated{}",
        disputed.len(),
        already.len(),
        if list_only { " -- LIST MODE, nothing will be written" } else { "" }
    );
    println!();

    for item in &disputed {
        let legal = boards
            .get(item.class_id.as_str())
            .ok_or_else(|| anyhow!("starter item names unknown class '{}'", item.class_id))?;
        println!("--- seq {} [{}] class={} {}", item.seq, item.category, item.class_id, if already.contains_key(&item.seq) { "(ALREADY ADJUDICATED)" } else { "" });
        println!("  utterance: {:?}", item.text);
        println!("  current hypothesis: {}", item.label);
        println!("  dispute note: {}", item.dispute_note);
        println!("  legal board candidates:");
        for (i, (id, desc)) in legal.iter().enumerate() {
            println!("    {i:2}. {id:35} {desc}");
        }
        if let Some(a) = already.get(&item.seq) {
            println!("  already adjudicated: {} ({})", a.adjudicated_label, a.note);
            println!();
            continue;
        }

        if list_only {
            println!();
            continue;
        }

        print!("  verdict [index number / 'k' keep hypothesis / 's' skip]: ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        let n = std::io::stdin().read_line(&mut line).unwrap_or(0);
        let line = line.trim();
        if n == 0 {
            // EOF mid-run: stop asking, write nothing further, never guess.
            println!("  (EOF on stdin — stopping, no verdict recorded for this or remaining items)");
            break;
        }
        let chosen_label: Option<String> = match line {
            "s" | "S" | "" => None,
            "k" | "K" => Some(item.label.clone()),
            other => match other.parse::<usize>() {
                Ok(i) if i < legal.len() => Some(legal[i].0.clone()),
                _ => {
                    println!("  (unrecognised input {other:?} — skipping this item, no verdict recorded)");
                    None
                }
            },
        };
        if let Some(label) = chosen_label {
            print!("  one-line note for the record (why): ");
            std::io::stdout().flush().ok();
            let mut note = String::new();
            std::io::stdin().read_line(&mut note).ok();
            existing.push(Adjudication {
                seq: item.seq,
                adjudicated_label: label,
                adjudicated_at_unix: unix_now(),
                note: note.trim().to_string(),
                provenance: "adam-adjudicated".to_string(),
            });
            std::fs::write(&adj_path, serde_json::to_string_pretty(&existing)? + "\n")?;
            println!("  recorded.");
        }
        println!();
    }

    if list_only {
        println!("LIST MODE: nothing written. Run interactively (a real TTY, no --list) to adjudicate.");
    } else {
        println!("Adjudication file: {adj_path:?} ({} total entries)", existing.len());
    }
    Ok(())
}
