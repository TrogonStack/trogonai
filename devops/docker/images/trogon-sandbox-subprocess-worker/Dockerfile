# syntax=docker/dockerfile:1
#
# Subprocess worker: fulfills sandbox leases as isolated local processes.
# On Linux it isolates them with bubblewrap, so the host must allow
# unprivileged user namespaces for the container. Each lease also gets the
# egress proxy, which ships in this image as `trogon-sandbox-egress-proxyd`.
# It runs `git` for lease checkouts. The `sandbox-subprocess-worker` chart
# runs this image as `trogon-sandbox-subprocess-worker`.
#
# SANDBOX_AGENT_BINARY names the agent each lease runs, so add the agent in
# a derived image (or a volume) and set that variable. The build context is
# the repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-subprocess-worker/Dockerfile -t trogon-sandbox-subprocess-worker:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-subprocess-worker --bin trogon-sandbox-subprocess-worker --package trogon-sandbox-egress-proxy --bin trogon-sandbox-egress-proxyd \
    && install -D /cargo-target/release/trogon-sandbox-subprocess-worker /out/trogon-sandbox-subprocess-worker \
    && install -D /cargo-target/release/trogon-sandbox-egress-proxyd /out/trogon-sandbox-egress-proxyd

# Trixie's glibc is newer than the one on the CI runners that compile the
# prebuilt binaries, so those binaries load here.
FROM debian:trixie-20261104-slim AS base

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends bubblewrap ca-certificates git \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 trogon \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin trogon

USER 65532:65532

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-subprocess-worker"]

# CI compiles each architecture natively and passes the binaries in as the
# `binaries` build context.
FROM base AS prebuilt

ARG TARGETARCH
COPY --from=binaries --chmod=0755 ${TARGETARCH}/trogon-sandbox-subprocess-worker /usr/local/bin/trogon-sandbox-subprocess-worker
COPY --from=binaries --chmod=0755 ${TARGETARCH}/trogon-sandbox-egress-proxyd /usr/local/bin/trogon-sandbox-egress-proxyd

FROM base AS runtime

COPY --from=builder /out/trogon-sandbox-subprocess-worker /usr/local/bin/trogon-sandbox-subprocess-worker
COPY --from=builder /out/trogon-sandbox-egress-proxyd /usr/local/bin/trogon-sandbox-egress-proxyd
