# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.85-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace

# Copy workspace manifests first for layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates/llm-types/Cargo.toml   crates/llm-types/Cargo.toml
COPY crates/llm-gemini/Cargo.toml  crates/llm-gemini/Cargo.toml
COPY crates/trogon-nats/Cargo.toml crates/trogon-nats/Cargo.toml
COPY crates/trogon-std/Cargo.toml  crates/trogon-std/Cargo.toml

# Stub sources so cargo can resolve the dependency graph.
RUN mkdir -p \
    crates/llm-types/src \
    crates/llm-gemini/src \
    crates/trogon-nats/src \
    crates/trogon-std/src && \
    echo "fn main(){}" > crates/llm-gemini/src/main.rs && \
    touch crates/llm-types/src/lib.rs \
          crates/trogon-nats/src/lib.rs \
          crates/trogon-std/src/lib.rs

RUN cargo build --release -p llm-gemini 2>/dev/null || true

# Copy real sources and build.
COPY crates/ crates/
RUN cargo build --release -p llm-gemini

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 wget && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/llm-gemini /usr/local/bin/llm-gemini

ENV RUST_LOG=info

CMD ["llm-gemini"]
