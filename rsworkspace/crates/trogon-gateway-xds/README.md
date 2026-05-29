# trogon-gateway-xds

Thin **xDS v3 interop bridge** that watches Trogon gateway configuration in NATS KV (`mcp-gateway-config`) and serves derived Envoy resources over gRPC ADS (State-of-the-World). External Envoy, Istio, agentgateway, or any xDS client can subscribe without duplicating MCP policy logic in mesh CRDs.

## Supported xDS version

- **Envoy xDS v3 only** (`envoy.service.discovery.v3`)
- **SotW ADS** via `StreamAggregatedResources`
- **Delta ADS** is stubbed (`UNIMPLEMENTED`)

## Served type URLs

| Type URL | Resource |
|---|---|
| `type.googleapis.com/envoy.config.listener.v3.Listener` | One listener per ingress port |
| `type.googleapis.com/envoy.config.route.v3.RouteConfiguration` | One route table per HTTPRoute equivalent |
| `type.googleapis.com/envoy.config.cluster.v3.Cluster` | One cluster per backend target |
| `type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment` | Endpoints for each backend |
| `type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC` | Inline RBAC filter configs derived from gateway CEL |

## ACK / NACK semantics

1. Each `DiscoveryResponse` carries a monotonic `nonce` and the KV snapshot `version_info`.
2. The server tracks, per stream and `type_url`, the last **sent** and **ACKed** version.
3. **ACK**: client returns the same `version_info` with matching `response_nonce` and no `error_detail`. The server will not push that version again.
4. **NACK**: client sets `error_detail`. The server clears the sent version so the same snapshot can be re-pushed.
5. When KV revision changes, a new `version_info` is pushed only for subscribed types that are not yet ACKed at that version.

## KV layout

Keys under bucket `mcp-gateway-config`:

```
xds/projection/{node_id}
```

Value: JSON `GatewayConfigSnapshot` (ingress ports, routes, backends, CEL policies).

## Run locally

```bash
cargo run -p trogon-gateway-xds
```

Environment:

| Variable | Default | Purpose |
|---|---|---|
| `TROGON_XDS_LISTEN` | `0.0.0.0:18000` | gRPC bind address |
| `NATS_URL` | `nats://127.0.0.1:4222` | NATS server |
| `TROGON_XDS_KV_BUCKET` | `mcp-gateway-config` | Config bucket |
| `TROGON_XDS_KV_PREFIX` | `xds/projection/` | Per-node projection keys |
| `TROGON_XDS_DEFAULT_NODE_ID` | `trogon-gateway` | Node id when `Node.id` is empty |

## Point Envoy at the bridge

Static bootstrap excerpt (ADS, v3):

```yaml
node:
  id: trogon-gateway
  cluster: trogon

dynamic_resources:
  ads_config:
    api_type: GRPC
    transport_api_version: V3
    grpc_services:
      - envoy_grpc:
          cluster_name: xds_cluster
  cds_config:
    ads: {}
  lds_config:
    ads: {}
  rds_config:
    ads: {}
  eds_config:
    ads: {}

static_resources:
  clusters:
    - name: xds_cluster
      type: STRICT_DNS
      connect_timeout: 1s
      load_assignment:
        cluster_name: xds_cluster
        endpoints:
          - lb_endpoints:
              - endpoint:
                  address:
                    socket_address:
                      address: trogon-gateway-xds
                      port_value: 18000
      typed_extension_protocol_options:
        envoy.extensions.upstreams.http.v3.HttpProtocolOptions:
          "@type": type.googleapis.com/envoy.extensions.upstreams.http.v3.HttpProtocolOptions
          explicit_http_config:
            http2_protocol_options: {}
```

## RBAC mapping vs ext_authz fallback

| CEL pattern | Translation |
|---|---|
| `mcp.method == "<value>"` | RBAC URL path prefix rule |
| `request.headers["<name>"] == "<value>"` | RBAC header matcher |
| Any other expression | **ext_authz** gRPC filter reference (policy stays in Trogon gateway) |

## Tests

```bash
cargo test -p trogon-gateway-xds
```

Includes mapping round-trips and an in-process ADS ACK cycle.
