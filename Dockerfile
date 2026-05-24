# syntax=docker/dockerfile:1
#
# Build context: bpmn-lite/ workspace root (run: docker build -t bpmn-lite-server:local .)
# xtask uses this via: cargo run -p xtask -- docker-up
#
# ob-poc-types is a rev-pinned git dep — cargo fetches it from GitHub during build;
# no COPY from the parent ob-poc directory is needed (B0 consolidation, 2026-05-16).

# ── Stage 1: chef ────────────────────────────────────────────────────────────
# Install cargo-chef once; this layer is shared by planner and builder stages.
FROM rust:1.95-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /build

# ── Stage 2: planner ─────────────────────────────────────────────────────────
# Analyse the full workspace dependency tree and produce recipe.json.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: builder ─────────────────────────────────────────────────────────
# Cook deps (cached; only invalidated when Cargo.toml/Cargo.lock change),
# then compile the real binary.
FROM chef AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends libprotobuf-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY --from=planner /build/recipe.json recipe.json
# bpmn-lite-server/build.rs runs tonic_build to compile proto files.
# Copy the proto directory before `cook` so the build script can find them.
COPY bpmn-lite-server/proto bpmn-lite-server/proto
RUN cargo chef cook --release --features postgres -p bpmn-lite-server --recipe-path recipe.json

COPY . .
RUN cargo build --release --features postgres -p bpmn-lite-server \
    && mkdir -p /build/out \
    && cp target/release/bpmn-lite-server /build/out/bpmn-lite-server \
    && strip /build/out/bpmn-lite-server

# ── Stage 4: runtime ─────────────────────────────────────────────────────────
# Distroless cc keeps glibc/runtime libs without a shell or package manager.
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=builder /build/out/bpmn-lite-server /usr/local/bin/

EXPOSE 50051

# Required env: DATABASE_URL (unless BPMN_LITE_STORE=memory)
# Optional env: BPMN_LITE_BIND (default 0.0.0.0:50051), RUST_LOG (default info)
CMD ["bpmn-lite-server"]
