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
    -p a2a-nats-server \
    -p a2a-nats-agent \
    -p a2a-smoke-test

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p a2a-gateway && strip target/release/a2a-gateway

FROM debian:bookworm-20250317-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    procps \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/a2a-gateway /usr/local/bin/a2a-gateway

ENTRYPOINT ["/usr/local/bin/a2a-gateway"]
