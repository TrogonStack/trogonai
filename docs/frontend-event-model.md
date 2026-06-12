# Blueprint de Event Modeling — Frontend de TrogonAI

> **Qué es este documento.** El diseño del frontend web de TrogonAI expresado con
> [Event Modeling](https://eventmodeling.org/posts/what-is-event-modeling/) (Adam Dymitruk),
> mapeado 1:1 a las APIs y subjects que **ya existen** en la rama `platform`.
> Sirve como contrato compartido entre la UI y el backend: cada *command* apunta a un
> endpoint/método real, cada *event* a un `TranscriptEntry`/`SessionNotification`/subject NATS
> real, y cada *view* a su fuente de datos real.
>
> **Por qué Event Modeling aquí.** TrogonAI ya es event-native: los `transcripts.*` viven en
> JetStream como log durable y NATS pub-sub mueve los eventos en vivo. El frontend no inventa un
> modelo de datos paralelo: **lee y escribe el modelo de eventos que el backend ya emite.**
> El estilo de producto (control plane multi-modelo, human-in-the-loop, audit trail de primera
> clase) está inspirado en plataformas tipo [8090](https://www.8090.ai/) / `platform.claude.com`,
> ya citadas en [`console-api-plan.md`](./console-api-plan.md).

---

## 1. Las 3 piezas (mapeadas a lo que ya existe)

| Pieza | Definición Event Modeling | Dónde vive en `platform` |
|---|---|---|
| **Command** | Intención del usuario de cambiar el estado | ACP methods (`session/new`, `session/prompt`, `session/cancel`, `session/set_model`, ext `session/export\|import`) sobre `acp-nats-ws`; REST de `trogon-console` (`POST/PUT/DELETE` de agents/skills/environments/credentials) |
| **Event** | Hecho que cambió el estado, guardado en orden cronológico | `TranscriptEntry` en JetStream (`crates/trogon-transcript/src/entry.rs`); `SessionNotification` en vivo; cambios de KV de `trogon-registry` |
| **View** (read model) | Proyección pasiva de eventos acumulados, para consumo de la UI | Vista de conversación, catálogo de agents, token/cost meter, historial de sesiones, monitor de runners vivos |

**Regla base:** un command **no** escribe la UI directamente. Produce un **event**; las **views**
se reconstruyen a partir de los events. Eso es lo que hace el transcript durable y el replay posibles.

---

## 2. Los 4 patrones aplicados a TrogonAI

| Patrón | Forma | Instancia concreta en TrogonAI |
|---|---|---|
| **State Change** | UI → command → event | Usuario envía prompt → `session/prompt` → `TranscriptEntry::Message{role:user}` + (al cerrar turno) `Message{role:assistant}` / `ToolCall` |
| **State View** | events → view → UI | `TranscriptEntry`/`SessionNotification` acumulados → vista de conversación, token meter, `/cost` |
| **Translation** | sistema externo → event de dominio | Webhooks de GitHub/Slack/Discord en `trogon-gateway` (:8000) → eventos de dominio que arrancan sesiones |
| **Automation** | proceso en background gestiona integraciones vía "todo list" | `trogon-router` (rutea events a actores), `trogon-webhook-dispatcher` (entrega saliente), spawn de sub-agentes (`TranscriptEntry::SubAgentSpawn`) |

---

## 3. Swimlanes (carriles por actor)

Una sola línea de tiempo horizontal; los carriles verticales separan responsabilidades por actor.
Por la Ley de Conway, estos carriles también son la propuesta de **división del trabajo de la UI**.

```
 Carril                         Responsabilidad en la UI                Fuente de datos backend
 ─────────────────────────────  ──────────────────────────────────────  ─────────────────────────────
 1. Usuario (Wireframe/UI)      Pantallas, inputs, render de streaming   (es el frontend mismo)
 2. Console (control plane)     CRUD agents/skills/envs/credentials      trogon-console REST  :8090
 3. Bridge ACP                  Ciclo de sesión + streaming en vivo      acp-nats-ws WS       :9000
 4. Runners                     Ejecución del agente por modelo          acp.claude|grok|openrouter|codex
 5. Transcript / Registry       Log durable + estado vivo de runners     transcripts.> (JS), trogon.registry (KV)
 6. Gateway / Automation        Ingesta externa + automatizaciones       trogon-gateway :8000, router, dispatcher
```

---

## 4. La línea de tiempo (información backbone)

Cada celda es un *slice* vertical `command → event → view`. De izquierda a derecha = tiempo.

```
TIEMPO ───────────────────────────────────────────────────────────────────────────────────────▶

 Usuario      [Crear Agent]     [Abrir chat]    [Enviar prompt]      [Aprobar tool]   [Cambiar modelo]
                  │                  │                │                    │                 │
                  ▼ cmd             ▼ cmd            ▼ cmd                ▼ cmd            ▼ cmd
 Console     POST /agents          │           ┌────┘                    │            (export+import)
                  │                 │           │                        │                 │
 Bridge ACP       │          session/new   session/prompt        permission resp     session/set_model
                  │                 │           │                        │                 │
 Runners          │                 │      tool-use loop ───────────────▶│                 │
                  │                 │           │                        │                 │
                  ▼ evt            ▼ evt        ▼ evt                    ▼ evt            ▼ evt
 Transcript  AgentCreated     SessionStarted  Message{user}         ToolCall(approved)  Message+import
                                              AgentMessageChunk…
                                              ToolCall…
                                              Message{assistant}
                  │                 │           │                        │                 │
                  ▼ view           ▼ view      ▼ view                   ▼ view           ▼ view
 Views      Catálogo agents   Sesión abierta  Conversación + token    Conversación      Selector de
                                              meter (streaming)        actualizada        modelo + coste
```

---

## 5. Catálogo de Commands

> Cada command lista: actor, dónde se envía (endpoint/método real), payload mínimo y el/los events que produce.

### Plano de interacción (vía `acp-nats-ws` :9000, WebSocket → ACP sobre NATS)

| Command | Método ACP | Subject NATS subyacente | Produce event(s) |
|---|---|---|---|
| **Inicializar cliente** | `initialize` | `acp.{prefix}.session.*.agent.initialize` | (capabilities; no transcript) |
| **Abrir sesión** | `session/new` | `acp.{prefix}.session.{id}.agent.new` | `SessionStarted` (lógico) |
| **Enviar prompt** | `session/prompt` | `acp.{prefix}.session.{id}.agent.prompt` | `Message{role:user}` → stream de `SessionNotification` → `Message{role:assistant}` / `ToolCall` |
| **Cancelar turno** | `session/cancel` | `acp.{prefix}.session.{id}.agent.cancel` | corta el stream; turno marcado cancelado |
| **Cambiar modelo (mismo runner)** | `session/set_model` | `…agent.set_model` | `CurrentModeUpdate` / metadata |
| **Cambiar de runner** | ext `session/export` + `session/import` | `…ext.*` (ver `acp-nats/src/agent/ext_method.rs`) | nueva sesión con historial portado |
| **Responder permiso** | respuesta a `RequestPermission` | canal de notificación de la sesión | desbloquea/aborta el `ToolCall` pendiente |

Streaming de respuesta: el cliente se suscribe a `acp.{prefix}.session.{id}.client.session.update`
(deltas) y `…client.prompt_response` (resultado final). Variantes de `SessionNotification`
(de `agent-client-protocol` v0.10.4): `AgentMessageChunk`, `AgentThoughtChunk`, `ToolCall`,
`ToolCallUpdate`, `Plan`, `AvailableCommandsUpdate`, `CurrentModeUpdate`.

### Plano de gestión (vía `trogon-console` REST :8090)

| Command | Endpoint | Produce event (lógico) |
|---|---|---|
| **Crear agent** | `POST /agents` | `AgentCreated` |
| **Editar agent (nueva versión)** | `PUT /agents/:id` | `AgentVersioned` |
| **Borrar agent** | `DELETE /agents/:id` | `AgentDeleted` |
| **Crear skill / versión** | `POST /skills`, `POST /skills/:id/versions` | `SkillCreated` / `SkillVersioned` |
| **Crear/editar/archivar environment** | `POST/PUT /environments`, `POST /environments/:id/archive` | `EnvironmentCreated/Updated/Archived` |
| **Añadir/quitar credencial** | `POST/DELETE /environments/:id/credentials[/:cred_id]` | `CredentialAdded/Removed` |

> Nota: en el control plane los "events" son lógicos (mutaciones de KV `CONSOLE_*`). El log durable
> de events ricos es el de **sesiones** (`transcripts.>`). El frontend trata ambos con el mismo modelo.

### Plano de translation/automation (no iniciado por la UI, pero la UI los observa)

| Origen | Command implícito | Event de dominio |
|---|---|---|
| Webhook GitHub/Slack/Discord (`trogon-gateway` :8000) | `IngestWebhook` | evento de dominio que dispara sesión (patrón Translation) |
| Router (`trogon-router`) | `RouteEvent` | `TranscriptEntry::RoutingDecision` |
| Sub-agente | `SpawnSubAgent` | `TranscriptEntry::SubAgentSpawn` |

---

## 6. Catálogo de Events (la verdad durable)

Tipos reales en `crates/trogon-transcript/src/entry.rs` (tag serde `type`, guardados en JetStream
bajo `transcripts.{actor_type}.{actor_key}.{session_id}`):

| Event (`TranscriptEntry`) | Campos | Qué view alimenta |
|---|---|---|
| `Message` | `role` (user/assistant/system/tool), `content`, `timestamp`, `tokens?` | Conversación, token meter |
| `ToolCall` | `name`, `input`, `output`, `duration_ms`, `timestamp` | Conversación (tarjeta de tool), timeline de actividad |
| `RoutingDecision` | `from`, `to`, `reasoning`, `timestamp` | Vista de automatización / debug de routing |
| `SubAgentSpawn` | `parent`, `child`, `capability`, `timestamp` | Árbol de sub-agentes, monitor |

Eventos de estado vivo (no en transcript, sino KV con heartbeat) desde `trogon-registry`:
`AgentCapability { nats_subject, capabilities, model_list, metadata }` → monitor de runners vivos.

**Propiedad de los events:** son inmutables y ordenados. El frontend **nunca** edita un event;
emite un nuevo command que produce un nuevo event (principio de *Replaceability*).

---

## 7. Catálogo de Views (read models)

> Cada view declara: de qué events se proyecta y cuál es su fuente concreta.

| View | Se proyecta de | Fuente backend |
|---|---|---|
| **Conversación de sesión** | `Message`, `ToolCall`, `SubAgentSpawn` + `SessionNotification` en vivo | replay `GET /sessions/:id` (histórico) + WS `…client.session.update` (vivo) |
| **Token / Cost meter** | `Message.tokens`, desglose de Session | `Session { input_tokens, output_tokens, cache_read_tokens, cache_write_tokens }` (`console-api-plan.md`) |
| **Catálogo de Agents** | `AgentCreated/Versioned/Deleted` | `GET /agents`, `GET /agents/:id/versions` |
| **Historial de Sesiones** | todos los `transcripts.*` de un actor | `GET /sessions`, `GET /agents/:id/sessions` |
| **Monitor de Runners** | heartbeats del registry | KV `trogon.registry` (vía console o gateway) |
| **Environments & Credentials** | `Environment*` / `Credential*` | `GET /environments`, `…/credentials`, `GET /mcp-registry` |
| **Vista de Routing/Automatización** | `RoutingDecision`, `SubAgentSpawn` | transcript filtrado por tipo |

---

## 8. Especificaciones Given-When-Then

> El corazón del Event Modeling: cada command y cada view se especifica con escenarios. Estos son
> directamente los **tests de aceptación** del frontend (y el contrato contra el backend).

### Command: Enviar prompt
```
GIVEN  una sesión abierta {prefix, session_id} con un runner vivo en el registry
WHEN   el usuario envía "arregla el bug en foo.rs"  (session/prompt)
THEN   se publica TranscriptEntry::Message{role:user, content:"arregla…"}
AND    la UI se suscribe a acp.{prefix}.session.{id}.client.session.update
AND    renderiza AgentMessageChunk como texto en streaming
AND    al cerrar el turno persiste Message{role:assistant} y/o ToolCall en transcripts.>
AND    el token meter incrementa con Message.tokens
```

### Command: Aprobar un tool (human-in-the-loop)
```
GIVEN  un turno en curso que emite RequestPermission para `write_file(foo.rs)`
WHEN   el usuario pulsa "Aprobar"
THEN   se responde la notificación de permiso con allow
AND    el runner ejecuta el tool y emite ToolCall{name:"write_file", output, duration_ms}
WHEN   en cambio el usuario pulsa "Rechazar"
THEN   el ToolCall no se ejecuta y el turno continúa/aborta según el modo
```

### Command: Cambiar de runner (Claude → Grok)
```
GIVEN  una sesión activa en acp.claude con historial
WHEN   el usuario elige "Grok" en el selector de modelo
THEN   se invoca ext session/export sobre el runner actual → historial portable (JSON)
AND    se abre session/new en acp.grok
AND    se invoca ext session/import con el historial
AND    la view de conversación continúa sin pérdida (nuevos prefix+session_id)
```

### Command: Crear agent
```
GIVEN  el catálogo de agents
WHEN   el usuario crea "PR Reviewer" (model, system_prompt, skill_ids, tools)
THEN   POST /agents persiste en KV CONSOLE_AGENTS con version=1, status=Active
AND    el catálogo (GET /agents) lo lista
WHEN   el usuario lo edita
THEN   PUT /agents/:id crea una nueva versión (no sobrescribe) — visible en GET /agents/:id/versions
```

### View: Conversación de sesión (replay + vivo)
```
GIVEN  un session_id existente
WHEN   la UI abre la sesión
THEN   GET /sessions/:id hidrata la conversación desde transcripts.{actor}.{key}.{id} (histórico)
AND    si la sesión está Running, se conecta al WS para deltas en vivo
AND    cada TranscriptEntry se renderiza según su `type` (Message / ToolCall / RoutingDecision / SubAgentSpawn)
```

### View: Token / Cost meter
```
GIVEN  una sesión con N turnos
WHEN   se proyecta el read model de coste
THEN   suma input/output/cache_read/cache_write tokens del Session
AND    se actualiza en vivo con cada Message.tokens entrante
```

---

## 9. Storyboard (wireframes)

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ TrogonAI Console                                            [Runners: ● 4 vivos]│
├───────────────┬────────────────────────────────────────────────────────────────┤
│ ◉ Chat        │  Sesión sevt_XGg9JkV   Agent: PR Reviewer   Modelo: [Claude ▼] │
│ ○ Agents      │ ──────────────────────────────────────────────────────────────│
│ ○ Sessions    │  👤  arregla el bug en foo.rs                                   │
│ ○ Environments│  🤖  Voy a leer el archivo…            ▏(streaming)            │
│ ○ Skills      │      ┌─ tool: read_file(foo.rs)  142ms ──────────┐             │
│ ○ Credentials │      └────────────────────────────────────────────┘            │
│ ○ Automations │  ⚠  Permiso: write_file(foo.rs)   [Aprobar] [Rechazar]        │
│               │ ──────────────────────────────────────────────────────────────│
│               │  in 1.2k · out 850 · cache 3.4k          [Cancelar turno ✕]    │
│  [+ Nueva]    │  > escribe un mensaje…                                  [Enviar]│
└───────────────┴────────────────────────────────────────────────────────────────┘
  Carril Usuario          Carril Bridge ACP (vivo) + Transcript (replay)
```

---

## 10. Completitud: origen → destino de cada campo

> Principio de *Completeness*: ningún dato aparece o desaparece sin origen y destino documentados.

| Campo en la UI | Origen (event/endpoint) | Destino (view) |
|---|---|---|
| Texto del asistente | `AgentMessageChunk` (vivo) → `Message{assistant}` (durable) | Conversación |
| Tarjeta de tool | `ToolCall{name,input,output,duration_ms}` | Conversación |
| Contador de tokens | `Message.tokens` / `Session.*_tokens` | Token meter |
| Estado del runner | heartbeat `AgentCapability` (registry) | Monitor de runners |
| Lista de agents | `GET /agents` (KV `CONSOLE_AGENTS`) | Catálogo |
| Versión del agent | `PUT /agents/:id` → `GET /agents/:id/versions` | Selector de versión |
| Decisión de routing | `RoutingDecision{from,to,reasoning}` | Vista de automatización |

---

## 11. Slices de implementación (costo constante)

Cada fila es un slice vertical completo `command → event → view`, entregable e independiente.
*Constant Cost*: el orden no cambia el costo de cada slice; se pueden priorizar/subcontratar por separado.

| # | Slice | Commands | Events | Views | Backend que consume |
|---|---|---|---|---|---|
| 1 | **Leer sesiones** (read-only) | — | `transcripts.*` | Historial + Conversación (replay) | `GET /sessions`, `GET /sessions/:id` |
| 2 | **Chat en vivo** | `session/new`, `session/prompt`, `session/cancel` | `Message`, `ToolCall`, `SessionNotification` | Conversación (streaming) + token meter | `acp-nats-ws` WS :9000 |
| 3 | **Human-in-the-loop** | responder `RequestPermission` | `ToolCall` (approved/denied) | Banner de permiso | `permission-ui-design.md` |
| 4 | **Multi-modelo** | `session/set_model`, ext `export/import` | sesión portada | Selector de modelo | runners + ext methods |
| 5 | **Control plane: Agents** | `POST/PUT/DELETE /agents` | `AgentCreated/Versioned` | Catálogo + versiones | `trogon-console` REST |
| 6 | **Control plane: Envs/Creds/Skills** | CRUD respectivos | `Environment*/Credential*/Skill*` | Gestión | `trogon-console` REST + `mcp-registry` |
| 7 | **Monitor de runners** | — | heartbeats | Dashboard de runners vivos | KV `trogon.registry` |
| 8 | **Automatización** (translation) | observar | `RoutingDecision`, `SubAgentSpawn`, webhooks | Vista de automatización | `trogon-gateway`, `trogon-router`, dispatcher |

**Orden recomendado:** 1 → 2 → 3 → 5 → 4 → 6 → 7 → 8.
(Empezar por read-only de sesiones da valor inmediato y valida el modelo de events antes de escribir.)

---

## Referencias

- Event Modeling: https://eventmodeling.org/posts/what-is-event-modeling/
- Inspiración de producto (control plane multi-modelo): https://www.8090.ai/ , `platform.claude.com`
- API de control plane: [`console-api-plan.md`](./console-api-plan.md)
- UI de permisos: [`permission-ui-design.md`](./permission-ui-design.md)
- Tipos de event: `rsworkspace/crates/trogon-transcript/src/entry.rs`
- Bridge ACP / ext methods: `rsworkspace/crates/acp-nats/src/agent/`
- Subjects NATS y arquitectura: `rsworkspace/CLAUDE.md`
</content>
</invoke>
