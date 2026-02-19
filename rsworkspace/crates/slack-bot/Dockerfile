# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

# Copy workspace manifests first for layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates/slack-types/Cargo.toml   crates/slack-types/Cargo.toml
COPY crates/slack-nats/Cargo.toml    crates/slack-nats/Cargo.toml
COPY crates/slack-bot/Cargo.toml     crates/slack-bot/Cargo.toml
COPY crates/trogon-nats/Cargo.toml   crates/trogon-nats/Cargo.toml
COPY crates/trogon-std/Cargo.toml    crates/trogon-std/Cargo.toml

# Stub sources so cargo can resolve the dependency graph.
RUN mkdir -p \
    crates/slack-types/src \
    crates/slack-nats/src \
    crates/slack-bot/src \
    crates/trogon-nats/src \
    crates/trogon-std/src && \
    echo "fn main(){}" > crates/slack-bot/src/main.rs && \
    touch crates/slack-types/src/lib.rs \
          crates/slack-nats/src/lib.rs \
          crates/trogon-nats/src/lib.rs \
          crates/trogon-std/src/lib.rs

RUN cargo build --release -p slack-bot 2>/dev/null || true

# Copy real sources and build.
COPY crates/ crates/
RUN cargo build --release -p slack-bot

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/slack-bot /usr/local/bin/slack-bot

ENV RUST_LOG=info

CMD ["slack-bot"]
