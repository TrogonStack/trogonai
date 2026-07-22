# trogon-compactor — Context Compaction Service

## Problema que resuelve

`trogon-acp-runner` persiste el historial completo de cada sesión en NATS KV como `SessionState.messages: Vec<Message>`. Cada invocación de `run_prompt()` carga todos los mensajes acumulados y los envía al modelo. En sesiones largas (trabajo de horas con muchos tool calls) el conteo de tokens crece ilimitadamente hasta que la API devuelve `stop_reason == "max_tokens"`, momento en el que el runner falla sin recovery.

El `context_window_tokens()` en `agent.rs` está hardcodeado a 200 000 y no se usa como protección activa. No hay ningún mecanismo actual de poda o resumen.

## Qué hace este crate

Expone un servicio NATS que, dado un historial de mensajes, comprueba si se acerca al límite de tokens y, si es así, reemplaza los mensajes más antiguos con un resumen estructurado generado por el LLM. Los mensajes recientes se conservan verbatim.

```
trogon-acp-runner  (o cualquier servicio trogon)
        │  NATS request-reply  trogon.compactor.compact
        ▼
trogon-compactor  [este binario]
    └─ Compactor::compact_if_needed()
         └─ summarizer::generate_summary()
                 │  HTTP POST  (Bearer auth)
                 ▼
        trogon-secret-proxy → Anthropic API
```

## Algoritmo de compactación

El algoritmo está basado en la implementación de referencia de [pi-mono/coding-agent](https://github.com/badlogic/pi-mono/tree/main/packages/coding-agent).

### 1. Detección (`detector::should_compact`)

```
tokens_estimados > context_window - reserve_tokens
```

Se usa la heurística `chars / 4` (1 token ≈ 4 bytes UTF-8). Sobreestima intencionalmente para ser conservador.

Valores por defecto:
- `context_window`: 200 000 tokens
- `reserve_tokens`: 16 384 (espacio libre para nueva salida y tool results)
- `keep_recent_tokens`: 20 000 (mínimo de historial reciente que se conserva verbatim)

### 2. Punto de corte (`detector::find_cut_point`)

Recorre los mensajes **de atrás hacia adelante** acumulando tokens hasta alcanzar `keep_recent_tokens`. Desde esa posición busca hacia atrás el límite de turno válido más cercano: un mensaje `user` que **no** sea un pure tool-result (para no cortar en medio de un par tool_use / tool_result).

```
[ m0  m1  m2  m3  m4  m5  m6  m7 ]
                       ↑
                   cut_point = 5
                   ←── summarize ──│── keep verbatim ──→
```

### 3. Resumen (`summarizer::generate_summary`)

Se hace una única llamada al LLM (sin streaming, sin herramientas) con un prompt estructurado que pide este formato:

```markdown
## Goal
## Constraints & Preferences
## Progress
### Done / In Progress / Blocked
## Key Decisions
## Next Steps
## Critical Context
```

Si ya existe un summary previo en el primer mensaje (resumen incremental), se usa un prompt de actualización (`UPDATE_PROMPT`) que pide al modelo **mergear** la información nueva en lugar de regenerar desde cero.

La llamada va al endpoint `{PROXY_URL}/anthropic/v1/messages` con `Authorization: Bearer {token}`, igual que el resto de servicios trogon.

### 4. Reconstrucción

El historial resultante tiene esta estructura:

```
[0]  user:      <context-summary>\n{summary}\n</context-summary>
[1]  assistant: "I've reviewed the conversation summary..."
[2…] ...mensajes recientes conservados verbatim...
```

El marcador XML `<context-summary>` permite que compactaciones posteriores detecten el resumen existente y lo actualicen incrementalmente.

## Tipos propios (`types.rs`)

`Message` y `ContentBlock` son tipos propios del crate, deliberadamente con la misma representación serde que los de `trogon-agent-core`:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock { Text, Image, Thinking, ToolUse, ToolResult }
```

Esto permite que los mensajes guardados en NATS KV por `trogon-acp-runner` se deserialicen directamente aquí sin conversión. En la integración, el lado del runner sólo necesita un `From` impl.

## Interfaz NATS

**Subject**: `trogon.compactor.compact`

**Request**:
```json
{
  "messages": [{ "role": "user", "content": [...] }, ...]
}
```

**Response (éxito)**:
```json
{
  "messages": [...],
  "compacted": true,
  "tokens_before": 185432,
  "tokens_after": 22018
}
```

**Response (error)**:
```json
{ "error": "..." }
```

## Variables de entorno del binario

| Variable                       | Default                         | Descripción                                     |
|--------------------------------|---------------------------------|-------------------------------------------------|
| `NATS_URL`                     | `nats://localhost:4222`         | URL del servidor NATS                           |
| `PROXY_URL`                    | `http://localhost:8080`         | Base URL de trogon-secret-proxy                 |
| `ANTHROPIC_TOKEN`              | *(requerido)*                   | Bearer token para el proxy                      |
| `COMPACTOR_MODEL`              | `claude-haiku-4-5-20251001`     | Modelo usado para los resúmenes                 |
| `COMPACTOR_MAX_SUMMARY_TOKENS` | `8192`                          | Máximo de tokens en el resumen generado         |
| `COMPACTOR_CONTEXT_WINDOW`     | `200000`                        | Ventana de contexto del modelo objetivo         |
| `COMPACTOR_RESERVE_TOKENS`     | `16384`                         | Tokens reservados para nueva salida             |
| `COMPACTOR_KEEP_RECENT_TOKENS` | `20000`                         | Mínimo de tokens recientes conservados verbatim |

## Tests

23 tests en total, sin necesidad de API key real ni NATS en vivo:

- **20 unit tests** — lógica pura: estimación de tokens, detección del cut point, serialización de mensajes, construcción de prompts
- **3 integration tests** (`tests/compactor.rs`) — flujo completo con `httpmock` interceptando las llamadas al LLM:
  - No compacta cuando está bajo el umbral (verifica que no se hace ninguna llamada HTTP)
  - Compacta correctamente cuando supera el umbral
  - Usa prompt de actualización incremental cuando ya hay un resumen previo

## Estructura de archivos

```
src/
├── lib.rs          — Compactor struct, API pública
├── main.rs         — Binario: lee env vars, conecta a NATS, arranca el servicio
├── service.rs      — Capa NATS: request-reply sobre trogon.compactor.compact
├── types.rs        — Message, ContentBlock (propios, compatibles con trogon-agent-core)
├── error.rs        — CompactorError
├── tokens.rs       — Estimación de tokens (chars/4)
├── detector.rs     — should_compact(), find_cut_point()
├── serializer.rs   — Serialización de mensajes para el prompt de summarización
└── summarizer.rs   — Llamada al LLM, prompts, LlmConfig, AuthStyle
tests/
└── compactor.rs    — Tests de integración con httpmock
```

---

## Pendiente: Integración con trogon-acp-runner

> **Este crate está completo y desplegable como servicio independiente. La integración con `trogon-acp-runner` es un paso separado, no incluido en esta rama.**

### Qué hay que hacer en trogon-acp-runner

La integración es mínima: ~5 líneas en `run_prompt()` (`agent.rs:275`) y un `From` impl.

**1. Llamada al servicio de compactación** (en `run_prompt()`, antes de `run_chat_streaming`):

```rust
// Después de cargar el estado y antes de llamar al agent loop:
let messages = nats
    .request(
        "trogon.compactor.compact",
        serde_json::to_vec(&CompactRequest { messages: state.messages })?.into(),
    )
    .await?;

let compact_resp: CompactResponse = serde_json::from_slice(&messages.payload)?;
state.messages = compact_resp.messages;
```

**2. Conversión de tipos** (en `trogon-acp-runner` o en un módulo de integración):

```rust
impl From<trogon_agent_core::agent_loop::Message> for trogon_compactor::Message {
    fn from(m: trogon_agent_core::agent_loop::Message) -> Self {
        // serde round-trip: ambos tipos tienen la misma representación JSON
        serde_json::from_value(serde_json::to_value(m).unwrap()).unwrap()
    }
}
```

En la práctica, como los tipos tienen la misma representación serde, se puede hacer directamente con `serde_json::from_value(serde_json::to_value(&state.messages)?)` sin un `From` explícito.

### Consideraciones para la integración

- La llamada NATS añade una round-trip de red. Para minimizar latencia, el compactor debería desplegarse en el mismo cluster NATS.
- La compactación ocurre solo cuando es necesaria (`compacted: false` en la mayoría de requests → overhead despreciable).
- Si el compactor no está disponible, `run_prompt()` puede continuar sin compactación (degradación graceful) en lugar de fallar.
- El `context_window` del compactor debe coincidir con el modelo que usa el runner en esa sesión.
