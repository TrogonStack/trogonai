# Provider gateway reference

The HTTP foundation for provider egress. Provider integrations, OAuth, account pools,
inference routes and management routes are not implemented yet.

## Audience and listener

The service serves the **Private** audience only. It has no Public audience, edge
route or debug port. Deployments must authenticate calling workloads through mesh
mTLS or workload identity before application routes are introduced. The process
does not provide workload authentication itself.

HTTP/1.1 and plaintext HTTP/2 (h2c) share the same listener. The native default is
`127.0.0.1:8317`. Deployments that need a network listener must set
`TROGON_PROVIDER_GATEWAY_HOST=0.0.0.0` (or an appropriate IPv6 address). Name the
container and Service ports `private`; use `appProtocol: kubernetes.io/h2c` for a
plaintext HTTP/2 Service. Mesh policy, NetworkPolicy and Kubernetes probes belong
to deployment configuration.

## Configuration

Precedence, from lowest to highest: defaults, TOML selected by `--config`,
environment variables, CLI flags. Unknown TOML fields are rejected. Effective
host and port values are validated after applying overrides.

| Setting | TOML field | Environment variable | CLI flag | Default |
| --- | --- | --- | --- | --- |
| Bind IP address | `host` | `TROGON_PROVIDER_GATEWAY_HOST` | `--host` | `127.0.0.1` |
| TCP port | `port` | `TROGON_PROVIDER_GATEWAY_PORT` | `--port` | `8317` |

The host must be an IP address. Port `0` requests an ephemeral port; the selected
address appears in the startup log.

```toml
host = "127.0.0.1"
port = 8317
```

```sh
cargo run -p trogon-provider-gateway -- --config gateway.toml
```

## Operational endpoints

| Path | Ready | Draining |
| --- | --- | --- |
| `/-/liveness` | `200` | `200` |
| `/-/readiness` | `200` | `503` |

GET and HEAD return status only, without a response body or dependency calls.
Readiness changes before graceful shutdown starts draining in-flight requests.
Operational paths are unauthenticated and create no server spans. The server
stops accepting connections when draining starts. `/`, `/healthz`, `/metrics`,
inference and management paths return `404`.

## Telemetry

The binary uses the shared `trogon-telemetry` OTLP/HTTP pipeline for traces,
metrics and structured logs. All signals use the same resource. Service identity
follows `OTEL_SERVICE_NAME`, then `service.name` in `OTEL_RESOURCE_ATTRIBUTES`,
then the fallback `trogon-provider-gateway`.

```sh
OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:4318 \
OTEL_EXPORTER_OTLP_PROTOCOL=http/json \
OTEL_SERVICE_NAME=trogon-provider-gateway \
OTEL_RESOURCE_ATTRIBUTES=service.namespace=trogonai \
cargo run -p trogon-provider-gateway
```

Use a collector address reachable from the process or container. The exporter
supports signal-specific OTLP endpoint configuration through the standard SDK
environment variables. `RUST_LOG` controls filtering; keep `info` enabled for
request spans and completion logs. `TROGON_LOG_DIR` selects file logs. The shared
metrics reader exports every 30 seconds and flushes during shutdown.

Non-operational requests, including `404` responses, continue incoming W3C
`traceparent` and `tracestate`. Baggage is not extracted. Server spans and
`http.server.request.duration` record normalized methods, status, protocol and
route templates when available. Duration uses seconds and covers response-body
completion, body failures and cancellation. Completion logs carry the request's
trace and span identifiers. Raw paths, query strings, request bodies,
authorization and arbitrary headers are excluded.

Every path under `/-/` bypasses request spans, completion logs and duration
metrics, including unknown operational paths. The existing endpoint responses
are unchanged. SIGTERM drains requests before the shared providers flush.


## Container image

From the repository root:

```sh
mise run images:build trogon-provider-gateway
```

An optional final argument supplies a local image tag. The default is
`trogon-provider-gateway:<commit>`. This command builds locally without publishing.
It requires a clean worktree so the tag and revision identify the built source.
The Dockerfile is in `devops/docker/images/trogon-provider-gateway/`.

The container runs as UID/GID `10001:10001` and binds `0.0.0.0:8317` by default.
Its entrypoint accepts the same CLI flags as the native binary. Deployments own
readiness and liveness probe configuration and must target the effective bind
address and port. The image declares no Docker health check because CLI overrides
can change the listener, including an IPv6-only address or an ephemeral port.
`SIGTERM` initiates graceful shutdown. BuildKit refreshes workspace file timestamps
inside its disposable build mount before compiling, without changing host files,
so older source timestamps cannot reuse stale workspace artifacts. It stores
compiler output in a cache mount, with incremental compilation disabled. Share that cache per
platform and remove it after verification; prune it before it exceeds 40 GB.

## Architecture references

- [Service audiences, listeners, health checks and telemetry](https://straw-hat-team.github.io/adr/adrs/1008684393/README.html)
- [Operational asset layout](https://straw-hat-team.github.io/adr/adrs/2078011537/README.html)
