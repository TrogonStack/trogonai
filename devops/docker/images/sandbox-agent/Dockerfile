# syntax=docker/dockerfile:1
#
# Default image for coding workloads: git, a compiler toolchain, a shell with
# the usual search and JSON tools, and Python and Node for scripts. Build it
# once, push it to a registry your workers can pull from, and pass it as the
# lease image. The docker worker's default image is `sandbox-agent:dev`:
#
#   docker build -f devops/docker/images/sandbox-agent/Dockerfile -t sandbox-agent:dev .
FROM node:22-bookworm-slim@sha256:be979e5523c76fa4d5ca4ebd3099078f220aa47ec39c48ad5b5df9314debd6eb

ARG OCI_REVISION
LABEL org.opencontainers.image.source="https://github.com/TrogonStack/trogonai" \
    org.opencontainers.image.revision="${OCI_REVISION}" \
    org.opencontainers.image.licenses="Apache-2.0"


ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    GIT_TERMINAL_PROMPT=0 \
    COREPACK_HOME=/opt/corepack

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash \
      build-essential \
      ca-certificates \
      curl \
      git \
      jq \
      less \
      openssh-client \
      patch \
      procps \
      python3 \
      python3-pip \
      python3-venv \
      ripgrep \
      unzip \
      xz-utils \
 && rm -rf /var/lib/apt/lists/*

RUN corepack enable pnpm \
 && corepack prepare pnpm@10.3.0 --activate \
 && chmod -R a+rX /opt/corepack \
 && node -e "const [major, minor] = process.versions.node.split('.').map(Number); if (major < 22 || (major === 22 && minor < 7)) process.exit(1)" \
 && pnpm --version

# Commits made inside a sandbox need an identity; override per lease with env.
RUN git config --system user.name "sandbox" \
 && git config --system user.email "sandbox@localhost" \
 && git config --system init.defaultBranch main \
 && git config --system --add safe.directory '*'

RUN groupadd --gid 65532 sandbox \
 && useradd --no-log-init --uid 65532 --gid 65532 --shell /bin/bash --create-home --home-dir /home/sandbox sandbox \
 && mkdir -p /workspace \
 && chown 65532:65532 /workspace

ENV HOME=/home/sandbox
USER 65532:65532
WORKDIR /workspace
CMD ["sleep", "infinity"]
