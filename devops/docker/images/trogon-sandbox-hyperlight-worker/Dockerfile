# syntax=docker/dockerfile:1
#
# Hyperlight worker: runs sandbox leases as Wasm components in hypervisor
# guests. The `sandbox-hyperlight-worker` chart runs this image as
# `trogon-sandbox-hyperlight-worker`, privileged and with /dev/kvm mounted,
# so the image runs as root.
#
# The stock binary ships no hypervisor backend and exits at startup; build
# it with a backend before you deploy it. The build context is the
# repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-hyperlight-worker/Dockerfile -t trogon-sandbox-hyperlight-worker:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-hyperlight-worker --bin trogon-sandbox-hyperlight-worker \
    && install -D /cargo-target/release/trogon-sandbox-hyperlight-worker /out/trogon-sandbox-hyperlight-worker

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

COPY --from=builder /out/trogon-sandbox-hyperlight-worker /usr/local/bin/trogon-sandbox-hyperlight-worker

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-hyperlight-worker"]
