# Vendored xDS / Envoy protos

Minimal subset copied from upstream for `trogon-gateway-xds` code generation. Do not pull the full `data-plane-api` tree as a dependency.

## Upstream commits

| Source repository | Commit | Paths vendored under `proto/` |
|---|---|---|
| [envoyproxy/data-plane-api](https://github.com/envoyproxy/data-plane-api) | `84e84367f2560cdb47b9bb78fd3e615feb80c3e4` | `envoy/service/discovery/v3/*`, `envoy/config/{listener,route,cluster,endpoint,core,accesslog,trace}/v3/*`, `envoy/extensions/filters/{http/{rbac,router,ext_authz},network/http_connection_manager}/v3/*`, `envoy/extensions/transport_sockets/tls/v3/*`, `envoy/type/**`, `envoy/annotations/*` |
| [cncf/xds](https://github.com/cncf/xds) | `dba9d589def2cd10099a3a64887d859188c2f57a` | `xds/**`, `udpa/annotations/**` |
| [googleapis/googleapis](https://github.com/googleapis/googleapis) | `96eefa39128a99f4dff4a57a64a1177b858f1b3f` | `google/api/{annotations,http}.proto`, `google/rpc/status.proto` |
| [envoyproxy/protoc-gen-validate](https://github.com/envoyproxy/protoc-gen-validate) | `414042a5ff2e98dc47f8161937316a25b1da5bba` | `validate/validate.proto` |

`google/protobuf/*` well-known types are resolved from the local `protoc` installation include path at build time (`PROTOC_INCLUDE` or Homebrew `protobuf` prefix).

## Compiled entrypoints (`build.rs`)

- `envoy/service/discovery/v3/ads.proto` — ADS gRPC service
- `envoy/config/listener/v3/listener.proto`
- `envoy/config/route/v3/route.proto`
- `envoy/config/cluster/v3/cluster.proto`
- `envoy/config/endpoint/v3/endpoint.proto`
- `envoy/extensions/filters/http/rbac/v3/rbac.proto`
