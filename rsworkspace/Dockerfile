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
#   acp-nats-ws        — ACP WebSocket bridge (port 8085)
#   acp-nats-server    — ACP HTTP transport bridge (port 8086)
#   trogon-acp-runner  — ACP agent runner / Claude (NATS only)
#   trogon-compactor   — Context compaction service (NATS only)
#   trogon-xai-runner  — ACP agent runner / xAI Grok (NATS only)
#   trogon-codex-runner — ACP agent runner / Codex (NATS only)
#   trogon-wasm-runtime — WASM tool execution runtime (NATS only)
#   trogon-console     — Console management API (port 8091)
#   trogon-router      — Event router / LLM dispatch (NATS only)
#   trogon-pr-actor    — PR automation actor (NATS only)
#
# Build examples:
#   docker build --target agent  -t trogon-agent  .
#   docker build --target proxy  -t trogon-proxy  .

# ── Stage 1: cargo-chef installer ────────────────────────────────────────────
FROM rust:1.94-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /workspace

# ── Stage 2: compute dependency recipe ───────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: build dependencies (cached unless Cargo.lock changes) ────────────
FROM chef AS builder
# ca-certificates needed for reqwest rustls-tls to verify upstream TLS certs
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
    -p trogon-incidentio \
    -p acp-nats-ws \
    -p acp-nats-server \
    -p trogon-acp-runner \
    -p trogon-compactor \
    -p trogon-xai-runner \
    -p trogon-codex-runner \
    -p trogon-wasm-runtime \
    -p trogon-console \
    -p trogon-router \
    -p trogon-pr-actor

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

# ── Stage 11: acp-nats-ws runtime ────────────────────────────────────────────
FROM debian:bookworm-slim AS acp-nats-ws
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/acp-nats-ws /usr/local/bin/acp-nats-ws
EXPOSE 8085
ENTRYPOINT ["acp-nats-ws"]

# ── Stage 12: acp-nats-server runtime ────────────────────────────────────────
FROM debian:bookworm-slim AS acp-nats-server
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/acp-nats-server /usr/local/bin/acp-nats-server
EXPOSE 8086
ENTRYPOINT ["acp-nats-server"]

# ── Stage 13: trogon-acp-runner runtime ──────────────────────────────────────
FROM debian:bookworm-slim AS trogon-acp-runner
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-acp-runner /usr/local/bin/trogon-acp-runner
ENTRYPOINT ["trogon-acp-runner"]

# ── Stage 14: trogon-compactor runtime ───────────────────────────────────────
FROM debian:bookworm-slim AS trogon-compactor
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-compactor /usr/local/bin/trogon-compactor
ENTRYPOINT ["trogon-compactor"]

# ── Stage 15: trogon-xai-runner runtime ──────────────────────────────────────
FROM debian:bookworm-slim AS trogon-xai-runner
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-xai-runner /usr/local/bin/trogon-xai-runner
ENTRYPOINT ["trogon-xai-runner"]

# ── Stage 16: trogon-codex-runner runtime ────────────────────────────────────
FROM ubuntu:24.04 AS trogon-codex-runner
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-codex-runner /usr/local/bin/trogon-codex-runner
ENTRYPOINT ["trogon-codex-runner"]

# ── Stage 17: trogon-wasm-runtime runtime ────────────────────────────────────
FROM ubuntu:24.04 AS trogon-wasm-runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-wasm-runtime /usr/local/bin/trogon-wasm-runtime
ENTRYPOINT ["trogon-wasm-runtime"]

# ── Stage 18: trogon-console runtime ─────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-console
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-console /usr/local/bin/trogon-console
EXPOSE 8091
ENTRYPOINT ["trogon-console"]

# ── Stage 19: trogon-router runtime ──────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-router
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-router /usr/local/bin/trogon-router
ENTRYPOINT ["trogon-router"]

# ── Stage 20: trogon-pr-actor runtime ────────────────────────────────────────
FROM debian:bookworm-slim AS trogon-pr-actor
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /workspace/target/release/trogon-pr-actor /usr/local/bin/trogon-pr-actor
ENTRYPOINT ["trogon-pr-actor"]
