# ── Stage 1: chef — generate dependency recipe ──────────────────────────────
FROM rust:1.93-slim AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

# ── Stage 2: planner — capture dependency graph ─────────────────────────────
FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: builder — cached dependency build + final compile ──────────────
FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p trogon-source-github

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release -p trogon-source-github && \
    strip target/release/trogon-source-github

# ── Stage 4: runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-20250317-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /build/target/release/trogon-source-github /usr/local/bin/trogon-source-github

USER trogon

EXPOSE 8080

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/trogon-source-github"]
