FROM rust:1.96.0-slim-bookworm AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY cli/ cli/
COPY wasm-components/ wasm-components/

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p a2a-gateway

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY cli/ cli/
COPY wasm-components/ wasm-components/

RUN cargo build --release -p a2a-gateway --bin a2a-gateway && \
    strip target/release/a2a-gateway

FROM debian:bookworm-20260518-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --no-create-home --shell /usr/sbin/nologin --uid 1000 trogon

COPY --from=builder /build/target/release/a2a-gateway /usr/local/bin/a2a-gateway

USER trogon

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/a2a-gateway"]
