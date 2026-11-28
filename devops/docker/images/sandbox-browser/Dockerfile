# syntax=docker/dockerfile:1
#
# Headless Chromium for browsing, screenshots and end-to-end tests inside a
# sandbox. Chromium runs as the unprivileged `browser` user; the lease's
# network policy must allow the hosts it visits.
#
#   docker build -f devops/docker/images/sandbox-browser/Dockerfile -t sandbox-browser:dev .
FROM node:22-bookworm-slim@sha256:be979e5523c76fa4d5ca4ebd3099078f220aa47ec39c48ad5b5df9314debd6eb

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"


ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    COREPACK_HOME=/opt/corepack \
    HOME=/tmp

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates \
      chromium \
      curl \
      fonts-liberation \
      fonts-noto-color-emoji \
      jq \
      procps \
 && rm -rf /var/lib/apt/lists/*

RUN corepack enable pnpm \
 && corepack prepare pnpm@10.3.0 --activate \
 && chmod -R a+rX /opt/corepack \
 && node -e "const [major, minor] = process.versions.node.split('.').map(Number); if (major < 22 || (major === 22 && minor < 7)) process.exit(1)" \
 && pnpm --version \
 && groupadd --gid 65532 browser \
 && useradd --no-log-init --uid 65532 --gid 65532 --shell /bin/bash --no-create-home --home-dir /tmp browser
USER 65532:65532
WORKDIR /tmp

ENV CHROME_BIN=/usr/bin/chromium
CMD ["sleep", "infinity"]
