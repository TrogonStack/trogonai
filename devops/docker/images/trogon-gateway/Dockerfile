# syntax=docker/dockerfile:1

# Compile on the build platform and cross-link for the target: rustc and cargo
# under QEMU arm64 emulation take hours, so only the runtime stage runs emulated.
FROM --platform=$BUILDPLATFORM rust:1.96.0-slim-bookworm AS builder

ARG TARGETARCH
ARG BUILDARCH
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# hadolint ignore=SC2086
RUN case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu; gnu=x86_64-linux-gnu; pkg="gcc-x86-64-linux-gnu libc6-dev-amd64-cross" ;; \
      arm64) triple=aarch64-unknown-linux-gnu; gnu=aarch64-linux-gnu; pkg="gcc-aarch64-linux-gnu libc6-dev-arm64-cross" ;; \
      *) echo "unsupported TARGETARCH=$TARGETARCH" >&2; exit 1 ;; \
    esac \
 && if [ "$TARGETARCH" = "$BUILDARCH" ]; then pkg=; fi \
 && apt-get update \
 && apt-get install -y --no-install-recommends build-essential cmake pkg-config $pkg \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$triple" \
 && echo "$triple" > /target.txt \
 && printf 'export CC_%s=%s-gcc\nexport CARGO_TARGET_%s_LINKER=%s-gcc\n' \
      "$(echo "$triple" | tr - _)" "$gnu" "$(echo "$triple" | tr 'a-z-' 'A-Z_')" "$gnu" > /cross.env

# The workspace arrives as a bind mount from the repository root context
# instead of a COPY, so the image never embeds a stale snapshot of the tree.
# The mount is read-only, so the target directory lives in a cache mount. Both
# platforms build at once, so each gets its own registry and target cache.
WORKDIR /src/rsworkspace
RUN --mount=type=bind,source=rsworkspace,target=/src/rsworkspace \
    --mount=type=cache,id=trogon-gateway-cargo-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=trogon-gateway-target-${TARGETARCH},target=/cargo-target,sharing=locked \
    . /cross.env \
 && CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_STRIP=symbols cargo build --locked --release --package trogon-gateway --bin trogon-gateway \
      --target "$(cat /target.txt)" --target-dir /cargo-target \
 && install -D "/cargo-target/$(cat /target.txt)/release/trogon-gateway" /out/trogon-gateway

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
