# syntax=docker/dockerfile:1
#
# Snapshot builder: exports worker snapshots, gates them on a scan, signs
# them and records their digests. The `sandbox-snapshot-builder` chart runs
# this image as `trogon-sandbox-snapshot-builder`.
#
# The scan gate is on by default (SANDBOX_REQUIRE_SCAN_PASS=true) and runs
# `trivy`; `syft` adds an SBOM when present, and `cosign` signs when
# SANDBOX_COSIGN_KMS_URI is set. None of the three is in this image: add
# them in a derived image, or set SANDBOX_REQUIRE_SCAN_PASS=false to export
# without the gate. The build context is the repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-snapshot-builder/Dockerfile -t trogon-sandbox-snapshot-builder:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-snapshot-builder --bin trogon-sandbox-snapshot-builder \
    && install -D /cargo-target/release/trogon-sandbox-snapshot-builder /out/trogon-sandbox-snapshot-builder

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

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-snapshot-builder"]

# CI compiles each architecture natively and passes the binaries in as the
# `binaries` build context.
FROM base AS prebuilt

ARG TARGETARCH
COPY --from=binaries --chmod=0755 ${TARGETARCH}/trogon-sandbox-snapshot-builder /usr/local/bin/trogon-sandbox-snapshot-builder

FROM base AS runtime

COPY --from=builder /out/trogon-sandbox-snapshot-builder /usr/local/bin/trogon-sandbox-snapshot-builder
