use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

const DEFAULT_DOCKER_IMAGE: &str = "bpmn-lite-server:local";
const DEFAULT_INSTANCE_NAME: &str = "default";
const DEFAULT_DOCKER_SERVER_PORT: u16 = 50071;
const DEFAULT_DOCKER_DB_PORT: u16 = 5541;
const DEFAULT_DOCKER_DB_NAME: &str = "bpmn_lite";

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        print_help();
        bail!("missing xtask subcommand");
    }

    match args[0].as_str() {
        "smoke" => run_profile("smoke", &args[1..]),
        "stress" => run_profile("stress", &args[1..]),
        "poison-jobs" => run_profile("poison", &args[1..]),
        "subscription-fanout" => run_profile("subscription", &args[1..]),
        "docker-smoke" => run_docker_profile("smoke", &args[1..]),
        "docker-stress" => run_docker_profile("stress", &args[1..]),
        "docker-poison-jobs" => run_docker_profile("poison", &args[1..]),
        "docker-subscription-fanout" => run_docker_profile("subscription", &args[1..]),
        "docker-ha-stress" => run_docker_ha_profile("stress", &args[1..]),
        "docker-ha-subscription-fanout" => run_docker_ha_profile("subscription", &args[1..]),
        "docker-up" => docker_up_command(&args[1..]),
        "docker-down" => docker_down_command(&args[1..]),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => bail!("unknown xtask subcommand '{}'", other),
    }
}

fn run_profile(profile: &str, extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;

    let server_url = parsed.server_url.clone().unwrap_or_else(|| {
        if parsed.spawn_server {
            "http://127.0.0.1:50061".to_string()
        } else {
            "http://127.0.0.1:50051".to_string()
        }
    });

    let mut server_child = if parsed.spawn_server {
        Some(ChildGuard::new(spawn_server(
            &workspace_root,
            &server_url,
            parsed.database_url.as_deref(),
        )?))
    } else {
        None
    };

    if parsed.spawn_server {
        wait_for_server(&server_url, Duration::from_secs(20))?;
    }

    let status = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server")
        .arg("--bin")
        .arg("load_harness")
        .arg("--")
        .arg("--profile")
        .arg(profile)
        .arg("--server-url")
        .arg(&server_url)
        .args(&parsed.forward_args)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run load harness")?;

    if let Some(child) = &mut server_child {
        child.stop();
    }

    if !status.success() {
        bail!("load harness exited with {}", status);
    }

    Ok(())
}

fn run_docker_profile(profile: &str, extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;
    let deployment = docker_up(&workspace_root, &parsed)?;
    let server_url = parsed
        .server_url
        .clone()
        .unwrap_or_else(|| deployment.server_url.clone());

    let result = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server")
        .arg("--bin")
        .arg("load_harness")
        .arg("--")
        .arg("--profile")
        .arg(profile)
        .arg("--server-url")
        .arg(&server_url)
        .args(&parsed.forward_args)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run load harness against docker deployment")
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                bail!("load harness exited with {}", status)
            }
        });

    if !parsed.keep_running {
        let cleanup = docker_down_deployment(&deployment);
        result.and(cleanup)?;
    } else {
        result?;
    }

    Ok(())
}

fn run_docker_ha_profile(profile: &str, extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;
    let deployment = docker_up(&workspace_root, &parsed)?;
    let replicas = parsed.server_replicas.unwrap_or(2).max(2);
    let mut extra_containers = Vec::new();

    let setup_result = (|| -> Result<()> {
        for replica_idx in 2..=replicas {
            let name = format!(
                "bpmn-lite-svc-{}-r{}",
                deployment.instance_name, replica_idx
            );
            remove_container_if_exists(&name)?;
            let port = deployment.server_port + replica_idx as u16 - 1;
            let database_url = format!(
                "postgresql://postgres@{}/{}",
                deployment.db_container_name, deployment.db_name
            );
            run_command(
                Command::new("docker")
                    .arg("run")
                    .arg("-d")
                    .arg("--name")
                    .arg(&name)
                    .arg("--network")
                    .arg(&deployment.network_name)
                    .arg("-p")
                    .arg(format!("{port}:50051"))
                    .arg("-e")
                    .arg(format!("DATABASE_URL={database_url}"))
                    .arg("-e")
                    .arg(format!("BPMN_LITE_SCHEDULER_OWNER={name}"))
                    .arg("-e")
                    .arg("RUST_LOG=info")
                    .arg(&deployment.image),
            )?;
            extra_containers.push(name);
        }
        for replica_idx in 2..=replicas {
            let name = format!(
                "bpmn-lite-svc-{}-r{}",
                deployment.instance_name, replica_idx
            );
            let port = deployment.server_port + replica_idx as u16 - 1;
            wait_for_server(&format!("http://127.0.0.1:{port}"), Duration::from_secs(30))
                .with_context(|| format!("timed out waiting for HA replica '{}'", name))?;
        }
        Ok(())
    })();

    if let Err(error) = setup_result {
        cleanup_extra_containers(&extra_containers)?;
        docker_down_deployment(&deployment)?;
        return Err(error);
    }

    let mut harness_args = parsed.forward_args.clone();
    if profile == "subscription" && !has_forward_arg(&harness_args, "--subscription-server-url") {
        harness_args.push("--subscription-server-url".to_string());
        harness_args.push(format!("http://127.0.0.1:{}", deployment.server_port + 1));
    }

    let result = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server")
        .arg("--bin")
        .arg("load_harness")
        .arg("--")
        .arg("--profile")
        .arg(profile)
        .arg("--server-url")
        .arg(&deployment.server_url)
        .args(&harness_args)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run load harness against HA docker deployment")
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                bail!("load harness exited with {}", status)
            }
        });

    if !parsed.keep_running {
        let cleanup = cleanup_extra_containers(&extra_containers)
            .and_then(|_| docker_down_deployment(&deployment));
        result.and(cleanup)?;
    } else {
        result?;
    }
    Ok(())
}

fn docker_up_command(extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;
    let deployment = docker_up(&workspace_root, &parsed)?;
    println!("instance_name={}", deployment.instance_name);
    println!("server_url={}", deployment.server_url);
    println!("database_url={}", deployment.host_database_url);
    println!("docker_image={}", deployment.image);
    Ok(())
}

fn docker_down_command(extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;
    docker_down(&workspace_root, &parsed)
}

fn spawn_server(
    workspace_root: &Path,
    server_url: &str,
    database_url: Option<&str>,
) -> Result<Child> {
    let bind_addr = extract_bind_addr(server_url)?;
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server")
        .current_dir(workspace_root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env("RUST_LOG", "info")
        .env("BPMN_LITE_BIND", &bind_addr);

    if let Some(url) = database_url {
        command.arg("--features").arg("postgres");
        command.env("DATABASE_URL", url);
    } else {
        command.env("BPMN_LITE_STORE", "memory");
    }
    command.arg("--bin").arg("bpmn-lite-server");

    let child = command.spawn().with_context(|| {
        format!(
            "failed to spawn bpmn-lite-server for load harness on {}",
            bind_addr
        )
    })?;
    Ok(child)
}

fn wait_for_server(server_url: &str, timeout: Duration) -> Result<()> {
    let addr = extract_host_port(server_url)?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(&addr).is_ok() {
            return Ok(());
        }
        sleep(Duration::from_millis(200));
    }
    bail!("timed out waiting for BPMN-Lite server at {}", server_url);
}

fn extract_host_port(server_url: &str) -> Result<String> {
    let stripped = server_url
        .strip_prefix("http://")
        .or_else(|| server_url.strip_prefix("https://"))
        .ok_or_else(|| anyhow!("server url must start with http:// or https://"))?;
    Ok(stripped.to_string())
}

fn extract_port(server_url: &str) -> Result<u16> {
    let host_port = extract_host_port(server_url)?;
    let port = host_port
        .rsplit(':')
        .next()
        .ok_or_else(|| anyhow!("missing port in server url"))?;
    port.parse::<u16>()
        .with_context(|| format!("invalid port in {}", server_url))
}

fn extract_bind_addr(server_url: &str) -> Result<String> {
    Ok(format!("0.0.0.0:{}", extract_port(server_url)?))
}

fn workspace_root() -> Result<PathBuf> {
    std::env::current_dir().context("failed to read current directory")
}

fn parse_args(extra_args: &[String]) -> Result<ParsedArgs> {
    let mut parsed = ParsedArgs::default();
    let mut forward_args = Vec::new();

    let mut i = 0usize;
    while i < extra_args.len() {
        let arg = &extra_args[i];
        match arg.as_str() {
            "--server-url" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--server-url requires a value"))?;
                parsed.server_url = Some(value.clone());
                forward_args.push(arg.clone());
                forward_args.push(value.clone());
                i += 2;
            }
            "--database-url" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--database-url requires a value"))?;
                parsed.database_url = Some(value.clone());
                i += 2;
            }
            "--spawn-server" => {
                parsed.spawn_server = true;
                i += 1;
            }
            "--instance-name" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--instance-name requires a value"))?;
                parsed.instance_name = Some(value.clone());
                i += 2;
            }
            "--server-port" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--server-port requires a value"))?;
                parsed.server_port = Some(
                    value
                        .parse()
                        .with_context(|| format!("invalid --server-port '{}'", value))?,
                );
                i += 2;
            }
            "--db-port" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--db-port requires a value"))?;
                parsed.db_port = Some(
                    value
                        .parse()
                        .with_context(|| format!("invalid --db-port '{}'", value))?,
                );
                i += 2;
            }
            "--db-name" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--db-name requires a value"))?;
                parsed.db_name = Some(value.clone());
                i += 2;
            }
            "--docker-image" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--docker-image requires a value"))?;
                parsed.docker_image = Some(value.clone());
                i += 2;
            }
            "--keep-running" => {
                parsed.keep_running = true;
                i += 1;
            }
            "--skip-build" => {
                parsed.skip_build = true;
                i += 1;
            }
            "--server-replicas" => {
                let value = extra_args
                    .get(i + 1)
                    .ok_or_else(|| anyhow!("--server-replicas requires a value"))?;
                parsed.server_replicas = Some(
                    value
                        .parse()
                        .with_context(|| format!("invalid --server-replicas '{}'", value))?,
                );
                i += 2;
            }
            other => {
                forward_args.push(other.to_string());
                i += 1;
            }
        }
    }

    parsed.forward_args = forward_args;
    Ok(parsed)
}

fn docker_up(workspace_root: &Path, parsed: &ParsedArgs) -> Result<DockerDeployment> {
    ensure_docker_available()?;

    let instance_name = sanitize_instance_name(
        parsed
            .instance_name
            .as_deref()
            .unwrap_or(DEFAULT_INSTANCE_NAME),
    );
    let server_port = parsed.server_port.unwrap_or(DEFAULT_DOCKER_SERVER_PORT);
    let db_port = parsed.db_port.unwrap_or(DEFAULT_DOCKER_DB_PORT);
    let db_name = parsed
        .db_name
        .clone()
        .unwrap_or_else(|| DEFAULT_DOCKER_DB_NAME.to_string());
    let image = parsed
        .docker_image
        .clone()
        .unwrap_or_else(|| DEFAULT_DOCKER_IMAGE.to_string());

    if !parsed.skip_build {
        ensure_docker_image(workspace_root, &image)?;
    }

    let network_name = format!("bpmn-lite-net-{instance_name}");
    let db_container_name = format!("bpmn-lite-db-{instance_name}");
    let server_container_name = format!("bpmn-lite-svc-{instance_name}");
    let volume_name = format!("bpmn-lite-pgdata-{instance_name}");
    let deployment = DockerDeployment {
        instance_name,
        image,
        server_url: parsed
            .server_url
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{server_port}")),
        server_port,
        db_name: db_name.clone(),
        host_database_url: format!("postgresql://postgres@127.0.0.1:{db_port}/{db_name}"),
        db_container_name,
        server_container_name,
        network_name,
        volume_name,
    };

    remove_container_if_exists(&deployment.db_container_name)?;
    remove_container_if_exists(&deployment.server_container_name)?;
    remove_containers_with_prefix(&format!("bpmn-lite-svc-{}-r", deployment.instance_name))?;
    remove_network_if_exists(&deployment.network_name)?;
    remove_volume_if_exists(&deployment.volume_name)?;

    let result = (|| -> Result<()> {
        run_command(
            Command::new("docker")
                .arg("network")
                .arg("create")
                .arg(&deployment.network_name),
        )?;

        run_command(
            Command::new("docker")
                .arg("volume")
                .arg("create")
                .arg(&deployment.volume_name),
        )?;

        run_command(
            Command::new("docker")
                .arg("run")
                .arg("-d")
                .arg("--name")
                .arg(&deployment.db_container_name)
                .arg("--network")
                .arg(&deployment.network_name)
                .arg("-p")
                .arg(format!("{db_port}:5432"))
                .arg("-e")
                .arg("POSTGRES_HOST_AUTH_METHOD=trust")
                .arg("-e")
                .arg(format!("POSTGRES_DB={db_name}"))
                .arg("-v")
                .arg(format!(
                    "{}:/var/lib/postgresql/data",
                    deployment.volume_name
                ))
                .arg("postgres:16-bookworm"),
        )?;

        wait_for_postgres(
            &deployment.db_container_name,
            &db_name,
            Duration::from_secs(30),
        )?;

        let container_database_url = format!(
            "postgresql://postgres@{}/{}",
            deployment.db_container_name, db_name
        );

        run_command(
            Command::new("docker")
                .arg("run")
                .arg("-d")
                .arg("--name")
                .arg(&deployment.server_container_name)
                .arg("--network")
                .arg(&deployment.network_name)
                .arg("-p")
                .arg(format!("{server_port}:50051"))
                .arg("-e")
                .arg(format!("DATABASE_URL={container_database_url}"))
                .arg("-e")
                .arg(format!(
                    "BPMN_LITE_SCHEDULER_OWNER={}",
                    deployment.server_container_name
                ))
                .arg("-e")
                .arg("RUST_LOG=info")
                .arg(&deployment.image),
        )?;

        wait_for_server(&deployment.server_url, Duration::from_secs(30))?;
        Ok(())
    })();

    if let Err(error) = result {
        let _ = docker_down_deployment(&deployment);
        return Err(error);
    }

    Ok(deployment)
}

fn docker_down(_workspace_root: &Path, parsed: &ParsedArgs) -> Result<()> {
    ensure_docker_available()?;
    let instance_name = sanitize_instance_name(
        parsed
            .instance_name
            .as_deref()
            .unwrap_or(DEFAULT_INSTANCE_NAME),
    );

    remove_container_if_exists(&format!("bpmn-lite-svc-{instance_name}"))?;
    remove_container_if_exists(&format!("bpmn-lite-db-{instance_name}"))?;
    remove_containers_with_prefix(&format!("bpmn-lite-svc-{instance_name}-r"))?;
    remove_network_if_exists(&format!("bpmn-lite-net-{instance_name}"))?;
    remove_volume_if_exists(&format!("bpmn-lite-pgdata-{instance_name}"))?;
    Ok(())
}

fn docker_down_deployment(deployment: &DockerDeployment) -> Result<()> {
    remove_container_if_exists(&deployment.server_container_name)?;
    remove_container_if_exists(&deployment.db_container_name)?;
    remove_containers_with_prefix(&format!("bpmn-lite-svc-{}-r", deployment.instance_name))?;
    remove_network_if_exists(&deployment.network_name)?;
    remove_volume_if_exists(&deployment.volume_name)?;
    Ok(())
}

fn cleanup_extra_containers(names: &[String]) -> Result<()> {
    for name in names {
        remove_container_if_exists(name)?;
    }
    Ok(())
}

fn ensure_docker_available() -> Result<()> {
    run_command(
        Command::new("docker")
            .arg("version")
            .arg("--format")
            .arg("{{.Server.Version}}"),
    )
    .context("docker is required for docker-* xtask commands")
}

fn ensure_docker_image(workspace_root: &Path, image: &str) -> Result<()> {
    let repo_root = workspace_root
        .parent()
        .ok_or_else(|| anyhow!("failed to locate repo root from bpmn-lite workspace"))?;
    run_command(
        Command::new("docker")
            .arg("build")
            .arg("-t")
            .arg(image)
            .arg("-f")
            .arg("bpmn-lite/Dockerfile")
            .arg(".")
            .current_dir(repo_root),
    )
    .with_context(|| format!("failed to build docker image '{}'", image))
}

fn wait_for_postgres(container_name: &str, db_name: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let status = Command::new("docker")
            .arg("exec")
            .arg(container_name)
            .arg("pg_isready")
            .arg("-U")
            .arg("postgres")
            .arg("-d")
            .arg(db_name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Ok(status) = status {
            if status.success() {
                return Ok(());
            }
        }
        sleep(Duration::from_millis(300));
    }
    bail!(
        "timed out waiting for PostgreSQL container '{}' to become ready",
        container_name
    );
}

fn remove_container_if_exists(name: &str) -> Result<()> {
    let _ = Command::new("docker")
        .arg("rm")
        .arg("-f")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn remove_network_if_exists(name: &str) -> Result<()> {
    let _ = Command::new("docker")
        .arg("network")
        .arg("rm")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn remove_volume_if_exists(name: &str) -> Result<()> {
    let _ = Command::new("docker")
        .arg("volume")
        .arg("rm")
        .arg("-f")
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

fn remove_containers_with_prefix(prefix: &str) -> Result<()> {
    let output = Command::new("docker")
        .arg("ps")
        .arg("-a")
        .arg("--format")
        .arg("{{.Names}}")
        .output()
        .context("failed to list docker containers")?;
    if !output.status.success() {
        return Ok(());
    }
    let names = String::from_utf8_lossy(&output.stdout);
    for name in names.lines().filter(|name| name.starts_with(prefix)) {
        remove_container_if_exists(name)?;
    }
    Ok(())
}

fn run_command(command: &mut Command) -> Result<()> {
    let status = command.status().context("failed to spawn command")?;
    if !status.success() {
        bail!("command exited with {}", status);
    }
    Ok(())
}

fn sanitize_instance_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if ch == '-' || ch == '_' {
            out.push('-');
        }
    }
    if out.is_empty() {
        DEFAULT_INSTANCE_NAME.to_string()
    } else {
        out
    }
}

fn has_forward_arg(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn print_help() {
    eprintln!(
        "Usage:
  cargo run -p xtask -- smoke [--spawn-server] [--database-url URL] [harness args...]
  cargo run -p xtask -- stress [--spawn-server] [--database-url URL] [harness args...]
  cargo run -p xtask -- poison-jobs [--spawn-server] [--database-url URL] [harness args...]
  cargo run -p xtask -- subscription-fanout [--spawn-server] [--database-url URL] [harness args...]
  cargo run -p xtask -- docker-up [--instance-name NAME] [--server-port PORT] [--db-port PORT]
  cargo run -p xtask -- docker-down [--instance-name NAME]
  cargo run -p xtask -- docker-smoke [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--keep-running] [harness args...]
  cargo run -p xtask -- docker-stress [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--keep-running] [harness args...]
  cargo run -p xtask -- docker-poison-jobs [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--keep-running] [harness args...]
  cargo run -p xtask -- docker-subscription-fanout [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--keep-running] [harness args...]
  cargo run -p xtask -- docker-ha-stress [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--server-replicas N] [--keep-running] [harness args...]
  cargo run -p xtask -- docker-ha-subscription-fanout [--instance-name NAME] [--server-port PORT] [--db-port PORT] [--server-replicas N] [--keep-running] [harness args...]

Examples:
  cargo run -p xtask -- smoke --spawn-server
  cargo run -p xtask -- stress --spawn-server --instances 500 --workers 24
  cargo run -p xtask -- poison-jobs --spawn-server --instances 50 --workers 8
  cargo run -p xtask -- subscription-fanout --spawn-server --instances 24 --subscriptions-per-instance 3
  cargo run -p xtask -- stress --server-url http://127.0.0.1:50051 --timeout-secs 180
  cargo run -p xtask -- docker-up --instance-name alpha --server-port 50071 --db-port 5541
  cargo run -p xtask -- docker-smoke --instance-name alpha --server-port 50071 --db-port 5541
  cargo run -p xtask -- docker-stress --instance-name beta --server-port 50072 --db-port 5542 --instances 200
  cargo run -p xtask -- docker-subscription-fanout --instance-name sub --server-port 50073 --db-port 5543
  cargo run -p xtask -- docker-ha-stress --instance-name ha --server-port 50100 --db-port 5550 --server-replicas 2 --instances 200
  cargo run -p xtask -- docker-ha-subscription-fanout --instance-name hasub --server-port 50110 --db-port 5560 --server-replicas 2"
    );
}

#[derive(Default)]
struct ParsedArgs {
    server_url: Option<String>,
    database_url: Option<String>,
    spawn_server: bool,
    instance_name: Option<String>,
    server_port: Option<u16>,
    db_port: Option<u16>,
    db_name: Option<String>,
    docker_image: Option<String>,
    server_replicas: Option<usize>,
    keep_running: bool,
    skip_build: bool,
    forward_args: Vec<String>,
}

struct DockerDeployment {
    instance_name: String,
    image: String,
    server_url: String,
    server_port: u16,
    db_name: String,
    host_database_url: String,
    #[allow(dead_code)]
    db_container_name: String,
    #[allow(dead_code)]
    server_container_name: String,
    #[allow(dead_code)]
    network_name: String,
    #[allow(dead_code)]
    volume_name: String,
}
