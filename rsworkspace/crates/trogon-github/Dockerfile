# ── Stage 1: build ────────────────────────────────────────────────────────────
FROM rust:1.87-slim AS builder

WORKDIR /build

# Pre-fetch dependencies before copying source (better layer caching).
COPY Cargo.toml Cargo.lock ./
COPY crates/trogon-std         crates/trogon-std
COPY crates/trogon-nats        crates/trogon-nats
COPY crates/trogon-github      crates/trogon-github

RUN cargo build --release -p trogon-github

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/trogon-github /usr/local/bin/trogon-github

# Webhook port
EXPOSE 8080

# Liveness probe default
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/bin/sh", "-c", "curl -sf http://localhost:${GITHUB_WEBHOOK_PORT:-8080}/health || exit 1"]

ENTRYPOINT ["/usr/local/bin/trogon-github"]
