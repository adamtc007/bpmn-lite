use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

mod fuzz;

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
        "docker-ffi-smoke" => docker_ffi_smoke_command(&args[1..]),
        "docker-http-smoke" => docker_http_smoke_command(&args[1..]),
        "docker-heterogeneous-smoke" => docker_heterogeneous_smoke_command(&args[1..]),
        "docker-grpc-smoke" => docker_grpc_smoke_command(&args[1..]),
        "docker-ha-stress" => run_docker_ha_profile("stress", &args[1..]),
        "docker-ha-subscription-fanout" => run_docker_ha_profile("subscription", &args[1..]),
        "docker-up" => docker_up_command(&args[1..]),
        "docker-down" => docker_down_command(&args[1..]),
        "pack-build" => {
            if args.len() < 2 {
                bail!("missing domain argument. Usage: cargo xtask pack-build <domain>");
            }
            pack_build_command(&args[1])
        }
        "pack-check" => {
            if args.len() < 2 {
                bail!("missing domain argument. Usage: cargo xtask pack-check <domain>");
            }
            pack_check_command(&args[1])
        }
        "run-pack" => run_pack_command(),
        "fuzz" => fuzz::fuzz_command(&workspace_root()?, &args[1..]),
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
        .arg("bpmn-lite-server-runner")
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

const DEFAULT_HTTP_TARGET_IMAGE: &str = "bpmn-lite-http-target:local";
const DEFAULT_HTTP_TARGET_PORT: u16 = 8080;
const DEFAULT_GRPC_TARGET_IMAGE: &str = "bpmn-lite-grpc-target:local";
const DEFAULT_GRPC_TARGET_PORT: u16 = 50099;

fn docker_http_smoke_command(extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;

    // Build the http-test-target image from the same workspace context.
    if !parsed.skip_build {
        run_command(
            Command::new("docker")
                .arg("build")
                .arg("-t")
                .arg(DEFAULT_HTTP_TARGET_IMAGE)
                .arg("--target")
                .arg("builder")
                .arg("--build-arg")
                .arg("BINARY=http_test_target")
                .arg(".")
                .current_dir(&workspace_root),
        )
        .unwrap_or(());
        ensure_docker_image(&workspace_root, DEFAULT_DOCKER_IMAGE)?;
        // Build http-test-target image using a dedicated Dockerfile.
        build_http_target_image(&workspace_root)?;
    }

    let deployment = docker_up(&workspace_root, &parsed)?;
    let server_url = parsed
        .server_url
        .clone()
        .unwrap_or_else(|| deployment.server_url.clone());

    // Spin up the http-test-target container on the same network.
    let http_container = format!("bpmn-lite-http-target-{}", deployment.instance_name);
    remove_container_if_exists(&http_container)?;
    run_command(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg(&http_container)
            .arg("--network")
            .arg(&deployment.network_name)
            .arg("-p")
            .arg(format!(
                "{}:{}",
                DEFAULT_HTTP_TARGET_PORT, DEFAULT_HTTP_TARGET_PORT
            ))
            .arg(DEFAULT_HTTP_TARGET_IMAGE),
    )?;

    // Wait for http-test-target health.
    let http_target_url = format!("http://{}:{}", http_container, DEFAULT_HTTP_TARGET_PORT);
    wait_for_http_target(DEFAULT_HTTP_TARGET_PORT, Duration::from_secs(15))?;

    let result = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server-runner")
        .arg("--bin")
        .arg("http_proof")
        .arg("--")
        .arg("--server-url")
        .arg(&server_url)
        .arg("--http-target-url")
        .arg(&http_target_url)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run http_proof")
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                bail!("http_proof exited with {}", s)
            }
        });

    let _ = run_command(
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(&http_container),
    );
    let cleanup = docker_down_deployment(&deployment);
    result.and(cleanup)?;
    Ok(())
}

fn docker_grpc_smoke_command(extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;

    if !parsed.skip_build {
        ensure_docker_image(&workspace_root, DEFAULT_DOCKER_IMAGE)?;
        build_grpc_target_image(&workspace_root)?;
    }

    let deployment = docker_up(&workspace_root, &parsed)?;
    let server_url = parsed
        .server_url
        .clone()
        .unwrap_or_else(|| deployment.server_url.clone());

    let grpc_container = format!("bpmn-lite-grpc-target-{}", deployment.instance_name);
    remove_container_if_exists(&grpc_container)?;
    run_command(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg(&grpc_container)
            .arg("--network")
            .arg(&deployment.network_name)
            .arg("-p")
            .arg(format!(
                "{}:{}",
                DEFAULT_GRPC_TARGET_PORT, DEFAULT_GRPC_TARGET_PORT
            ))
            .arg(DEFAULT_GRPC_TARGET_IMAGE),
    )?;

    let grpc_target_url = format!("http://{}:{}", grpc_container, DEFAULT_GRPC_TARGET_PORT);
    wait_for_tcp_port(DEFAULT_GRPC_TARGET_PORT, Duration::from_secs(15))?;

    let result = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server-runner")
        .arg("--bin")
        .arg("grpc_proof")
        .arg("--")
        .arg("--server-url")
        .arg(&server_url)
        .arg("--grpc-target-url")
        .arg(&grpc_target_url)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run grpc_proof")
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                bail!("grpc_proof exited with {}", s)
            }
        });

    let _ = run_command(
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(&grpc_container),
    );
    let cleanup = docker_down_deployment(&deployment);
    result.and(cleanup)?;
    Ok(())
}

fn build_grpc_target_image(workspace_root: &Path) -> Result<()> {
    let dockerfile = r#"FROM rust:1.95-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends libprotobuf-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
RUN cargo build --release -p bpmn-lite-server-runner --bin grpc_test_target && strip target/release/grpc_test_target
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/grpc_test_target /usr/local/bin/
EXPOSE 50099
CMD ["grpc_test_target"]
"#;
    let path = workspace_root.join("Dockerfile.grpc-target");
    std::fs::write(&path, dockerfile).context("write Dockerfile.grpc-target")?;
    run_command(
        Command::new("docker")
            .arg("build")
            .arg("-t")
            .arg(DEFAULT_GRPC_TARGET_IMAGE)
            .arg("-f")
            .arg("Dockerfile.grpc-target")
            .arg(".")
            .current_dir(workspace_root),
    )
    .context("build grpc-target image")?;
    let _ = std::fs::remove_file(path);
    Ok(())
}

fn wait_for_tcp_port(port: u16, timeout: Duration) -> Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(&addr).is_ok() {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("port {} not ready after {}s", port, timeout.as_secs());
        }
        sleep(Duration::from_millis(200));
    }
}

fn docker_heterogeneous_smoke_command(extra_args: &[String]) -> Result<()> {
    let workspace_root = workspace_root()?;
    let parsed = parse_args(extra_args)?;

    if !parsed.skip_build {
        ensure_docker_image(&workspace_root, DEFAULT_DOCKER_IMAGE)?;
        build_http_target_image(&workspace_root)?;
        build_grpc_target_image(&workspace_root)?;
    }

    let deployment = docker_up(&workspace_root, &parsed)?;
    let server_url = parsed
        .server_url
        .clone()
        .unwrap_or_else(|| deployment.server_url.clone());

    // HTTP target
    let http_container = format!("bpmn-lite-http-target-{}", deployment.instance_name);
    remove_container_if_exists(&http_container)?;
    run_command(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg(&http_container)
            .arg("--network")
            .arg(&deployment.network_name)
            .arg("-p")
            .arg(format!(
                "{}:{}",
                DEFAULT_HTTP_TARGET_PORT, DEFAULT_HTTP_TARGET_PORT
            ))
            .arg(DEFAULT_HTTP_TARGET_IMAGE),
    )?;
    let http_target_url = format!("http://{}:{}", http_container, DEFAULT_HTTP_TARGET_PORT);
    wait_for_http_target(DEFAULT_HTTP_TARGET_PORT, Duration::from_secs(15))?;

    // gRPC target (B12: third vocabulary in the heterogeneous proof)
    let grpc_container = format!("bpmn-lite-grpc-target-{}", deployment.instance_name);
    remove_container_if_exists(&grpc_container)?;
    run_command(
        Command::new("docker")
            .arg("run")
            .arg("-d")
            .arg("--name")
            .arg(&grpc_container)
            .arg("--network")
            .arg(&deployment.network_name)
            .arg("-p")
            .arg(format!(
                "{}:{}",
                DEFAULT_GRPC_TARGET_PORT, DEFAULT_GRPC_TARGET_PORT
            ))
            .arg(DEFAULT_GRPC_TARGET_IMAGE),
    )?;
    let grpc_target_url = format!("http://{}:{}", grpc_container, DEFAULT_GRPC_TARGET_PORT);
    wait_for_tcp_port(DEFAULT_GRPC_TARGET_PORT, Duration::from_secs(15))?;

    let result = Command::new("cargo")
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server-runner")
        .arg("--bin")
        .arg("heterogeneous_proof")
        .arg("--")
        .arg("--server-url")
        .arg(&server_url)
        .arg("--http-target-url")
        .arg(&http_target_url)
        .arg("--grpc-target-url")
        .arg(&grpc_target_url)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run heterogeneous_proof")
        .and_then(|s| {
            if s.success() {
                Ok(())
            } else {
                bail!("heterogeneous_proof exited with {}", s)
            }
        });

    let _ = run_command(
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(&http_container),
    );
    let _ = run_command(
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .arg(&grpc_container),
    );
    let cleanup = docker_down_deployment(&deployment);
    result.and(cleanup)?;
    Ok(())
}

fn build_http_target_image(workspace_root: &Path) -> Result<()> {
    // Write a minimal Dockerfile for the http_test_target binary.
    let dockerfile_content = r#"FROM rust:1.95-bookworm AS builder
RUN apt-get update && apt-get install -y --no-install-recommends libprotobuf-dev protobuf-compiler && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
RUN cargo build --release -p bpmn-lite-server-runner --bin http_test_target && strip target/release/http_test_target
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/http_test_target /usr/local/bin/
EXPOSE 8080
CMD ["http_test_target"]
"#;
    let dockerfile_path = workspace_root.join("Dockerfile.http-target");
    std::fs::write(&dockerfile_path, dockerfile_content)
        .context("failed to write Dockerfile.http-target")?;

    run_command(
        Command::new("docker")
            .arg("build")
            .arg("-t")
            .arg(DEFAULT_HTTP_TARGET_IMAGE)
            .arg("-f")
            .arg("Dockerfile.http-target")
            .arg(".")
            .current_dir(workspace_root),
    )
    .context("failed to build http-test-target image")?;

    let _ = std::fs::remove_file(dockerfile_path);
    Ok(())
}

fn wait_for_http_target(host_port: u16, timeout: Duration) -> Result<()> {
    let addr = format!("127.0.0.1:{}", host_port);
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(&addr).is_ok() {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("http-test-target not ready after {}s", timeout.as_secs());
        }
        sleep(Duration::from_millis(200));
    }
}

fn docker_ffi_smoke_command(extra_args: &[String]) -> Result<()> {
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
        .arg("bpmn-lite-server-runner")
        .arg("--bin")
        .arg("ffi_proof")
        .arg("--")
        .arg("--server-url")
        .arg(&server_url)
        .current_dir(&workspace_root)
        .status()
        .context("failed to run ffi_proof against docker deployment")
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                bail!("ffi_proof exited with {}", status)
            }
        });

    let cleanup = docker_down_deployment(&deployment);
    result.and(cleanup)?;
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
        .arg("bpmn-lite-server-runner")
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
        .arg("bpmn-lite-server-runner")
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
        .arg("bpmn-lite-server-runner")
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

        // A18 — Split admin / runtime connection URLs.
        //
        // DATABASE_ADMIN_URL: superuser connection used for migrations
        //   (includes migration 026 which creates bpmn_lite_app).
        // DATABASE_URL: runtime connection.
        //
        // TODO(A18-store-rls): Once the WorkflowStore trait threads tenant_id
        // through load_instance/load_fibers/save_fiber so set_tenant_context
        // is called before every tenant-scoped query, revert DATABASE_URL to
        //   bpmn_lite_app (non-superuser, RLS active).
        // Until then, connect as postgres (superuser, RLS bypassed) so docker
        // smoke/stress/poison/subscription tests can run. Tracked issue: the
        // claim and load paths bypass app.tenant_id and see 0 rows under RLS.
        let container_admin_url = format!(
            "postgresql://postgres@{}/{}",
            deployment.db_container_name, db_name
        );
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
                .arg(format!("DATABASE_ADMIN_URL={container_admin_url}"))
                .arg("-e")
                .arg(format!("DATABASE_URL={container_database_url}"))
                .arg("-e")
                .arg(format!(
                    "BPMN_LITE_SCHEDULER_OWNER={}",
                    deployment.server_container_name
                ))
                .arg("-e")
                // TODO(A18-store-rls): remove when DATABASE_URL reverts to bpmn_lite_app
                .arg("BPMN_LITE_ALLOW_SUPERUSER=1")
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
    run_command(
        Command::new("docker")
            .arg("build")
            .arg("-t")
            .arg(image)
            .arg(".")
            .current_dir(workspace_root),
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
  cargo run -p xtask -- pack-build <domain>
  cargo run -p xtask -- run-pack
  cargo run -p xtask -- fuzz list
  cargo run -p xtask -- fuzz run [--target NAME] [--time SECS]
  cargo run -p xtask -- fuzz smoke
  cargo run -p xtask -- fuzz regress
  cargo run -p xtask -- fuzz clean

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

fn pack_build_command(domain: &str) -> Result<()> {
    let workspace_root = workspace_root()?;
    let manifests_dir = workspace_root.join("manifests");
    let dag_path = manifests_dir.join(format!("{}.dag.yaml", domain));
    if !dag_path.exists() {
        bail!(
            "DAG file not found for domain '{}' at {}",
            domain,
            dag_path.display()
        );
    }

    println!(
        "Loading DAG for domain '{}' from {}...",
        domain,
        dag_path.display()
    );
    let dag_content = std::fs::read_to_string(&dag_path)
        .with_context(|| format!("read DAG at {}", dag_path.display()))?;

    let dag: bpmn_lite_compiler::dsl::WorkflowPackDAG =
        serde_yaml::from_str(&dag_content).context("parse DAG yaml")?;

    println!("Generating manifest for domain '{}'...", domain);
    let mut manifest = bpmn_lite_compiler::dsl::generate_manifest(&dag)
        .map_err(|e| anyhow!("generate_manifest error: {}", e))?;

    // Derive the version with G5 (lex, dag, sorted pins)
    let version = bpmn_lite_compiler::dsl::derive_version(&manifest, &dag);
    println!("Derived pack version: {}", version);

    // Update manifest's catalogue_version
    manifest.catalogue_version = version.clone();

    println!("Running G1-G6 validation gates...");
    bpmn_lite_compiler::dsl::validate_pack(&dag, &manifest)
        .map_err(|errs| anyhow!("Validation failed: {:?}", errs))?;

    // Write manifest to disk at <domain>-v<dag.version>.yaml
    let manifest_yaml = manifest
        .to_yaml()
        .map_err(|e| anyhow!("serialize manifest error: {}", e))?;
    let manifest_path = manifests_dir.join(format!("{}-{}.yaml", domain, dag.version));
    std::fs::write(&manifest_path, &manifest_yaml)
        .with_context(|| format!("write manifest to {}", manifest_path.display()))?;
    println!("Wrote manifest to {}", manifest_path.display());

    // Generate and write closure to <domain>-v<dag.version>.closure.yaml
    let closure = bpmn_lite_compiler::dsl::generate_closure(&manifest, &dag);
    let closure_yaml = serde_yaml::to_string(&closure).context("serialize closure manifest")?;
    let closure_path = manifests_dir.join(format!("{}-{}.closure.yaml", domain, dag.version));
    std::fs::write(&closure_path, &closure_yaml)
        .with_context(|| format!("write closure to {}", closure_path.display()))?;
    println!("Wrote closure manifest to {}", closure_path.display());

    Ok(())
}

/// Verify checked-in pack artifacts without modifying them.
///
/// This is deliberately separate from `pack-build`: a generation command can
/// make a stale checkout look green by overwriting the evidence under test.
fn pack_check_command(domain: &str) -> Result<()> {
    let workspace_root = workspace_root()?;
    let manifests_dir = workspace_root.join("manifests");
    let dag_path = manifests_dir.join(format!("{domain}.dag.yaml"));
    let dag_content = std::fs::read_to_string(&dag_path)
        .with_context(|| format!("read DAG at {}", dag_path.display()))?;
    let dag: bpmn_lite_compiler::dsl::WorkflowPackDAG =
        serde_yaml::from_str(&dag_content).context("parse DAG yaml")?;

    let manifest_path = manifests_dir.join(format!("{}-{}.yaml", domain, dag.version));
    let checked_manifest_content = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read checked manifest at {}", manifest_path.display()))?;
    let checked_manifest = dsl_manifest::Manifest::load_from_yaml(&checked_manifest_content)
        .context("parse checked manifest")?;

    bpmn_lite_compiler::dsl::validate_pack(&dag, &checked_manifest)
        .map_err(|errors| anyhow!("checked pack validation failed: {errors:?}"))?;

    let mut generated = bpmn_lite_compiler::dsl::generate_manifest(&dag)
        .map_err(|error| anyhow!("generate_manifest error: {error}"))?;
    generated.catalogue_version = bpmn_lite_compiler::dsl::derive_version(&generated, &dag);
    let generated_yaml = generated
        .to_yaml()
        .map_err(|error| anyhow!("serialize generated manifest: {error}"))?;
    let checked_yaml = checked_manifest
        .to_yaml()
        .map_err(|error| anyhow!("serialize checked manifest: {error}"))?;
    if generated_yaml != checked_yaml {
        bail!(
            "checked manifest {} does not match the DAG-derived artifact; run `cargo xtask pack-build {domain}` and review the diff",
            manifest_path.display()
        );
    }

    let closure_path = manifests_dir.join(format!("{}-{}.closure.yaml", domain, dag.version));
    let checked_closure_content = std::fs::read_to_string(&closure_path)
        .with_context(|| format!("read checked closure at {}", closure_path.display()))?;
    let checked_closure: bpmn_lite_compiler::dsl::PackClosureManifest =
        serde_yaml::from_str(&checked_closure_content).context("parse checked closure")?;
    let generated_closure = bpmn_lite_compiler::dsl::generate_closure(&generated, &dag);
    if generated_closure != checked_closure {
        bail!(
            "checked closure {} does not match regeneration",
            closure_path.display()
        );
    }

    if domain == "bpmn" {
        let declared = checked_manifest
            .verb_ids()
            .collect::<std::collections::BTreeSet<_>>();
        let handled: std::collections::BTreeSet<&str> = bpmn_lite_bus_handler::HANDLED_VERBS
            .iter()
            .copied()
            .collect();
        check_bpmn_invocation_surface(&declared, &handled)?;
    }

    println!(
        "pack-check {domain}: checked DAG, manifest, closure, handler surface, and structural-node separation"
    );
    Ok(())
}

/// The checked BPMN invocation manifest declares exactly this
/// business-scoped subset of `BpmnLiteBusHandler`'s routed verbs
/// (excluding its two read-only catalogue verbs, `list-templates` and
/// `get-template-version`, which are not part of the per-instance
/// invocation surface this manifest governs).
const BPMN_INVOCATION_VERBS: [&str; 6] = [
    "define-template",
    "spawn-instance",
    "deliver-message",
    "correlate-message",
    "cancel-instance",
    "inspect-instance",
];

/// Structural BPMN elements (waits, boundary events) that must never appear
/// as a service-invocation verb — they compile into typed graph/IR
/// operations, not bus calls. Checked before the verb-subset-equality rule
/// below so the diagnostic names the actual defect (a structural id leaked
/// into the invocation surface) rather than a generic count/set mismatch;
/// otherwise this arm would never be reached, since any declared set
/// containing one of these ids already differs from the expected real-verb
/// set and would be caught there first with a less specific message.
const BPMN_STRUCTURAL_NODES: [&str; 4] = [
    "message-wait",
    "timer-wait",
    "boundary-timer",
    "boundary-error",
];

/// Pure, file-free checks over the BPMN invocation surface, factored out of
/// `pack_check_command` so each rule has its own red-fixture test: a
/// structural node, an unresolved verb, or a mismatched declared set must
/// each be independently detectable from a doctored input, not only
/// provable by reading real manifest/DAG files end to end.
fn check_bpmn_invocation_surface(
    declared: &std::collections::BTreeSet<&str>,
    handled: &std::collections::BTreeSet<&str>,
) -> Result<()> {
    check_no_structural_verbs(declared)?;
    check_invocation_verb_subset(declared)?;
    check_verbs_resolve_to_handlers(handled)?;
    Ok(())
}

fn check_no_structural_verbs(declared: &std::collections::BTreeSet<&str>) -> Result<()> {
    for forbidden in BPMN_STRUCTURAL_NODES {
        if declared.contains(forbidden) {
            bail!("structural BPMN node '{forbidden}' appears as an invocation verb");
        }
    }
    Ok(())
}

fn check_invocation_verb_subset(declared: &std::collections::BTreeSet<&str>) -> Result<()> {
    let expected = BPMN_INVOCATION_VERBS
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    if declared != &expected {
        bail!(
            "BPMN invocation manifest does not declare exactly the expected invocation verb subset: declared={declared:?}, expected={expected:?}"
        );
    }
    Ok(())
}

/// Resolve the expected invocation verbs against the handler crate's own
/// exported verb list (`bpmn_lite_bus_handler::HANDLED_VERBS`, declared next
/// to its real dispatch match and proven not to drift from it by
/// `handled_verbs_matches_every_dispatch_arm` in that crate) rather than a
/// copy of the verb strings kept here. A hand-maintained copy would stay
/// green even after a match arm was renamed or deleted upstream — this
/// resolves against the live source.
fn check_verbs_resolve_to_handlers(handled: &std::collections::BTreeSet<&str>) -> Result<()> {
    let expected = BPMN_INVOCATION_VERBS
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    let unresolved: Vec<&str> = expected
        .iter()
        .copied()
        .filter(|verb| !handled.contains(verb))
        .collect();
    if !unresolved.is_empty() {
        bail!(
            "BPMN invocation manifest declares verbs with no real BpmnLiteBusHandler route (checked against bpmn_lite_bus_handler::HANDLED_VERBS, not a hand-copied list): {unresolved:?}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod pack_check_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn expected_set() -> BTreeSet<&'static str> {
        BPMN_INVOCATION_VERBS.into_iter().collect()
    }

    #[test]
    fn accepts_the_real_verb_subset_fully_resolved() {
        let declared = expected_set();
        let handled = expected_set();
        assert!(check_bpmn_invocation_surface(&declared, &handled).is_ok());
    }

    #[test]
    fn red_dag_verb_without_handler_is_refused() {
        // The manifest declares exactly the expected verb subset, but the
        // handler crate no longer routes one of them (e.g. its match arm
        // was renamed or deleted) -- this is the drift a hand-copied
        // handler list could not catch; it must fail here.
        let handled = {
            let mut h = expected_set();
            h.remove("cancel-instance");
            h
        };

        let error = check_verbs_resolve_to_handlers(&handled)
            .expect_err("a manifest verb with no real handler route must be refused");
        let message = error.to_string();
        assert!(
            message.contains("no real BpmnLiteBusHandler route"),
            "unexpected error message: {message}"
        );
        assert!(
            message.contains("cancel-instance"),
            "diagnostic must name the unresolved verb: {message}"
        );

        // And the same fixture through the full combined check, so the
        // ordering (structural, then subset, then handler resolution)
        // doesn't accidentally hide this failure behind an earlier one.
        let declared = expected_set();
        let error = check_bpmn_invocation_surface(&declared, &handled)
            .expect_err("the combined check must also refuse this fixture");
        assert!(error.to_string().contains("cancel-instance"));
    }

    #[test]
    fn red_structural_node_registered_as_invocation_verb_is_refused() {
        // Exercise the dedicated structural-node check directly and in
        // true isolation: a same-SIZE declared set that substitutes one
        // real verb for a structural one must be refused specifically as
        // a structural-node violation, not merely as "not the expected 6".
        let declared = {
            let mut d = expected_set();
            d.remove("cancel-instance");
            d.insert("boundary-timer");
            d
        };
        assert_eq!(declared.len(), BPMN_INVOCATION_VERBS.len());

        let error = check_no_structural_verbs(&declared)
            .expect_err("a structural node in the declared surface must be refused");
        assert!(
            error
                .to_string()
                .contains("structural BPMN node 'boundary-timer'"),
            "unexpected error message: {error}"
        );

        // And through the combined check: the structural-node check runs
        // FIRST, so the diagnostic names the real defect (a structural id)
        // rather than the secondary "wrong 6 verbs" symptom.
        let handled = expected_set();
        let error = check_bpmn_invocation_surface(&declared, &handled)
            .expect_err("the combined check must refuse this fixture");
        assert!(
            error.to_string().contains("structural BPMN node"),
            "combined check must surface the structural-node diagnostic first, got: {error}"
        );
    }

    #[test]
    fn each_forbidden_structural_node_is_individually_detected() {
        for forbidden in BPMN_STRUCTURAL_NODES {
            let mut declared = expected_set();
            declared.insert(forbidden);
            let error = check_no_structural_verbs(&declared).unwrap_err();
            assert!(
                error.to_string().contains(forbidden),
                "'{forbidden}' must be named in the diagnostic, got: {error}"
            );
        }
    }
}

fn run_pack_command() -> Result<()> {
    let workspace_root = workspace_root()?;
    let server_url = "http://127.0.0.1:8080";

    // Spawn server
    println!("Starting bpmn-lite-server...");
    let mut server_child = ChildGuard::new(spawn_server_for_run_pack(&workspace_root, server_url)?);

    wait_for_server(server_url, Duration::from_secs(20))?;
    println!("Server is up on {}", server_url);

    // Read the comprehensive test workflow
    let dsl_path = workspace_root.join("manifests/comprehensive_test.dsl.lisp");
    let dsl_content = std::fs::read_to_string(&dsl_path)
        .with_context(|| format!("Failed to read comprehensive workflow at {:?}", dsl_path))?;

    // Create client
    let client = reqwest::blocking::Client::new();

    // ── Scenario 1: Fund (triggers Loop branch) ──
    println!("\n=== Executing Scenario 1: Fund (triggers Loop branch) ===");
    run_scenario(
        &client,
        server_url,
        &dsl_content,
        "fund",
        vec![
            // create-cbu -> advances to type-decision
            serde_json::json!({}),
            // type-decision -> DMN output is "fund" -> advances to type-gateway -> drive_forward walks past gateway to loop run-loop -> loop-step
            serde_json::json!({ "@cbu-type": "fund" }),
            // loop-step (Iteration 1) -> loop-step
            serde_json::json!({}),
            // loop-step (Iteration 2) -> ends
            serde_json::json!({}),
        ],
    )?;

    // ── Scenario 2: Corporate (triggers Parallel branch) ──
    println!("\n=== Executing Scenario 2: Corporate (triggers Parallel branch) ===");
    run_scenario(
        &client,
        server_url,
        &dsl_content,
        "corporate",
        vec![
            // create-cbu -> type-decision
            serde_json::json!({}),
            // type-decision -> DMN output is "corporate" -> advances to parallel branch split -> task-p1 and task-p2
            serde_json::json!({ "@cbu-type": "corporate" }),
            // task-p1 -> parallel-join
            serde_json::json!({}),
            // task-p2 -> parallel-join -> join completes -> ends
            serde_json::json!({}),
        ],
    )?;

    // ── Scenario 3: Trust (triggers Single branch) ──
    println!("\n=== Executing Scenario 3: Trust (triggers Single branch) ===");
    run_scenario(
        &client,
        server_url,
        &dsl_content,
        "trust",
        vec![
            // create-cbu -> type-decision
            serde_json::json!({}),
            // type-decision -> DMN output is "trust" -> advances to run-single
            serde_json::json!({ "@cbu-type": "trust" }),
            // run-single -> ends
            serde_json::json!({}),
        ],
    )?;

    println!("\nAll comprehensive DSL scenarios successfully executed!");
    server_child.stop();
    Ok(())
}

fn spawn_server_for_run_pack(workspace_root: &Path, _server_url: &str) -> Result<Child> {
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("-p")
        .arg("bpmn-lite-server-runner")
        .current_dir(workspace_root)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env("RUST_LOG", "info")
        .env("BPMN_LITE_REST_BIND", "127.0.0.1:8080")
        .env("BPMN_LITE_BIND", "127.0.0.1:50061") // run gRPC on a different port to avoid conflict
        .env("BPMN_LITE_STORE", "memory")
        .arg("--bin")
        .arg("bpmn-lite-server");

    let child = command
        .spawn()
        .context("Failed to spawn bpmn-lite-server")?;
    Ok(child)
}

fn run_scenario(
    client: &reqwest::blocking::Client,
    server_url: &str,
    dsl_content: &str,
    cbu_type: &str,
    step_outputs: Vec<serde_json::Value>,
) -> Result<()> {
    // Start instance
    let start_url = format!("{}/bpmn/instances/start", server_url);
    let start_body = serde_json::json!({
        "cbu_type": cbu_type,
        "bpmn_dsl": dsl_content
    });

    let res = client
        .post(&start_url)
        .json(&start_body)
        .send()
        .context("HTTP request failed starting instance")?;

    if !res.status().is_success() {
        let status = res.status();
        let err_text = res.text().unwrap_or_default();
        bail!(
            "Failed to start instance: HTTP {}. Body: {}",
            status,
            err_text
        );
    }

    let res_json: serde_json::Value = res.json().context("Failed to parse start response JSON")?;
    let instance_id = res_json["instance_id"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing instance_id"))?;
    println!(
        "Started instance {} for cbu_type '{}'",
        instance_id, cbu_type
    );

    // Get detail
    let detail_url = format!("{}/bpmn/instances/{}", server_url, instance_id);
    let res = client.get(&detail_url).send()?;
    let detail: serde_json::Value = res.json()?;
    println!(
        "Initial state: node = {}, status = {}",
        detail["current_node"], detail["status"]
    );

    // Step through the process
    for (i, outputs) in step_outputs.into_iter().enumerate() {
        let next_url = format!("{}/bpmn/instances/{}/next-step", server_url, instance_id);
        let next_body = serde_json::json!({
            "outputs": outputs
        });

        let res = client.post(&next_url).json(&next_body).send()?;
        if !res.status().is_success() {
            let status = res.status();
            let err_text = res.text().unwrap_or_default();
            bail!("Step {} failed: HTTP {}. Body: {}", i + 1, status, err_text);
        }

        let step_res: serde_json::Value = res.json()?;
        println!(
            "Step {} response: node = {}, status = {}, message = {}",
            i + 1,
            step_res["node"],
            step_res["status"],
            step_res["message"]
        );
    }

    // Load detail and check final status
    let res = client.get(&detail_url).send()?;
    let final_detail: serde_json::Value = res.json()?;
    println!(
        "Final state: node = {}, status = {}",
        final_detail["current_node"], final_detail["status"]
    );

    let status_str = final_detail["status"].as_str().unwrap_or_default();
    if !status_str.contains("Completed") {
        bail!(
            "Workflow did not reach Completed state! Current state: {}",
            status_str
        );
    }

    println!("Scenario for '{}' completed successfully!", cbu_type);
    Ok(())
}
