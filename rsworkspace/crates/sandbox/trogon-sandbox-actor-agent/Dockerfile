# syntax=docker/dockerfile:1
#
# Build from the repository root:
#   docker build -f rsworkspace/crates/sandbox/trogon-sandbox-actor-agent/Dockerfile .
#
# The runtime runs the tenant's own image with its entrypoint overridden to the
# agent binary, so the agent has to be baked into whatever image a lease names.
# `useradd`/`getent` need a non-distroless base, and the agent must run as root
# for user provisioning to work at all.

FROM rust:1-slim-bookworm AS chef
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates pkg-config libssl-dev protobuf-compiler libprotobuf-dev \
 && rm -rf /var/lib/apt/lists/* \
 && cargo install cargo-chef --locked
WORKDIR /build/rsworkspace

FROM chef AS planner
COPY rsworkspace/ /build/rsworkspace/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /build/rsworkspace/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p trogon-sandbox-actor-agent

COPY rsworkspace/ /build/rsworkspace/
RUN cargo build --release -p trogon-sandbox-actor-agent --bin trogon-sandbox-actor-agent

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends bash passwd ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/rsworkspace/target/release/trogon-sandbox-actor-agent /usr/local/bin/trogon-sandbox-actor-agent

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-actor-agent"]
