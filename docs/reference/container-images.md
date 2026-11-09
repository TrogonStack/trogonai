# Container Images

Images follow the
[OCI artifact paths and tags ADR](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6497710059/README.md).

| Image | Platforms | Dockerfile |
|---|---|---|
| `ghcr.io/trogonstack/images/trogonai/trogon-gateway` | `linux/amd64`, `linux/arm64` | `devops/docker/images/trogon-gateway/` |

| Tag | Points at |
|---|---|
| `canary` | The newest build of `main` |
| `<commit>` | The first build of that full commit SHA, never moved |

## Migration

The legacy image reference `ghcr.io/trogonstack/trogonai/trogon-gateway` was
retired after tracked deployments moved to the ADR-compliant path above. The
legacy package is no longer published; update consumers to the new path.