# Container Images

Images follow the
[OCI artifact paths and tags ADR](https://github.com/straw-hat-team/adr/blob/main/src/adrs/6497710059/README.md).

| Image | Platforms | Dockerfile |
|---|---|---|
| `ghcr.io/trogonstack/images/trogonai/trogon-gateway` | `linux/amd64` | `devops/docker/images/trogon-gateway/` |

| Tag | Points at |
|---|---|
| `canary` | The newest build of `main` |
| `<commit>` | The first build of that full commit SHA, never moved |

## Deprecated

| Image | Replaced by |
|---|---|
| `ghcr.io/trogonstack/trogonai/trogon-gateway` | `ghcr.io/trogonstack/images/trogonai/trogon-gateway` |

Deprecated images no longer receive builds. Their existing tags stay until
nothing pulls them, then the package is deleted.
