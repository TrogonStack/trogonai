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

| Image | Replaced by |
|---|---|
| `ghcr.io/trogonstack/trogonai/trogon-gateway` | `ghcr.io/trogonstack/images/trogonai/trogon-gateway` |

The legacy package is no longer published. Update consumers to the new path.