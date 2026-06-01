# syntax=docker/dockerfile:1.7
# Multi-stage build for any shipping binary in `rsworkspace/`.
#
# Usage:
#   docker build -f devops/docker/Dockerfile \
#     --build-arg BIN=trogon-mcp-gateway \
#     -t agentgateway/trogon-mcp-gateway:dev .
#
# Supported BIN values match crate names under `rsworkspace/crates/` that have
# a `src/main.rs`:
#   a2a-bridge, a2a-gateway, a2a-auth-callout, mcp-nats-server, mcp-pack,
#   trogon-agent-registry, trogon-agent-registry-controller, trogon-gateway-ctl,
#   trogon-gateway-k8s, trogon-jwks-publisher, trogon-mcp-gateway, trogon-sts,
#   agctl.

ARG RUST_VERSION=1.93.0

FROM rust:${RUST_VERSION}-bookworm AS builder
ARG BIN

WORKDIR /workspace
COPY rsworkspace ./rsworkspace

WORKDIR /workspace/rsworkspace
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/workspace/rsworkspace/target \
    cargo build --release --bin "${BIN}" \
    && mkdir -p /out \
    && cp "target/release/${BIN}" "/out/${BIN}"

FROM gcr.io/distroless/cc-debian12:nonroot
ARG BIN
ENV BIN_NAME=${BIN}

COPY --from=builder /out/${BIN} /usr/local/bin/agentgateway-bin
USER nonroot:nonroot

ENTRYPOINT ["/usr/local/bin/agentgateway-bin"]
