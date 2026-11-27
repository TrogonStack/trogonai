# Container Images

Images follow the
[OCI artifact paths and tags ADR](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6497710059/README.md).

| Image | Platforms | Dockerfile |
|---|---|---|
| `ghcr.io/trogonstack/images/trogonai/trogon-gateway` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-gateway/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-aca-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-aca-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-actor-agent` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-actor-agent/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-broker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-broker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-credential-broker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-credential-broker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-dashboard` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-dashboard/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-daytona-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-daytona-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-docker-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-docker-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-egress-proxy` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-egress-proxy/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-hyperlight-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-hyperlight-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-k8s-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-k8s-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-kubevirt-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-kubevirt-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-modal-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-modal-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-snapshot-builder` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-snapshot-builder/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-ssh-gateway` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-ssh-gateway/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-subprocess-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-subprocess-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-substrate-worker` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-substrate-worker/Dockerfile` |
| `ghcr.io/trogonstack/images/trogonai/trogon-sandbox-ws-gateway` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-sandbox-ws-gateway/Dockerfile` |

| Tag | Points at |
|---|---|
| `canary` | The newest build of `main` |
| `<commit>` | The first build of that full commit SHA, never moved |

## Migration

| Image | Replaced by |
|---|---|
| `ghcr.io/trogonstack/trogonai/trogon-gateway` | `ghcr.io/trogonstack/images/trogonai/trogon-gateway` |

The legacy package is no longer published. Update consumers to the new path.