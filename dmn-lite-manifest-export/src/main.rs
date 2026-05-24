//! Manifest export CLI — emits the v0.6 §7 manifest for dmn-lite.
//!
//! Default invocation (run from the bpmn-lite repo root):
//!
//! ```text
//! cargo run -p dmn-lite-manifest-export -- \
//!     --decisions-dir dmn-lite-decisions \
//!     --allowlist dmn-lite-decisions/manifest-allowlist.yaml \
//!     --output manifests/dmn-lite-v1.0.0.yaml \
//!     --catalogue-version v1.0.0
//! ```

use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about = "Emit dmn-lite catalogue manifest (v0.6 §7)")]
struct Cli {
    /// Directory containing `.dmn-lite` source files.
    #[arg(long)]
    decisions_dir: PathBuf,

    /// Publication allowlist YAML (per v0.6 §7.5).
    #[arg(long)]
    allowlist: PathBuf,

    /// Output manifest path.
    #[arg(long)]
    output: PathBuf,

    /// Domain id this manifest publishes — typically `dmn-lite`.
    #[arg(long, default_value = "dmn-lite")]
    domain: String,

    /// Catalogue version stamped into the manifest.
    #[arg(long, default_value = "v1.0.0")]
    catalogue_version: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let written = dmn_lite_manifest_export::export_to_path(
        &cli.decisions_dir,
        &cli.allowlist,
        &cli.output,
        &cli.domain,
        &cli.catalogue_version,
    )?;
    println!("wrote manifest to {}", written.display());
    Ok(())
}
