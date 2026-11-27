# syntax=docker/dockerfile:1
#
# Egress proxy: the sidecar that enforces a sandbox's egress policy. Pass
# the pushed image to the docker worker as SANDBOX_EGRESS_PROXY_IMAGE (the
# chart's `egressProxyImage`). The k8s worker's sidecar pulls
# `sandbox-egress-proxy:latest`, so tag or mirror the image under that name
# for it.
#
# The default user is the proxy uid 65531, which the redirect rules exempt.
# The image also carries `sh`, `iptables` and `setpriv`: the docker sidecar
# starts as root, installs the redirect rules and then drops to uid 65531
# before it runs `sandbox-egress-proxyd`, and the k8s init container runs the
# same rules from `/bin/sh`. Both binary names point at the same file. The
# build context is the repository root:
#
#   docker build -f devops/docker/images/trogon-sandbox-egress-proxy/Dockerfile -t trogon-sandbox-egress-proxy:dev .
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
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-sandbox-egress-proxy --bin trogon-sandbox-egress-proxyd \
    && install -D /cargo-target/release/trogon-sandbox-egress-proxyd /out/trogon-sandbox-egress-proxyd

FROM debian:trixie-20261104-slim

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates iptables util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 65532 trogon \
    && useradd --uid 65532 --gid 65532 --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /out/trogon-sandbox-egress-proxyd /usr/local/bin/trogon-sandbox-egress-proxyd

RUN ln -s trogon-sandbox-egress-proxyd /usr/local/bin/sandbox-egress-proxyd

USER 65531:65531

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-sandbox-egress-proxyd"]
