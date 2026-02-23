# Build context: rsworkspace/
# docker build -f crates/trogon-cron/Dockerfile -t trogon-cron .

# ── Chef stage ────────────────────────────────────────────────────────────────
FROM rust:1.83-slim AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Planner stage ─────────────────────────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Builder stage ─────────────────────────────────────────────────────────────
FROM chef AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release -p trogon-cron

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/trogon-cron /usr/local/bin/trogon-cron

ENV NATS_URL=nats://localhost:4222
ENV RUST_LOG=info

ENTRYPOINT ["trogon-cron"]
CMD ["serve"]
