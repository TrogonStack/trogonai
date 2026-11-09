# syntax=docker/dockerfile:1

FROM rust:1.96.0-slim-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*

ARG TARGETARCH
ENV CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=/cargo-target
WORKDIR /src

RUN --mount=type=bind,source=rsworkspace,target=/src,rw \
    --mount=type=cache,id=trogonai-rust-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=trogonai-rust-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=trogonai-rust-1.96.0-${TARGETARCH},target=/cargo-target,sharing=locked \
    find . -type f -exec touch {} + \
    && CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-gateway --bin trogon-gateway \
    && install -D /cargo-target/release/trogon-gateway /out/trogon-gateway

FROM debian:bookworm-20260518-slim AS runtime

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /out/trogon-gateway /usr/local/bin/trogon-gateway

USER trogon

EXPOSE 8080

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-gateway", "serve"]
