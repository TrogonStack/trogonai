# ── Stage 1: chef — generate dependency recipe ──────────────────────────────
FROM rust:1.96.0-slim-bookworm AS chef

RUN cargo install cargo-chef --locked

WORKDIR /build

# ── Stage 2: planner — capture dependency graph ─────────────────────────────
FROM chef AS planner

# Every workspace member the root manifest declares, not just the ones this
# binary depends on: cargo refuses to read a workspace whose members are
# missing, so a recipe prepared without them fails before any crate is built.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY cli/ cli/
COPY wasm-components/ wasm-components/

RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: builder — cached dependency build + final compile ──────────────
FROM chef AS builder

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json -p channel-bridge-telegram

COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY cli/ cli/
COPY wasm-components/ wasm-components/

RUN cargo build --release -p channel-bridge-telegram && \
    strip target/release/channel-bridge-telegram

# ── Stage 4: runtime ────────────────────────────────────────────────────────
FROM debian:bookworm-20260518-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --no-create-home --shell /usr/sbin/nologin trogon

COPY --from=builder /build/target/release/channel-bridge-telegram /usr/local/bin/channel-bridge-telegram

USER trogon

STOPSIGNAL SIGTERM

ENTRYPOINT ["/usr/local/bin/channel-bridge-telegram"]
