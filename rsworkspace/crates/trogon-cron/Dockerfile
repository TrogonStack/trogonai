# ── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Cache dependencies before copying source
COPY Cargo.toml Cargo.lock* ./
# Stub out workspace members so cargo can resolve the graph
COPY crates/trogon-std/Cargo.toml   crates/trogon-std/Cargo.toml
COPY crates/trogon-nats/Cargo.toml  crates/trogon-nats/Cargo.toml
COPY crates/trogon-cron/Cargo.toml  crates/trogon-cron/Cargo.toml
RUN mkdir -p crates/trogon-std/src  && echo "pub fn _stub() {}" > crates/trogon-std/src/lib.rs
RUN mkdir -p crates/trogon-nats/src && echo "pub fn _stub() {}" > crates/trogon-nats/src/lib.rs
RUN mkdir -p crates/trogon-cron/src && echo "pub fn _stub() {}" > crates/trogon-cron/src/lib.rs \
    && echo "fn main() {}" > crates/trogon-cron/src/main.rs
RUN cargo build --release -p trogon-cron && rm -rf crates/*/src

# Build for real
COPY crates/ crates/
RUN touch crates/trogon-cron/src/main.rs crates/trogon-cron/src/lib.rs
RUN cargo build --release -p trogon-cron

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/trogon-cron /usr/local/bin/trogon-cron

ENV NATS_URL=nats://localhost:4222
ENV RUST_LOG=info

ENTRYPOINT ["trogon-cron"]
CMD ["serve"]
