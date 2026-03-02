# ── Stage 1: build ────────────────────────────────────────────────────────────
FROM rust:1.87-slim AS builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p trogon-linear

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/trogon-linear /usr/local/bin/trogon-linear

# Webhook port
EXPOSE 8080

# Liveness probe default
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/bin/sh", "-c", "curl -sf http://localhost:${LINEAR_WEBHOOK_PORT:-8080}/health || exit 1"]

ENTRYPOINT ["/usr/local/bin/trogon-linear"]
