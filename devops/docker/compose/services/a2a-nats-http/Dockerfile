FROM rust:1.93-slim AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json \
    -p a2a-auth-callout \
    -p a2a-gateway \
    -p a2a-bridge \
    -p a2a-nats-http \
    -p a2a-nats-server \
    -p a2a-smoke-test

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p a2a-nats-http && strip target/release/a2a-nats-http

FROM debian:bookworm-20250317-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/a2a-nats-http /usr/local/bin/a2a-nats-http

ENTRYPOINT ["/usr/local/bin/a2a-nats-http"]
