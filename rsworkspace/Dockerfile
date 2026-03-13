# Multi-stage Dockerfile for the full trogon stack.
#
# Targets:
#   proxy              — HTTP detokenization proxy (port 8080)
#   worker             — JetStream pull consumer + vault admin
#   agent              — Agentic loop + automations + chat API (port 8090)
#   trogon-github      — GitHub webhook receiver (port 8081)
#   trogon-linear      — Linear webhook receiver (port 8082)
#   trogon-datadog     — Datadog webhook receiver (port 8083)
#   trogon-incidentio  — incident.io webhook receiver (port 8084)
#
# Build examples:
#   docker build --target agent  -t trogon-agent  .
#   docker build --target proxy  -t trogon-proxy  .

# ── Stage 1: cargo-chef installer ────────────────────────────────────────────
FROM rust:1.87-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /workspace

# ── Stage 2: compute dependency recipe ───────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: build dependencies (cached unless Cargo.lock changes) ────────────
FROM chef AS builder
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=planner /workspace/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release \
    -p trogon-secret-proxy \
    -p trogon-agent \
    -p trogon-github \
    -p trogon-linear \
    -p trogon-datadog \
    -p trogon-incidentio

# ── Stage 4: proxy runtime ───────────────────────────────────────────────────
FROM debian:bookworm-slim AS proxy
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/proxy /usr/local/bin/proxy
EXPOSE 8080
ENTRYPOINT ["proxy"]

# ── Stage 5: worker runtime ──────────────────────────────────────────────────
FROM debian:bookworm-slim AS worker
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/worker /usr/local/bin/worker
ENTRYPOINT ["worker"]

# ── Stage 6: agent runtime ───────────────────────────────────────────────────
FROM debian:bookworm-slim AS agent
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/agent /usr/local/bin/agent
EXPOSE 8090
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["sh", "-c", "curl -sf http://localhost:${AUTOMATIONS_API_PORT:-8090}/stats -H 'x-tenant-id: health' || exit 1"]
ENTRYPOINT ["agent"]

# ── Stage 7: trogon-github runtime ───────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-github
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-github /usr/local/bin/trogon-github
EXPOSE 8081
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["sh", "-c", "curl -sf http://localhost:${GITHUB_WEBHOOK_PORT:-8081}/health || exit 1"]
ENTRYPOINT ["trogon-github"]

# ── Stage 8: trogon-linear runtime ───────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-linear
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-linear /usr/local/bin/trogon-linear
EXPOSE 8082
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["sh", "-c", "curl -sf http://localhost:${LINEAR_WEBHOOK_PORT:-8082}/health || exit 1"]
ENTRYPOINT ["trogon-linear"]

# ── Stage 9: trogon-datadog runtime ──────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-datadog
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-datadog /usr/local/bin/trogon-datadog
EXPOSE 8083
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["sh", "-c", "curl -sf http://localhost:${DATADOG_WEBHOOK_PORT:-8083}/health || exit 1"]
ENTRYPOINT ["trogon-datadog"]

# ── Stage 10: trogon-incidentio runtime ──────────────────────────────────────
FROM debian:bookworm-slim AS trogon-incidentio
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-incidentio /usr/local/bin/trogon-incidentio
EXPOSE 8084
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["sh", "-c", "curl -sf http://localhost:${INCIDENTIO_WEBHOOK_PORT:-8084}/health || exit 1"]
ENTRYPOINT ["trogon-incidentio"]
