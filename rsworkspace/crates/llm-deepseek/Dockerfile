# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

# Copy workspace manifests first for layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates/llm-types/Cargo.toml     crates/llm-types/Cargo.toml
COPY crates/llm-deepseek/Cargo.toml  crates/llm-deepseek/Cargo.toml
COPY crates/trogon-nats/Cargo.toml   crates/trogon-nats/Cargo.toml
COPY crates/trogon-std/Cargo.toml    crates/trogon-std/Cargo.toml

# Stub sources so cargo can resolve the dependency graph.
RUN mkdir -p \
    crates/llm-types/src \
    crates/llm-deepseek/src \
    crates/trogon-nats/src \
    crates/trogon-std/src && \
    echo "fn main(){}" > crates/llm-deepseek/src/main.rs && \
    touch crates/llm-types/src/lib.rs \
          crates/trogon-nats/src/lib.rs \
          crates/trogon-std/src/lib.rs

RUN cargo build --release -p llm-deepseek 2>/dev/null || true

# Copy real sources and build.
COPY crates/ crates/
RUN cargo build --release -p llm-deepseek

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 wget && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/llm-deepseek /usr/local/bin/llm-deepseek

ENV RUST_LOG=info

CMD ["llm-deepseek"]
