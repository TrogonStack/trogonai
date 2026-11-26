# Sandbox observability

`trogon-sandbox-observability` holds the tracing and metrics setup shared by sandbox
binaries: optional OTLP span export and a Prometheus `/metrics` endpoint.

- `ObservabilityArgs` is a `clap` argument group with `--otlp-endpoint`
  (`SANDBOX_OTLP_ENDPOINT`) and `--metrics-addr` (`SANDBOX_METRICS_ADDR`). The
  metrics address is parsed into a `SocketAddr` when arguments are parsed.
- `init_tracing` installs the `tracing` subscriber with an `EnvFilter` (default
  `info`) and, when an endpoint is configured, exports spans over OTLP HTTP. The
  returned `TracingGuard` flushes the exporter on drop, waiting at most three
  seconds. `init_tracing_with_env` takes an explicit environment source.
- `default_registry` builds a Prometheus registry containing `sandbox_build_info`;
  `gather_text` renders it, and `serve_metrics` / `spawn_metrics_server` expose it
  at `/metrics`.
- `metrics_addr` derives the default scrape address, loopback on
  `service_port + 1000`. The `BROKER`, `WS_GATEWAY`, `DOCKER_WORKER`, and related
  constants list the documented service ports.
- `bootstrap` combines the above for a binary's `main`. It must run inside a Tokio
  runtime.
- `ObservabilityError` is the typed failure for exporter construction, registry
  operations, and metrics listener errors.

The crate has no dependency on other sandbox crates. Binaries such as the lease
broker and the provider workers call `bootstrap` once at startup.
