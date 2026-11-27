# syntax=docker/dockerfile:1
#
# SSH gateway: per-lease SSH sessions tunneled over NATS. The
# `sandbox-ssh-gateway` chart runs this image as `trogon-sandbox-ssh-gateway`
# and keeps the host key on a volume mounted at /data, which must be writable
# by uid 65532. The build context is the repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-ssh-gateway/Dockerfile -t trogon-sandbox-ssh-gateway:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-ssh-gateway --bin trogon-sandbox-ssh-gateway \
    && install -D /cargo-target/release/trogon-sandbox-ssh-gateway /out/trogon-sandbox-ssh-gateway

FROM debian:trixie-20261104-slim

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 trogon \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /out/trogon-sandbox-ssh-gateway /usr/local/bin/trogon-sandbox-ssh-gateway

USER 65532:65532

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-ssh-gateway"]
