FROM rust:1.93-slim AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p trogon-cron

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p trogon-cron && \
    strip target/release/trogon-cron

FROM debian:bookworm-20250317-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /build/target/release/trogon-cron /usr/local/bin/trogon-cron

USER trogon

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-cron", "serve"]
