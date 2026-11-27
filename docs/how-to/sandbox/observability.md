# Scrape sandbox metrics and export traces

Each sandbox binary exposes Prometheus metrics and can export OTLP traces. Control plane traffic stays on NATS. Only the `/metrics` scrape endpoint and the trace exporter use HTTP.

## Find the scrape target

Each binary serves Prometheus text on `127.0.0.1:<service_port + 1000>`.

| Binary | Service port | Scrape URL |
|--------|-------------:|------------|
| `trogon-sandbox-broker` | 8090 | `http://127.0.0.1:9090/metrics` |
| `trogon-sandbox-credential-broker` | 8091 | `http://127.0.0.1:9091/metrics` |
| `trogon-sandbox-snapshot-builder` | 8095 | `http://127.0.0.1:9095/metrics` |
| `trogon-sandbox-ws-gateway` | 8443 | `http://127.0.0.1:9443/metrics` |
| `trogon-sandbox-ssh-gateway` | 2222 | `http://127.0.0.1:3222/metrics` |
| `trogon-sandbox-dashboard` | 8080 | `http://127.0.0.1:9080/metrics` |
| `trogon-sandbox-docker-worker` | 8100 | `http://127.0.0.1:9100/metrics` |
| `trogon-sandbox-k8s-worker` | 8101 | `http://127.0.0.1:9101/metrics` |
| `trogon-sandbox-hyperlight-worker` | 8102 | `http://127.0.0.1:9102/metrics` |

The other workers use consecutive service ports from 8103 (Daytona, KubeVirt, subprocess, ACA, V8 and Wasm, Substrate, Modal), then 8111 (Firecracker), 8112 (E2B), 8113 (Vercel) and 8114 (SPACE). The scrape port is always the service port plus 1000.

Override any target with `--metrics-addr` or `SANDBOX_METRICS_ADDR`. To scrape from another host, set the address to a non-loopback interface.

## Know the metric families

- **OCI**: `sandbox_oci_push_bytes_total`, `sandbox_oci_pull_bytes_total` and `sandbox_oci_referrers_fallback_total`, plus push and pull duration histograms.
- **Broker**: `sandbox_broker_lease_dispatch_total`, `sandbox_broker_active_leases`, `sandbox_broker_locality_hit_total` and `sandbox_broker_digest_pin_mismatch_total`.
- **Snapshot builder**: `snapshot_builder_pushed_total`, `snapshot_builder_failed_total` and queue and in-flight gauges.
- **Firecracker worker**: `sandbox_firecracker_spoofed_frames_total`, the frames the TAP guard dropped for a forged source MAC or a VLAN tag. A lease's count is added when its network is torn down, so a running lease is not counted yet. Any increase means a guest tried to forge frames.
- **Volumes**: `sandbox_volumes_bytes_in_total`, `sandbox_volumes_bytes_out_total`, pull and push durations and cache hits.

## Scrape with Prometheus

Add the targets you run to a scrape job:

```yaml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: sandbox
    static_configs:
      - targets:
          - 127.0.0.1:9090   # broker
          - 127.0.0.1:9095   # snapshot builder
          - 127.0.0.1:9443   # ws gateway
```

Query a metric such as `sandbox_broker_lease_dispatch_total` to confirm the scrape works.

## Export traces

Every sandbox binary accepts an OTLP endpoint:

```sh
--otlp-endpoint http://127.0.0.1:4318/v1/traces
# or
export SANDBOX_OTLP_ENDPOINT=http://127.0.0.1:4318/v1/traces
```

The value is the full OTLP/HTTP traces URL. When it is unset, tracing stays on stdout only and nothing is exported. Point it at any collector or tracing backend that accepts OTLP over HTTP.

## Follow a lease with its correlation id

The broker assigns a `correlation_id` when it accepts a lease request and records it on the lease. Search your trace backend for `correlation_id=<value>` to follow one lease across the broker, the worker and the snapshot builder.

## Check the metrics on a running stack

With NATS, the broker, the snapshot builder and a worker running:

```sh
export SANDBOX_OTLP_ENDPOINT=http://127.0.0.1:4318/v1/traces

cargo run -p trogon-sandbox-broker -- --nats-url nats://127.0.0.1:4222 --insecure-dev
cargo run -p trogon-sandbox-snapshot-builder -- --nats-url nats://127.0.0.1:4222
cargo run -p trogon-sandbox-docker-worker -- --nats-url nats://127.0.0.1:4222
```

Request a lease, then scrape:

```sh
curl -s http://127.0.0.1:9090/metrics | grep sandbox_broker_
curl -s http://127.0.0.1:9095/metrics | grep snapshot_builder_
```
