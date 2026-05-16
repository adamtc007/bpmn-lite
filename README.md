# BPMN-lite

Standalone BPMN runtime and gRPC service.

## Runtime Contract

- [Runtime invariants](docs/runtime-invariants.md)
- [Executable BPMN subset](docs/bpmn-subset.md)

## Boundary

- BPMN-lite owns its own database schema and migrations.
- BPMN-lite does not read or write `"ob-poc".*` database tables.
- The shared `SessionStackState` type is imported from `ob-poc-types`, but it is
  copied by value at the integration boundary.
- BPMN-lite persists its own copy on `process_instances.session_stack` and
  `job_queue.session_stack`.
- There is no shared database record and no live aliasing back into ob-poc
  session persistence.

## Standalone Postgres Bootstrap

Create a dedicated database:

```bash
createdb bpmn_lite_test
```

Start the standalone server with Postgres enabled:

```bash
cd bpmn-lite
DATABASE_URL=postgresql:///bpmn_lite_test \
RUST_LOG=info \
cargo run -p bpmn-lite-server --features postgres --bin bpmn-lite-server
```

On startup the server will:

1. connect to `DATABASE_URL`
2. apply BPMN-lite migrations from `bpmn-lite-core/migrations`
3. serve gRPC on `0.0.0.0:50051` by default

Override the bind address if needed:

```bash
cd bpmn-lite
DATABASE_URL=postgresql:///bpmn_lite_test \
BPMN_LITE_BIND=127.0.0.1:50061 \
cargo run -p bpmn-lite-server --features postgres --bin bpmn-lite-server
```

## Standalone Test Paths

Compile-only checks:

```bash
cd bpmn-lite
env RUSTC_WRAPPER= cargo check
env RUSTC_WRAPPER= cargo check -p bpmn-lite-server --features postgres
```

Schema independence guard:

```bash
cd bpmn-lite
env RUSTC_WRAPPER= cargo test test_master_schema_is_standalone_from_ob_poc_namespace -- --nocapture
```

Copy-by-value session stack tests without Postgres:

```bash
cd bpmn-lite
env RUSTC_WRAPPER= cargo test test_start_with_session_stack_copies_value -- --nocapture
env RUSTC_WRAPPER= cargo test test_instance_session_stack_is_not_aliased -- --nocapture
env RUSTC_WRAPPER= cargo test test_job_queue_session_stack_is_not_aliased -- --nocapture
```

Postgres-backed standalone tests:

```bash
cd bpmn-lite
BPMN_LITE_TEST_DATABASE_URL=postgresql:///bpmn_lite_test \
env RUSTC_WRAPPER= cargo test -p bpmn-lite-core --features postgres test_pg_instance_round_trip -- --ignored --nocapture

BPMN_LITE_TEST_DATABASE_URL=postgresql:///bpmn_lite_test \
env RUSTC_WRAPPER= cargo test -p bpmn-lite-core --features postgres test_pg_instance_session_stack_copy_round_trip -- --ignored --nocapture
```

These Postgres tests use only BPMN-lite tables and mocked `SessionStackState`
values. They do not require ob-poc migrations or ob-poc database tables.

## Load Harness

Run the existing smoke profile against a standalone server:

```bash
cd bpmn-lite
cargo run -p xtask -- smoke --spawn-server --database-url postgresql:///bpmn_lite_test
```

## Multi-Instance Docker Harness

`xtask` can now spin up isolated Docker deployments where each BPMN-lite server
has its own Postgres container, network, and volume.

Bring up one instance:

```bash
cd bpmn-lite
cargo run -p xtask -- docker-up \
  --instance-name alpha \
  --server-port 50071 \
  --db-port 5541
```

Bring up a second independent instance:

```bash
cd bpmn-lite
cargo run -p xtask -- docker-up \
  --instance-name beta \
  --server-port 50072 \
  --db-port 5542
```

Each instance gets:

- its own BPMN-lite server container
- its own Postgres container
- its own Docker network
- its own Docker volume

Run the harness against a specific isolated instance:

```bash
cd bpmn-lite
cargo run -p xtask -- docker-smoke \
  --instance-name alpha \
  --server-port 50071 \
  --db-port 5541
```

Keep the containers running after the harness:

```bash
cd bpmn-lite
cargo run -p xtask -- docker-stress \
  --instance-name beta \
  --server-port 50072 \
  --db-port 5542 \
  --keep-running \
  --instances 200
```

Tear down a specific instance cleanly:

```bash
cd bpmn-lite
cargo run -p xtask -- docker-down --instance-name alpha
```

These Docker flows still use the same copy-by-value `SessionStackState`
contract. The harness can mock session-stack values freely; BPMN-lite persists
its own copies inside the instance/job records and does not depend on ob-poc
database state.

## Release Checklist

- `env RUSTC_WRAPPER= cargo check`
- `env RUSTC_WRAPPER= cargo check -p xtask`
- `cargo run -p xtask -- docker-up --instance-name alpha --server-port 50071 --db-port 5541`
- `cargo run -p xtask -- docker-up --instance-name beta --server-port 50072 --db-port 5542`
- `cargo run -p xtask -- smoke --server-url http://127.0.0.1:50071`
- `cargo run -p xtask -- smoke --server-url http://127.0.0.1:50072`
- Verify `process_instances` row counts independently in each paired Postgres instance.
- `cargo run -p xtask -- docker-down --instance-name alpha`
- `cargo run -p xtask -- docker-down --instance-name beta`
- Confirm `docker ps -a` shows no leftover BPMN-lite test containers.

Expected result:

- each BPMN-lite deployment migrates and serves against its own database
- the smoke harness passes independently against multiple live instances
- `SessionStackState` is copied by value into BPMN-owned persistence and is not shared by DB reference
