# trogon-sandbox-ws-gateway

WebSocket gateway that bridges browser-friendly clients to the sandbox NATS service. Browsers never receive raw NATS credentials; they hold a short-lived gateway session token instead.

## Main types

- `GatewayArgs`: command line flags of the binary (`--listen`, `--nats-url`, `--api-key-bucket`, `--cors-origin`, `--session-ttl-seconds`, the preview flags, plus the shared observability flags).
- `run`, `serve_with_shutdown`, `shutdown_signal`: the entry point used by the binary, the server loop, and the signal future that stops it. Failures are reported as `GatewayError`.
- `router`, `AppState`: the axum router (`POST /v1/sessions`, `GET /v1/ws`, the preview proxy) and the state it shares.
- `SandboxBackend`: the tool surface behind the socket. `NatsSandboxBackend` talks to the broker over NATS; tests plug in their own implementation.
- `SessionStore`: where session tokens live. `NatsKvSessionStore` keeps them in JetStream KV, `InMemorySessionStore` in memory.
- `PreviewState`, `PreviewKeys`, `PreviewClaims`: signed preview URLs and the proxy that serves them.
- `WsFrame`: the request, response, and event frames on the socket.

## How it fits

```text
Browser  --POST /v1/sessions, WebSocket JSON frames-->  gateway  --NATS / JetStream-->  broker, workers, KV
```

Front side: WebSocket plus the small `POST /v1/sessions` handshake. Back side: NATS only.

## Auth handshake

1. The client sends `POST /v1/sessions` with the header `X-Sandbox-API-Key: <tenant-api-key>`.
2. The gateway validates the key against the API key bucket and answers `201` with `{ "token": "...", "expires_at": { "secs_since_epoch": N, "nanos_since_epoch": 0 } }`.
3. The client opens `GET /v1/ws` as a WebSocket upgrade. Browsers set the subprotocol `bearer.<token>`; other clients may send `Authorization: Bearer <token>`.
4. The gateway resolves the session by the SHA-256 of the token and multiplexes sandbox tool calls on the socket.

A bad key format answers `401` with `invalid_api_key`. Session tokens default to a one hour lifetime (`--session-ttl-seconds`).

## WebSocket framing

One JSON object per text frame, tagged by `kind`:

```json
{ "kind": "request", "id": "1", "tool": "sandbox_list_capabilities", "payload": {} }
{ "kind": "response", "id": "1", "result": { } }
{ "kind": "response", "id": "1", "error": { "code": "...", "message": "..." } }
{ "kind": "event", "stream_id": "<lease-id>", "event": { } }
```

Tool names start with `sandbox_` and cover leases, commands, processes, files, snapshots, tags, drives, entities, operations, terminals, and observability subscriptions. A frame that is not a request is refused with `invalid_frame`.

## Run

```bash
cargo run -p trogon-sandbox-ws-gateway -- \
  --listen 127.0.0.1:8443 \
  --nats-url nats://127.0.0.1:4222
```

- `--api-key-store-url <nats-url>` when API keys live on a different NATS server.
- `--cors-origin https://app.example.com` (repeatable; the default is same-origin only).
- `--session-ttl-seconds 3600`

The gateway serves plain HTTP. Terminate TLS in front of it, because session tokens must not cross untrusted networks in the clear.

## Preview access control

When preview signing keys are configured, `sandbox_get_domain` also returns a `url` and `expires_at`, and the gateway serves that URL as an authenticated HTTP reverse proxy:

```text
{public_url}/v1/preview/{lease_id}/{port}/{token}/{path}
```

- `SANDBOX_PREVIEW_SIGNING_KEYS`: `kid=hexkey[,kid=hexkey...]`, each key at least 32 bytes. Unset disables previews and the route answers 404.
- `SANDBOX_PREVIEW_ACTIVE_KEY_ID`: the key that signs new tokens (default: the first listed). Keep the previous key listed while old tokens drain.
- `SANDBOX_PREVIEW_PUBLIC_URL` / `--preview-public-url`: externally reachable base URL. Required with keys.
- `SANDBOX_PREVIEW_TTL_SECONDS` (default 900), clamped to the session lifetime.

The token is HMAC-SHA256 over tenant, lease id, port, and expiry, verified in constant time. The proxy also requires the grant stored when the token was minted, then resolves the upstream through the broker on every request. Upgrade requests answer 501, and `Location` headers pointing at the upstream root are rewritten under the preview prefix.

## Tests

```bash
cargo test -p trogon-sandbox-ws-gateway
```

The NATS-backed tests start a server in a container through Docker.
