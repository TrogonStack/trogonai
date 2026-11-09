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

The legacy package was removed after all tracked deployments migrated to the
ADR-compliant path above.