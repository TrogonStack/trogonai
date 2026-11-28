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

Every deployment references an image by tag and digest, `<repository>:<tag>@sha256:<digest>`.
Each image is a multi-platform index, and its SBOM and provenance attestations stay attached to that digest in the same repository.
The image workflow does not publish `latest`.

## Charts

Every push to `main` publishes each chart under `devops/helm/charts/` to `oci://ghcr.io/trogonstack/charts/trogonai`, and Helm appends the chart name.

| Chart | Reference |
|---|---|
| Umbrella | `oci://ghcr.io/trogonstack/charts/trogonai/trogon-sandbox` |
| Each other chart | `oci://ghcr.io/trogonstack/charts/trogonai/<chart>` |

| Tag | Points at |
|---|---|
| `0.0.0-sha-<commit>` | The charts of that full commit SHA, never moved |

Charts get no moving tags. The chart of a commit defaults to the images of that same commit, pinned by digest, and carries the `org.opencontainers.image.source` and `org.opencontainers.image.revision` annotations.

## Mirrors

The sandbox charts pull third-party images from `ghcr.io/trogonstack/mirrors/<upstream>/<path>`, with the upstream path and tag unchanged and pinned by digest.
`devops/oras/mirrors.txt` lists the pinned set, and the `mirror-container-images` workflow copies it with `oras copy -r`, so signatures and attestations move with each image.

| Mirror | Upstream |
|---|---|
| `ghcr.io/trogonstack/mirrors/docker-hub/authzed/spicedb` | `docker.io/authzed/spicedb` |
| `ghcr.io/trogonstack/mirrors/docker-hub/library/busybox` | `docker.io/library/busybox` |
| `ghcr.io/trogonstack/mirrors/docker-hub/library/nats` | `docker.io/library/nats` |
| `ghcr.io/trogonstack/mirrors/ghcr/oras-project/oras` | `ghcr.io/oras-project/oras` |
| `ghcr.io/trogonstack/mirrors/ghcr/project-zot/zot` | `ghcr.io/project-zot/zot` |

## Migration

| Image | Replaced by |
|---|---|
| `ghcr.io/trogonstack/trogonai/trogon-gateway` | `ghcr.io/trogonstack/images/trogonai/trogon-gateway` |

The legacy package is no longer published. Update consumers to the new path.