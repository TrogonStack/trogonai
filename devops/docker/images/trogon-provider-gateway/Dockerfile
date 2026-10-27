# syntax=docker/dockerfile:1

FROM rust:1.96.0-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

ARG TARGETARCH
ENV CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=/cargo-target
WORKDIR /src

RUN --mount=type=bind,source=rsworkspace,target=/src \
    --mount=type=cache,id=trogonai-rust-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=trogonai-rust-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=trogonai-rust-1.96.0-${TARGETARCH},target=/cargo-target,sharing=locked \
    cargo build --locked --release --package trogon-provider-gateway --bin trogon-provider-gateway \
    && install -D /cargo-target/release/trogon-provider-gateway /out/trogon-provider-gateway

FROM debian:bookworm-20260518-slim AS runtime

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 trogon \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin trogon \
    && install -d -o 10001 -g 10001 /var/log/trogon-provider-gateway

COPY --from=builder /out/trogon-provider-gateway /usr/local/bin/trogon-provider-gateway

ENV TROGON_PROVIDER_GATEWAY_HOST=0.0.0.0 \
    TROGON_PROVIDER_GATEWAY_PORT=8317 \
    TROGON_LOG_DIR=/var/log/trogon-provider-gateway
USER 10001:10001
EXPOSE 8317
STOPSIGNAL SIGTERM

HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl --fail --silent --show-error --max-time 2 "http://127.0.0.1:${TROGON_PROVIDER_GATEWAY_PORT}/-/readiness" || exit 1

ENTRYPOINT ["/usr/local/bin/trogon-provider-gateway"]
