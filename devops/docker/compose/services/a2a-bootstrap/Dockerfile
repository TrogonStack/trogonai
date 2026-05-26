FROM rust:1.93-slim AS smoke-mint-builder

WORKDIR /build
COPY rsworkspace/Cargo.toml rsworkspace/Cargo.lock ./
COPY rsworkspace/crates/ crates/
RUN cargo build --release -p a2a-smoke-test -p a2a-redaction \
    && strip target/release/a2a-smoke-test \
    && strip target/release/a2a-sign-bundle

FROM smoke-mint-builder AS tier3-bundles-builder

ENV A2A_PACK_SKILLS_ROOT=/build
RUN rustup target add wasm32-unknown-unknown
RUN bash crates/a2a-pack/skills/build.sh

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
COPY --from=smoke-mint-builder /build/target/release/a2a-sign-bundle /usr/local/bin/a2a-sign-bundle
COPY --from=tier3-bundles-builder /build/crates/a2a-pack/skills/pii_regex_redactor.wasm /opt/a2a/tier3-staging/pii_regex_redactor.wasm
COPY --from=tier3-bundles-builder /build/crates/a2a-pack/skills/smoke_tier3_refuse.wasm /opt/a2a/tier3-staging/smoke_tier3_refuse.wasm
COPY devops/docker/compose/services/a2a-bootstrap/tier3-pii-regex-redactor.manifest.json /opt/a2a/tier3-manifests/pii-regex-redactor.manifest.json
COPY rsworkspace/crates/a2a-pack/skills/smoke-tier3-refuse.manifest.json /opt/a2a/tier3-manifests/smoke-tier3-refuse.manifest.json
COPY rsworkspace/crates/a2a-pack/policies /opt/a2a/policies
COPY scripts/a2a-nsc-bootstrap.sh scripts/a2a-auth-callout-bootstrap.sh /opt/a2a/scripts/
COPY devops/docker/compose/services/a2a-bootstrap/a2a-nats-server-smoke.conf /opt/a2a/a2a-nats-server-smoke.conf
COPY devops/docker/compose/services/a2a-bootstrap/entrypoint.sh /entrypoint.sh

RUN chmod +x /entrypoint.sh /opt/a2a/scripts/*.sh

ENTRYPOINT ["/entrypoint.sh"]
