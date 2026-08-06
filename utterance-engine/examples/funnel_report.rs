//! WS-1.3 CLI: print the decomposed funnel report for a Q9 capture
//! directory as JSON.
//!
//!   cargo run -p utterance-engine --features q9-capture \
//!     --example funnel_report -- <capture-dir>

#[cfg(feature = "q9-capture")]
fn main() -> anyhow::Result<()> {
    let dir = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: funnel_report <capture-dir>"))?;
    let report = utterance_engine::funnel::funnel_from_capture_dir(std::path::Path::new(&dir))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

#[cfg(not(feature = "q9-capture"))]
fn main() {
    eprintln!("funnel_report requires --features q9-capture (DIR-004 structural separation)");
    std::process::exit(2);
}
