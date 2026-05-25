FROM rust:1.93-slim AS smoke-mint-builder

WORKDIR /build
COPY rsworkspace/Cargo.toml rsworkspace/Cargo.lock ./
COPY rsworkspace/crates/ crates/
RUN cargo build --release -p a2a-smoke-test && strip target/release/a2a-smoke-test

FROM debian:bookworm-slim

ARG NSC_VERSION=v2.12.0
RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    ca-certificates \
    curl \
    unzip \
    openssl \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL "https://github.com/nats-io/nsc/releases/download/${NSC_VERSION}/nsc-linux-amd64.zip" -o /tmp/nsc.zip \
    && unzip /tmp/nsc.zip -d /usr/local/bin \
    && mv /usr/local/bin/nsc /usr/local/bin/nsc-real \
    && rm /tmp/nsc.zip
COPY devops/docker/compose/services/a2a-bootstrap/nsc-wrapper.sh /usr/local/bin/nsc
RUN chmod +x /usr/local/bin/nsc /usr/local/bin/nsc-real

COPY --from=smoke-mint-builder /build/target/release/a2a-smoke-test /usr/local/bin/a2a-smoke-test
COPY scripts/a2a-nsc-bootstrap.sh scripts/a2a-auth-callout-bootstrap.sh /opt/a2a/scripts/
COPY devops/docker/compose/services/a2a-bootstrap/a2a-nats-server-smoke.conf /opt/a2a/a2a-nats-server-smoke.conf
COPY devops/docker/compose/services/a2a-bootstrap/entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh /opt/a2a/scripts/*.sh

ENTRYPOINT ["/entrypoint.sh"]
