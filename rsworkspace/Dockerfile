# Multi-stage Dockerfile for trogon-secret-proxy binaries.
#
# Targets:
#   proxy   — HTTP proxy (axum, port 8080)
#   worker  — JetStream pull consumer + detokenization worker
#
# Build examples:
#   docker build --target proxy  -t trogon-proxy  .
#   docker build --target worker -t trogon-worker .

# ── Stage 1: dependency cache ─────────────────────────────────────────────────
FROM rust:1.82-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /workspace

# ── Stage 2: compute recipe (dependency plan) ─────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: build dependencies (cached unless Cargo.lock changes) ────────────
FROM chef AS builder
# ca-certificates needed for reqwest rustls-tls to verify upstream TLS certs
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
# Build the actual binaries
COPY . .
RUN cargo build --release -p trogon-secret-proxy

# ── Stage 4: proxy runtime image ──────────────────────────────────────────────
FROM debian:bookworm-slim AS proxy
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/proxy /usr/local/bin/proxy
EXPOSE 8080
ENTRYPOINT ["proxy"]

# ── Stage 5: worker runtime image ─────────────────────────────────────────────
FROM debian:bookworm-slim AS worker
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/worker /usr/local/bin/worker
ENTRYPOINT ["worker"]
