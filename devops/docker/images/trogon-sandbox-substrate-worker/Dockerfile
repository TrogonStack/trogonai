# syntax=docker/dockerfile:1
#
# Agent Substrate worker: fulfills sandbox leases as Substrate actors.
# devops/kubernetes/sandbox/substrate-worker/validation has an example
# deployment. The build context is the repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-substrate-worker/Dockerfile -t trogon-sandbox-substrate-worker:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-substrate-worker --bin trogon-sandbox-substrate-worker \
    && install -D /cargo-target/release/trogon-sandbox-substrate-worker /out/trogon-sandbox-substrate-worker

# Trixie's glibc is newer than the one on the CI runners that compile the
# prebuilt binaries, so those binaries load here.
FROM debian:trixie-20261104-slim AS base

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 trogon \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin trogon

USER 65532:65532

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-substrate-worker"]

# CI compiles each architecture natively and passes the binaries in as the
# `binaries` build context.
FROM base AS prebuilt

ARG TARGETARCH
COPY --from=binaries --chmod=0755 ${TARGETARCH}/trogon-sandbox-substrate-worker /usr/local/bin/trogon-sandbox-substrate-worker

FROM base AS runtime

COPY --from=builder /out/trogon-sandbox-substrate-worker /usr/local/bin/trogon-sandbox-substrate-worker
