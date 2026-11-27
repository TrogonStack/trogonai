# trogon-sandbox-dashboard

Read-only web dashboard for sandbox lease, snapshot, worker, and tag observability. The browser never holds NATS credentials; every control-plane read goes through a WebSocket gateway using a bearer session token.

The single-page app (`web/index.html`, `web/app.js`, `web/style.css`) is embedded in the binary at build time, so the server needs no files on disk.

## Main types

- `DashboardArgs`: command line flags (`--listen`, `--ws-gateway-url`, plus the shared observability flags).
- `WsGatewayUrl`: a parsed `ws://` or `wss://` URL; any other input is rejected with `WsGatewayUrlError`.
- `DashboardConfig`: the document served at `/config.json`, which tells the app which gateway to connect to.
- `router`, `serve_with_shutdown`, `run`: the axum router, the server loop, and the entry point used by the binary.

## How it fits

```
Browser  --GET /, /app.js, /style.css, /config.json-->  trogon-sandbox-dashboard
Browser  --WebSocket JSON frames-->  gateway  --NATS / JetStream-->  broker, workers, KV
```

The gateway lists and watches JetStream KV and NATS subjects on behalf of the session tenant, so each viewer only receives data for the tenant bound to their token. Process metrics and traces come from `trogon-sandbox-observability`.

The dashboard has no auth flows and no mutation surface; tag changes and lease cancellation stay in the admin tooling.

## Run

```bash
cargo run -p trogon-sandbox-dashboard -- --listen 127.0.0.1:8080
```

- `--listen <addr>` (default `127.0.0.1:8080`)
- `--ws-gateway-url <url>` (default `ws://127.0.0.1:8443/v1/ws`, exposed to the app through `/config.json`)

Open `http://127.0.0.1:8080/`, enter the gateway URL and a bearer token, and connect. Embedded mode accepts `?token=<bearer-token>`. The gateway URL is never taken from the page URL, so a crafted link cannot send a token to another server.

## Panels

1. Live leases: tenant, provider, image, state, started_at, ttl_seconds.
2. Snapshot inventory: digest, artifact_type, tenant, published_at, bytes.
3. Worker fleet: provider, worker_id, capability summary, cached digest count, last heartbeat.
4. Tag map: tenant, tag name, resolved digest, artifact type.

## Tests

```bash
cargo test -p trogon-sandbox-dashboard
```
