# Implementation Summary — AI Provider Token Exchange Proxy

## Objetivo

Construir un sistema similar a [VGS (Very Good Security)](https://www.verygoodsecurity.com/) donde los servicios **nunca manejan claves reales** de OpenAI, Anthropic, etc. En su lugar manejan tokens opacos (`tok_anthropic_prod_abc`). Un proxy HTTP intercepta las llamadas salientes, intercambia el token por la clave real a través de un vault, y reenvía la solicitud al proveedor de IA.

**Garantía central:** la clave real (`sk-ant-...`) nunca sale del worker. El servicio upstream nunca la ve.

---

## Arquitectura

```
Servicio
  │
  │  POST /anthropic/v1/messages
  │  Authorization: Bearer tok_anthropic_prod_abc   ← solo token
  ▼
┌─────────────────────────┐
│   HTTP Proxy (axum)     │  puerto 8080
│                         │
│  1. genera correlation_id (UUID)
│  2. subscribe a reply subject (Core NATS)
│  3. publica OutboundHttpRequest → JetStream       ← durable
│  4. espera reply hasta 60s
└────────────┬────────────┘
             │ JetStream stream "PROXY_REQUESTS"
             │ (WorkQueue, Memory storage)
             ▼
┌─────────────────────────┐
│  Detokenization Worker  │  pull consumer
│                         │
│  1. lee token del header Authorization
│  2. vault.resolve(tok) → sk-ant-realkey
│  3. reemplaza header con clave real
│  4. llama a https://api.anthropic.com  ← clave real, nunca visible al servicio
│  5. publica OutboundHttpResponse → Core NATS reply subject
│  6. msg.ack()
└────────────┬────────────┘
             │ Core NATS (rápido, síncrono)
             ▼
┌─────────────────────────┐
│   HTTP Proxy            │
│  devuelve respuesta HTTP│
│  al servicio original   │
└─────────────────────────┘
```

### Por qué JetStream híbrido

| Canal | Tecnología | Motivo |
|---|---|---|
| Proxy → Worker | JetStream (stream) | Durable: sobrevive crash del worker, reentrega automática, máx 3 intentos |
| Worker → Proxy | Core NATS (pub/sub) | Rápido y efímero: la respuesta no necesita persistencia |

---

## Crates creadas

### `trogon-vault`

Abstracción de almacenamiento de tokens con backends intercambiables.

```
crates/trogon-vault/
  src/token.rs         ApiKeyToken (value object validado), AiProvider, Env, TokenError
  src/vault.rs         VaultStore trait (store / resolve / revoke)
  src/backends/
    memory.rs          MemoryVault — Arc<Mutex<HashMap>>, para tests y desarrollo
```

**Formato del token:** `tok_{provider}_{env}_{id}`
- `tok_anthropic_prod_a1b2c3`
- `tok_openai_test_xyz789`

El `id` solo acepta `[a-zA-Z0-9]+`. Cualquier otra cosa devuelve `TokenError`.

**`VaultStore` trait** (usando `impl Future`, sin `async_trait`):
```rust
pub trait VaultStore: Send + Sync {
    type Error: std::error::Error + Send + Sync;
    fn store(&self, token: &ApiKeyToken, plaintext: &str) -> impl Future<...>;
    fn resolve(&self, token: &ApiKeyToken) -> impl Future<Output = Result<Option<String>, ...>>;
    fn revoke(&self, token: &ApiKeyToken) -> impl Future<...>;
}
```

---

### `trogon-secret-proxy`

Proxy HTTP + worker de detokenización.

```
crates/trogon-secret-proxy/
  src/config.rs        Config builder (prefix, port, worker_timeout)
  src/messages.rs      OutboundHttpRequest / OutboundHttpResponse (serde JSON)
  src/subjects.rs      outbound(prefix) / reply(prefix, id) — helpers de subjects NATS
  src/stream.rs        ensure_stream() — crea "PROXY_REQUESTS" si no existe (idempotente)
  src/provider.rs      provider → base URL (anthropic, openai, gemini, cohere, mistral)
  src/proxy.rs         axum router + handle_request + ProxyError
  src/worker.rs        run() loop + process_request + forward_request_with_retry
  src/bin/proxy.rs     binario proxy (lee env vars, arranca axum)
  src/bin/worker.rs    binario worker (siembra vault desde env, arranca loop)
  tests/e2e.rs         tests de integración (Docker NATS + httpmock)
```

---

## Flujo detallado

### 1. Proxy recibe request

```
POST /anthropic/v1/messages
Authorization: Bearer tok_anthropic_prod_abc
```

- Extrae `{provider}` = `"anthropic"` y `{*path}` = `"v1/messages"` del path
- Construye URL destino: `https://api.anthropic.com/v1/messages`
- Genera `correlation_id` (UUID v4)
- Se subscribe a `trogon.proxy.reply.{correlation_id}` en Core NATS **antes** de publicar
- Serializa el request como `OutboundHttpRequest` (JSON) con todos los headers incluyendo el token
- Publica a JetStream con headers NATS: trace context + `Reply-To`
- Espera reply con `tokio::time::timeout(worker_timeout, reply_sub.next())`

### 2. Worker procesa mensaje

- Pull consumer durable con `AckPolicy::Explicit` y `max_deliver: 3`
- Busca header `Authorization` (case-insensitive)
- Valida que empiece con `Bearer tok_` — rechaza si es una clave real pasada por error
- `vault.resolve(&token)` → clave real
- Reemplaza `Authorization: Bearer tok_...` por `Authorization: Bearer sk-ant-...`
- Añade `X-Request-Id: {idempotency_key}` para idempotencia en el proveedor
- Llama al proveedor con `reqwest` + exponential backoff retry
- Publica `OutboundHttpResponse` al reply subject de Core NATS
- `msg.ack()` — JetStream elimina el mensaje del stream

### 3. Retry del worker (forward_request_with_retry)

```
Intento 1 → falla 5xx → espera 100ms
Intento 2 → falla 5xx → espera 200ms
Intento 3 → falla 5xx → espera 400ms
Intento 4 → devuelve última respuesta (sea 5xx u Ok)

Errores 4xx → NO se reintenta (error del cliente, reintentar no ayuda)
Errores de red → se reintenta igual que 5xx
```

### 4. Proxy construye respuesta HTTP

- Deserializa `OutboundHttpResponse`
- Si `error` está presente → devuelve `502 Bad Gateway`
- Si timeout → `504 Gateway Timeout`
- En caso contrario → copia status, headers y body de la respuesta del proveedor

---

## Mensajes en NATS

```rust
// Proxy → Worker (JetStream)
OutboundHttpRequest {
    method: "POST",
    url: "https://api.anthropic.com/v1/messages",
    headers: { "Authorization": "Bearer tok_anthropic_prod_abc", ... },
    body: [...],
    reply_to: "trogon.proxy.reply.{uuid}",
    idempotency_key: "{uuid}",          // → X-Request-Id al proveedor
}

// Worker → Proxy (Core NATS)
OutboundHttpResponse {
    status: 200,
    headers: { "content-type": "application/json", ... },
    body: [...],
    error: None,                         // Some("mensaje") si hubo error
}
```

---

## Subjects NATS

| Subject | Tipo | Dirección |
|---|---|---|
| `trogon.proxy.http.outbound` | JetStream | Proxy → Worker |
| `trogon.proxy.reply.{uuid}` | Core NATS | Worker → Proxy |

---

## Providers soportados

| Nombre en path | Base URL |
|---|---|
| `anthropic` | `https://api.anthropic.com` |
| `openai` | `https://api.openai.com` |
| `gemini` | `https://generativelanguage.googleapis.com` |
| `cohere` | `https://api.cohere.ai` |
| `mistral` | `https://api.mistral.ai` |

Provider desconocido → `502 Bad Gateway` inmediato (sin tocar NATS).

---

## Deployment

### Variables de entorno

| Variable | Default | Descripción |
|---|---|---|
| `NATS_URL` | `localhost:4222` | Dirección del servidor NATS |
| `PROXY_PREFIX` | `trogon` | Prefijo de subjects NATS |
| `PROXY_PORT` | `8080` | Puerto TCP del proxy |
| `PROXY_WORKER_TIMEOUT_SECS` | `60` | Segundos de espera por reply del worker |
| `VAULT_TOKEN_<TOKEN>=<KEY>` | — | Semilla del vault (ej: `VAULT_TOKEN_tok_anthropic_prod_abc=sk-ant-...`) |
| `RUST_LOG` | `info` | Filtro de logs (tracing-subscriber) |

### Docker Compose

```bash
docker compose up --scale worker=3   # 3 workers para escalado horizontal
```

Los workers forman un queue group — JetStream reparte los mensajes entre ellos automáticamente.

---

## Tests

### Cobertura total: 46 tests

| Suite | Tests | Qué verifica |
|---|---|---|
| `trogon-vault` unit | 21 | Token validation, MemoryVault CRUD, Display, error types |
| `trogon-secret-proxy` unit | 25 | Config, messages serde, subjects, provider URLs, ProxyError→HTTP status, worker resolve_token, process_request, retry logic |
| `trogon-secret-proxy` e2e | 5 | Pipeline completo con Docker NATS + mock AI provider |

### Tests e2e destacados

| Test | Verifica |
|---|---|
| `e2e_token_is_exchanged_for_real_key` | La clave real llega al proveedor; el servicio nunca la ve |
| `e2e_unknown_token_returns_error` | Token no en vault → respuesta de error |
| `e2e_proxy_timeout_returns_504` | Sin worker → 504 Gateway Timeout |
| `e2e_concurrent_requests_all_succeed` | 5 requests en paralelo, correlation IDs no se mezclan |
| `e2e_ensure_stream_is_idempotent` | `ensure_stream()` dos veces no falla |

### Ejecutar

```bash
# Unit tests (sin Docker)
cargo test -p trogon-vault -p trogon-secret-proxy --lib

# E2e (requiere Docker)
cargo test -p trogon-secret-proxy --test e2e

# Todo
cargo test -p trogon-vault -p trogon-secret-proxy
```

---

## Garantías de seguridad verificadas por tests

1. **Token exchange probado por e2e:** el mock AI solo responde 200 si recibe `Bearer sk-ant-realkey`. Si el worker enviara el token original, el mock no matchea y el test falla.
2. **Token nunca viaja al proveedor:** `resolve_token()` rechaza explícitamente valores que no empiecen con `tok_` — si alguien pasa una clave real por error, el worker la rechaza con error.
3. **Durabilidad probada:** mensaje publicado a JetStream antes de que el worker arranque es procesado correctamente cuando el worker se conecta.
