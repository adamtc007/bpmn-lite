//! `cargo xtask fuzz` — unified cargo-fuzz runner (EOP-FUZZ-BPMN-ISA-002 F1).
//!
//! Discovers per-crate `<crate>/fuzz/` cargo-fuzz projects (fork F-C),
//! invokes them on the nightly toolchain (repo pin is stable 1.95), and
//! captures per-run receipts under `fuzz-results/<unix-ts>/`.
//!
//! Fail-closed discipline: a missing nightly toolchain or cargo-fuzz binary
//! is a hard error with install instructions, never a silent skip — a fuzz
//! gate that doesn't run is not a gate.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;

const DEFAULT_RUN_SECS: u64 = 300;
const SMOKE_RUN_SECS: u64 = 60;

pub(crate) fn fuzz_command(root: &Path, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("list") => list(root, &args[1..]),
        Some("run") => {
            let (target, secs) = parse_run_args(&args[1..], DEFAULT_RUN_SECS)?;
            run_targets(root, target.as_deref(), secs, RunMode::Fuzz)
        }
        Some("smoke") => smoke(root),
        Some("regress") => run_targets(root, None, 0, RunMode::RegressOnly),
        Some("seed") => seed(root),
        Some("clean") => clean(root),
        _ => {
            eprintln!(
                "Usage:
  cargo xtask fuzz list [--json]
  cargo xtask fuzz run [--target NAME] [--time SECS]   (default {DEFAULT_RUN_SECS}s per target)
  cargo xtask fuzz smoke                                (build all + regress + {SMOKE_RUN_SECS}s per target)
  cargo xtask fuzz regress                              (committed regression inputs only)
  cargo xtask fuzz seed                                 (regenerate compiled-artifact seed corpora)
  cargo xtask fuzz clean                                (delete fuzz target dirs + evolved corpora)"
            );
            bail!("missing or unknown fuzz subcommand");
        }
    }
}

/// Regenerate seed corpora that require real compiled artifacts: without a
/// verifier-admitted seed, `artifact_verify` would have to discover exact
/// canonical JSON by mutation alone and its deep `verify_program` branches
/// would stay cold. Seeds are committed; re-run after ISA/ABI changes.
fn seed(root: &Path) -> Result<()> {
    const FIXTURES: &[(&str, &str)] = &[
        (
            "linear",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_seed_linear" isExecutable="true">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Do Work">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="do_work" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#,
        ),
        (
            "linear_budget",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<bpmn:definitions xmlns:bpmn="http://www.omg.org/spec/BPMN/20100524/MODEL"
                  xmlns:zeebe="http://camunda.org/schema/zeebe/1.0">
  <bpmn:process id="fuzz_seed_budget" isExecutable="true" defaultFailureBudget="2">
    <bpmn:startEvent id="start" />
    <bpmn:serviceTask id="task1" name="Do Work">
      <bpmn:extensionElements>
        <zeebe:taskDefinition type="do_work" />
      </bpmn:extensionElements>
    </bpmn:serviceTask>
    <bpmn:endEvent id="end" />
    <bpmn:sequenceFlow id="f1" sourceRef="start" targetRef="task1" />
    <bpmn:sequenceFlow id="f2" sourceRef="task1" targetRef="end" />
  </bpmn:process>
</bpmn:definitions>"#,
        ),
    ];

    let seeds_dir = root.join("bpmn-lite-types/fuzz/seeds/artifact_verify");
    fs::create_dir_all(&seeds_dir).context("create artifact_verify seeds dir")?;
    for (name, xml) in FIXTURES {
        let (ir, meta) = bpmn_lite_compiler::parse_bpmn_with_meta(xml)
            .map_err(|e| anyhow!("parse fixture '{name}': {e}"))?;
        let workflow =
            bpmn_lite_compiler::Compiler::lower_with_default(&ir, meta.default_failure_budget)
                .map_err(|e| anyhow!("lower fixture '{name}': {e}"))?;
        let bytes = workflow
            .canonical_bytes()
            .map_err(|e| anyhow!("encode fixture '{name}': {e}"))?;
        let path = seeds_dir.join(format!("{name}.artifact.json"));
        fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
        println!("seeded {} ({} bytes)", path.display(), bytes.len());
    }

    // F7 covering-array seeds for engine_graph: the enumerated
    // local-logic-alphabet corpus, written by the fuzz crate's own
    // (ignored) seed-writer test so the encoder stays next to the grammar
    // it mirrors.
    println!("== generating engine_graph covering seeds ==");
    let status = Command::new("cargo")
        .arg("test")
        .arg("--manifest-path")
        .arg(root.join("bpmn-lite-engine/fuzz/Cargo.toml"))
        .arg("write_covering_seeds")
        .arg("--")
        .arg("--ignored")
        .current_dir(root)
        .status()
        .context("spawn covering-seed writer")?;
    if !status.success() {
        bail!("covering-seed writer failed ({status})");
    }

    // F8.1 xml_compile seeds: one well-formed BPMN document per covering
    // single, so libFuzzer mutates from valid structure instead of
    // rediscovering XML syntax.
    println!("== generating xml_compile seeds ==");
    let status = Command::new("cargo")
        .arg("test")
        .arg("--manifest-path")
        .arg(root.join("bpmn-lite-engine/fuzz/Cargo.toml"))
        .arg("write_xml_seeds")
        .arg("--")
        .arg("--ignored")
        .current_dir(root)
        .status()
        .context("spawn xml-seed writer")?;
    if !status.success() {
        bail!("xml-seed writer failed ({status})");
    }
    Ok(())
}

fn parse_run_args(args: &[String], default_secs: u64) -> Result<(Option<String>, u64)> {
    let mut target = None;
    let mut secs = default_secs;
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                target = Some(
                    args.get(i + 1)
                        .ok_or_else(|| anyhow!("--target requires a value"))?
                        .clone(),
                );
                i += 2;
            }
            "--time" => {
                secs = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--time requires a value"))?
                    .parse()
                    .context("invalid --time value")?;
                i += 2;
            }
            other => bail!("unknown fuzz run argument '{other}'"),
        }
    }
    Ok((target, secs))
}

#[derive(Clone, Copy, PartialEq)]
enum RunMode {
    Fuzz,
    RegressOnly,
}

struct FuzzProject {
    crate_name: String,
    fuzz_dir: PathBuf,
    targets: Vec<String>,
}

struct TargetOutcome {
    crate_name: String,
    target: String,
    mode: &'static str,
    duration_secs: f64,
    execs: Option<u64>,
    cov_edges: Option<u64>,
    corpus_entries: Option<u64>,
    crashed: bool,
    crash_artifacts: Vec<String>,
    log_path: PathBuf,
}

#[derive(Serialize)]
struct MatrixTarget {
    crate_name: String,
    target: String,
    fuzz_dir: String,
}

fn list(root: &Path, args: &[String]) -> Result<()> {
    let json = match args {
        [] => false,
        [arg] if arg == "--json" => true,
        _ => bail!("usage: cargo xtask fuzz list [--json]"),
    };
    let projects = discover(root)?;
    if json {
        let targets = projects
            .into_iter()
            .flat_map(|project| {
                let fuzz_dir = project
                    .fuzz_dir
                    .strip_prefix(root)
                    .unwrap_or(&project.fuzz_dir)
                    .to_string_lossy()
                    .into_owned();
                project.targets.into_iter().map(move |target| MatrixTarget {
                    crate_name: project.crate_name.clone(),
                    target,
                    fuzz_dir: fuzz_dir.clone(),
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string(&targets)?);
        return Ok(());
    }

    for project in projects {
        println!("{} ({})", project.crate_name, project.fuzz_dir.display());
        for target in &project.targets {
            let regressions = count_files(&project.fuzz_dir.join("regressions").join(target));
            let seeds = count_files(&project.fuzz_dir.join("seeds").join(target));
            println!("  {target}  [regressions: {regressions}, seeds: {seeds}]");
        }
    }
    Ok(())
}

fn smoke(root: &Path) -> Result<()> {
    ensure_nightly_cargo_fuzz()?;
    let projects = discover(root)?;
    for project in &projects {
        verify_fuzz_lock(root, &project.fuzz_dir)?;
        println!("== building all fuzz targets in {} ==", project.crate_name);
        let status = nightly_fuzz_cmd(root, &project.fuzz_dir, "build", None)
            .status()
            .context("failed to spawn cargo +nightly fuzz build")?;
        if !status.success() {
            bail!("fuzz build failed for {} ({})", project.crate_name, status);
        }
    }
    // Regression pass first (deterministic, fast), then the time-boxed runs.
    run_targets(root, None, 0, RunMode::RegressOnly)?;
    run_targets(root, None, SMOKE_RUN_SECS, RunMode::Fuzz)
}

fn run_targets(root: &Path, only_target: Option<&str>, secs: u64, mode: RunMode) -> Result<()> {
    ensure_nightly_cargo_fuzz()?;
    let projects = discover(root)?;
    let results_dir = new_results_dir(root, mode)?;
    let mut outcomes = Vec::new();
    let mut matched = false;

    for project in &projects {
        let project_selected = only_target
            .map(|only| project.targets.iter().any(|target| target == only))
            .unwrap_or(true);
        if !project_selected {
            continue;
        }
        verify_fuzz_lock(root, &project.fuzz_dir)?;
        for target in &project.targets {
            if let Some(only) = only_target {
                if target != only {
                    continue;
                }
            }
            matched = true;
            let outcome = match mode {
                RunMode::Fuzz => run_one(root, project, target, secs, &results_dir)?,
                RunMode::RegressOnly => regress_one(root, project, target, &results_dir)?,
            };
            if let Some(outcome) = outcome {
                outcomes.push(outcome);
            }
        }
    }

    if !matched {
        bail!(
            "no fuzz target matched '{}' — run `cargo xtask fuzz list`",
            only_target.unwrap_or("<all>")
        );
    }

    if mode == RunMode::RegressOnly && outcomes.is_empty() {
        write_summary(&results_dir, &outcomes)?;
        bail!(
            "no committed fuzz regression inputs were executed — an empty permanent gate is a failure"
        );
    }

    write_summary(&results_dir, &outcomes)?;
    let crashed: Vec<_> = outcomes.iter().filter(|o| o.crashed).collect();
    println!(
        "\nfuzz results: {} target-run(s), {} crash(es); receipts in {}",
        outcomes.len(),
        crashed.len(),
        results_dir.display()
    );
    if !crashed.is_empty() {
        for outcome in &crashed {
            eprintln!(
                "CRASH {}::{} — artifacts: {}",
                outcome.crate_name,
                outcome.target,
                outcome.crash_artifacts.join(", ")
            );
        }
        bail!("fuzzing found {} crashing target(s)", crashed.len());
    }
    write_completion_receipt(&results_dir, &outcomes)?;
    Ok(())
}

fn write_completion_receipt(results_dir: &Path, outcomes: &[TargetOutcome]) -> Result<()> {
    let mut completed = outcomes
        .iter()
        .map(|outcome| outcome.target.as_str())
        .collect::<Vec<_>>();
    completed.sort_unstable();
    completed.dedup();
    let path = results_dir.join("completed-targets.txt");
    fs::write(&path, format!("{}\n", completed.join("\n")))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn run_one(
    root: &Path,
    project: &FuzzProject,
    target: &str,
    secs: u64,
    results_dir: &Path,
) -> Result<Option<TargetOutcome>> {
    let corpus_dir = project.fuzz_dir.join("corpus").join(target);
    fs::create_dir_all(&corpus_dir).context("create corpus dir")?;
    let seeds_dir = project.fuzz_dir.join("seeds").join(target);

    let mut cmd = nightly_fuzz_cmd(root, &project.fuzz_dir, "run", Some(target));
    cmd.arg(&corpus_dir);
    if seeds_dir.is_dir() && count_files(&seeds_dir) > 0 {
        cmd.arg(&seeds_dir);
    }
    cmd.arg("--")
        .arg(format!("-max_total_time={secs}"))
        .arg("-print_final_stats=1");

    println!("== fuzz {}::{} ({secs}s) ==", project.crate_name, target);
    Ok(Some(execute_and_record(
        cmd,
        project,
        target,
        "fuzz",
        results_dir,
    )?))
}

fn regress_one(
    root: &Path,
    project: &FuzzProject,
    target: &str,
    results_dir: &Path,
) -> Result<Option<TargetOutcome>> {
    let regressions_dir = project.fuzz_dir.join("regressions").join(target);
    if count_files(&regressions_dir) == 0 {
        println!(
            "== regress {}::{} — no regression inputs recorded yet (dir empty) ==",
            project.crate_name, target
        );
        return Ok(None);
    }
    let mut cmd = nightly_fuzz_cmd(root, &project.fuzz_dir, "run", Some(target));
    cmd.arg(&regressions_dir)
        .arg("--")
        .arg("-runs=0")
        .arg("-print_final_stats=1");
    println!("== regress {}::{} ==", project.crate_name, target);
    Ok(Some(execute_and_record(
        cmd,
        project,
        target,
        "regress",
        results_dir,
    )?))
}

fn execute_and_record(
    mut cmd: Command,
    project: &FuzzProject,
    target: &str,
    mode: &'static str,
    results_dir: &Path,
) -> Result<TargetOutcome> {
    let started = Instant::now();
    let output = cmd
        .output()
        .context("failed to spawn cargo +nightly fuzz run")?;
    let duration_secs = started.elapsed().as_secs_f64();
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let log_path = results_dir.join(format!("{}-{}-{}.log", project.crate_name, target, mode));
    fs::write(&log_path, format!("{stdout}\n{stderr}"))
        .with_context(|| format!("write {}", log_path.display()))?;

    // libFuzzer emits `artifact_prefix='...'; Test unit written to <path>`
    // — the marker is mid-line, so scan, don't prefix-match.
    const MARKER: &str = "Test unit written to ";
    let crash_artifacts: Vec<String> = stderr
        .lines()
        .filter_map(|line| {
            line.find(MARKER)
                .map(|idx| line[idx + MARKER.len()..].trim().to_string())
        })
        .collect();
    let crashed = !output.status.success();

    let outcome = TargetOutcome {
        crate_name: project.crate_name.clone(),
        target: target.to_string(),
        mode,
        duration_secs,
        execs: parse_stat(&stderr, "stat::number_of_executed_units:"),
        cov_edges: parse_last_field(&stderr, "cov:"),
        corpus_entries: parse_last_field(&stderr, "corp:"),
        crashed,
        crash_artifacts,
        log_path,
    };
    println!(
        "   {} — {:.0}s, execs: {}, cov: {}{}",
        if crashed { "CRASH" } else { "ok" },
        outcome.duration_secs,
        outcome.execs.map_or("?".into(), |n| n.to_string()),
        outcome.cov_edges.map_or("?".into(), |n| n.to_string()),
        if crashed {
            format!(" — see {}", outcome.log_path.display())
        } else {
            String::new()
        }
    );
    Ok(outcome)
}

fn write_summary(results_dir: &Path, outcomes: &[TargetOutcome]) -> Result<()> {
    let mut summary = String::from(
        "# fuzz run summary\n\n| crate | target | mode | secs | execs | cov | corpus | result |\n|---|---|---|---|---|---|---|---|\n",
    );
    for o in outcomes {
        summary.push_str(&format!(
            "| {} | {} | {} | {:.0} | {} | {} | {} | {} |\n",
            o.crate_name,
            o.target,
            o.mode,
            o.duration_secs,
            o.execs.map_or("?".into(), |n| n.to_string()),
            o.cov_edges.map_or("?".into(), |n| n.to_string()),
            o.corpus_entries.map_or("?".into(), |n| n.to_string()),
            if o.crashed {
                format!("CRASH ({})", o.crash_artifacts.join(", "))
            } else {
                "ok".to_string()
            }
        ));
    }
    let path = results_dir.join("summary.md");
    fs::write(&path, summary).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn clean(root: &Path) -> Result<()> {
    for project in discover(root)? {
        for sub in ["target", "corpus", "coverage"] {
            let dir = project.fuzz_dir.join(sub);
            if dir.exists() {
                fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
                println!("removed {}", dir.display());
            }
        }
        // artifacts/ (crash evidence) and regressions/ (committed cement) are
        // deliberately NOT cleaned.
    }
    Ok(())
}

fn discover(root: &Path) -> Result<Vec<FuzzProject>> {
    let mut projects = Vec::new();
    for entry in fs::read_dir(root).context("read workspace root")? {
        let crate_dir = entry?.path();
        let manifest = crate_dir.join("fuzz").join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let toml = fs::read_to_string(&manifest)
            .with_context(|| format!("read {}", manifest.display()))?;
        let targets = parse_bin_names(&toml);
        if targets.is_empty() {
            bail!(
                "fuzz project {} declares no [[bin]] targets",
                manifest.display()
            );
        }
        projects.push(FuzzProject {
            crate_name: crate_dir
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow!("unreadable crate dir name"))?
                .to_string(),
            fuzz_dir: crate_dir.join("fuzz"),
            targets,
        });
    }
    if projects.is_empty() {
        bail!(
            "no <crate>/fuzz/Cargo.toml projects found — nothing to fuzz is a failure, not a pass"
        );
    }
    projects.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    Ok(projects)
}

/// Minimal `[[bin]] name = "..."` extractor — cargo-fuzz manifests are
/// machine-written and flat; a TOML dependency is not warranted for this.
fn parse_bin_names(toml: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin = false;
    for line in toml.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if in_bin {
            if let Some(rest) = line.strip_prefix("name") {
                let rest = rest.trim_start().strip_prefix('=').unwrap_or("").trim();
                let name = rest.trim_matches('"');
                if !name.is_empty() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

fn nightly_fuzz_cmd(
    root: &Path,
    fuzz_dir: &Path,
    subcommand: &str,
    target: Option<&str>,
) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.arg("+nightly")
        .arg("fuzz")
        .arg(subcommand)
        .arg("--fuzz-dir")
        .arg(fuzz_dir)
        .current_dir(root);
    if let Some(target) = target {
        cmd.arg(target);
    }
    cmd
}

/// cargo-fuzz 0.13 does not expose Cargo's `--locked` switch. Perform an
/// explicit locked metadata resolution before every selected fuzz project so a
/// stale independently-resolved fuzz lockfile fails before build or execution.
fn verify_fuzz_lock(root: &Path, fuzz_dir: &Path) -> Result<()> {
    let status = Command::new("cargo")
        .arg("+nightly")
        .arg("metadata")
        .arg("--locked")
        .arg("--no-deps")
        .arg("--format-version")
        .arg("1")
        .arg("--manifest-path")
        .arg(fuzz_dir.join("Cargo.toml"))
        .stdout(Stdio::null())
        .current_dir(root)
        .status()
        .with_context(|| format!("validate locked fuzz project {}", fuzz_dir.display()))?;
    if !status.success() {
        bail!(
            "fuzz lockfile is stale for {} — regenerate and commit it before running fuzz gates",
            fuzz_dir.display()
        );
    }
    Ok(())
}

fn ensure_nightly_cargo_fuzz() -> Result<()> {
    let ok = Command::new("cargo")
        .arg("+nightly")
        .arg("fuzz")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        bail!(
            "cargo +nightly fuzz is unavailable (the workspace pins stable {}; fuzzing needs nightly).\n\
             Install with:\n  rustup toolchain install nightly\n  cargo install cargo-fuzz",
            "1.95"
        );
    }
    Ok(())
}

fn new_results_dir(root: &Path, mode: RunMode) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock before unix epoch")?
        .as_secs();
    let suffix = match mode {
        RunMode::Fuzz => "run",
        RunMode::RegressOnly => "regress",
    };
    let dir = root.join("fuzz-results").join(format!("{stamp}-{suffix}"));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn count_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| {
                    e.path().is_file() && e.file_name().to_str().map_or(true, |n| n != ".gitkeep")
                })
                .count()
        })
        .unwrap_or(0)
}

/// Parse `stat::<key>: N` from libFuzzer's `-print_final_stats=1` output.
fn parse_stat(stderr: &str, key: &str) -> Option<u64> {
    stderr
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(key))
        .and_then(|rest| rest.trim().parse().ok())
}

/// Parse the last status-line field of the form `<key> N` (e.g. `cov: 123`,
/// `corp: 56/789b` — the leading integer).
fn parse_last_field(stderr: &str, key: &str) -> Option<u64> {
    for line in stderr.lines().rev() {
        if let Some(idx) = line.find(key) {
            let rest = line[idx + key.len()..].trim_start();
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}
