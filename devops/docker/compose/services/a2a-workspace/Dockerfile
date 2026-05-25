# Shared Rust builder for A2A smoke stack binaries (cargo-chef dependency cache).
FROM rust:1.93-slim AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json \
    -p a2a-auth-callout \
    -p a2a-gateway \
    -p a2a-bridge \
    -p a2a-nats-server \
    -p a2a-nats-agent \
    -p a2a-smoke-test

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release \
    -p a2a-auth-callout \
    -p a2a-gateway \
    -p a2a-bridge \
    -p a2a-nats-server \
    -p a2a-nats-agent \
    -p a2a-smoke-test \
    && strip target/release/a2a-auth-callout \
    && strip target/release/a2a-gateway \
    && strip target/release/a2a-bridge \
    && strip target/release/a2a-nats-server \
    && strip target/release/a2a-nats-agent \
    && strip target/release/a2a-nats-agent-echo \
    && strip target/release/a2a-smoke-test

FROM debian:bookworm-20250317-slim AS runtime-base

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

FROM runtime-base AS a2a-auth-callout
COPY --from=builder /build/target/release/a2a-auth-callout /usr/local/bin/a2a-auth-callout
ENTRYPOINT ["/usr/local/bin/a2a-auth-callout"]

FROM runtime-base AS a2a-gateway
COPY --from=builder /build/target/release/a2a-gateway /usr/local/bin/a2a-gateway
ENTRYPOINT ["/usr/local/bin/a2a-gateway"]

FROM runtime-base AS a2a-bridge
COPY --from=builder /build/target/release/a2a-bridge /usr/local/bin/a2a-bridge
ENTRYPOINT ["/usr/local/bin/a2a-bridge"]

FROM runtime-base AS a2a-nats-server
COPY --from=builder /build/target/release/a2a-nats-server /usr/local/bin/a2a-nats-server
ENTRYPOINT ["/usr/local/bin/a2a-nats-server"]

FROM runtime-base AS a2a-nats-agent
COPY --from=builder /build/target/release/a2a-nats-agent-echo /usr/local/bin/a2a-nats-agent
ENTRYPOINT ["/usr/local/bin/a2a-nats-agent"]

FROM runtime-base AS a2a-smoke-test
COPY --from=builder /build/target/release/a2a-smoke-test /usr/local/bin/a2a-smoke-test
ENTRYPOINT ["/usr/local/bin/a2a-smoke-test"]
