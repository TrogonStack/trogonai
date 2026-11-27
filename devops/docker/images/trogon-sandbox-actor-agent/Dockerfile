# syntax=docker/dockerfile:1
#
# Actor agent: the in-actor HTTP agent that carries exec, file, terminal and
# identity traffic for sandbox leases. The substrate worker runs actors from
# this image as `trogon-sandbox-actor-agent`; see
# devops/kubernetes/sandbox/substrate-worker/validation.
#
# The runtime runs the tenant's own image with its entrypoint overridden to
# the agent binary, so the agent has to be baked into whatever image a lease
# names. `useradd`/`getent` need a non-distroless base, and the agent must
# run as root for user provisioning to work at all. The build context is the
# repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-actor-agent/Dockerfile -t trogon-sandbox-actor-agent:dev .
#
# Pass `--build-arg CARGO_BUILD_JOBS=<n>` to cap the compile jobs.
FROM rust:1.96.0-slim-trixie AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

ARG TARGETARCH
# Caps parallel compile jobs on a shared host. Left unset, cargo uses every CPU.
ARG CARGO_BUILD_JOBS
ENV CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=/cargo-target
WORKDIR /src

RUN --mount=type=bind,source=rsworkspace,target=/src,rw \
    --mount=type=cache,id=trogonai-rust-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=trogonai-rust-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=trogonai-rust-1.96.0-${TARGETARCH},target=/cargo-target,sharing=locked \
    find . -type f -exec touch {} + \
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-actor-agent --bin trogon-sandbox-actor-agent \
    && install -D /cargo-target/release/trogon-sandbox-actor-agent /out/trogon-sandbox-actor-agent

# Trixie's glibc is newer than the one on the CI runners that compile the
# prebuilt binaries, so those binaries load here.
FROM debian:trixie-20261104-slim AS base

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash passwd ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-actor-agent"]

# CI compiles each architecture natively and passes the binaries in as the
# `binaries` build context.
FROM base AS prebuilt

ARG TARGETARCH
COPY --from=binaries --chmod=0755 ${TARGETARCH}/trogon-sandbox-actor-agent /usr/local/bin/trogon-sandbox-actor-agent

FROM base AS runtime

COPY --from=builder /out/trogon-sandbox-actor-agent /usr/local/bin/trogon-sandbox-actor-agent
