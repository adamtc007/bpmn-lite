FROM rust:1.95-bookworm AS builder

RUN apt-get update && apt-get install -y protobuf-compiler && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY bpmn-lite ./bpmn-lite
COPY rust/crates/ob-poc-types ./rust/crates/ob-poc-types
WORKDIR /workspace/bpmn-lite
RUN cargo build --release -p bpmn-lite-server --features postgres

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/bpmn-lite/target/release/bpmn-lite-server /usr/local/bin/

EXPOSE 50051

CMD ["bpmn-lite-server"]
