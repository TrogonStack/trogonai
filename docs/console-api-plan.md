# Plan de implementación: trogon-console API

> Inspirado en platform.claude.com — backend listo para ser mostrado como una plataforma manejable.
> Sin frontend. Sin UI. Solo el plano de control REST.

## Contexto

Ahorita trogon es una caja negra: recibe webhooks, rutea eventos, corre actores. No hay forma de ver qué pasa ni gestionar nada sin tocar código. Este plan agrega el **plano de control** que le falta.

**Cinco conceptos a exponer:**
- **Sessions** — observabilidad de sesiones de agentes (read-only)
- **Agents** — definiciones de agentes (system prompt, modelo, tools, skills)
- **Skills** — instrucciones reutilizables versionadas, inyectadas al runtime
- **Environments** — configuraciones de entorno (networking, packages, metadata)
- **Credentials** — tokens/OAuth por environment para MCP servers

---

## Rama

`feat/console-api` creada desde `feat/durable-agent-runs`

**Razón:** `feat/durable-agent-runs` tiene `trogon-vault` (vault completo con Hashicorp backend + rotación), `trogon-agent/src/session.rs` con estado durable real (Running/Idle/Failed/Recovering), y `promise_store.rs` con checkpoints. Desde `main` las Sessions quedarían incompletas y el vault habría que construirlo desde cero.

---

## Qué se crea y qué se modifica

### Nuevo crate: `crates/trogon-console/`

API REST pura en Axum, backed por NATS JetStream KV.

```
crates/trogon-console/
  Cargo.toml
  src/
    main.rs
    server.rs                  ← build_router() + serve()
    models/
      agent.rs                 ← AgentDefinition { id, name, description, status(Active|Inactive), version, model, system_prompt, skill_ids, tools }
      skill.rs                 ← Skill { id, name, description, provider }, SkillVersion { version, content, created_at }
      environment.rs           ← Environment { id, name, description, type(Cloud|Local), networking, packages, metadata }
      credential.rs            ← Credential { id, name, type(OAuth|Bearer), mcp_server_url, status }, CredentialVault { id, env_id }
      mcp_registry.rs          ← McpServer { name, url } — lista estática o configurable de MCP servers conocidos
    store/
      agents.rs                ← NATS KV CRUD sobre CONSOLE_AGENTS
      skills.rs                ← NATS KV CRUD sobre CONSOLE_SKILLS (key: {id}::{version})
      environments.rs          ← NATS KV CRUD sobre CONSOLE_ENVS
      credentials.rs           ← NATS KV CRUD sobre CONSOLE_CREDS
    routes/
      agents.rs                ← GET/POST/PUT/DELETE /agents, GET/PUT/DELETE /agents/:id
      sessions.rs              ← GET /sessions, GET /sessions/:id  (lee transcript stream, solo lectura)
      skills.rs                ← GET/POST /skills, GET/POST /skills/:id/versions
      environments.rs          ← GET/POST /environments, GET/PUT/DELETE /environments/:id
      credentials.rs           ← GET/POST /environments/:id/credentials, DELETE /environments/:id/credentials/:cred_id
```

#### NATS KV buckets nuevos

| Bucket | Contenido | Key format |
|--------|-----------|------------|
| `CONSOLE_AGENTS` | Definiciones de agentes | `{agent_id}` |
| `CONSOLE_SKILLS` | Skills con versiones | `{skill_id}::{version}` |
| `CONSOLE_ENVS` | Environments | `{env_id}` |
| `CONSOLE_CREDS` | Credenciales | `{env_id}::{cred_id}` |

El stream `transcripts.>` ya existe — sessions solo lo lee, no crea nada nuevo.

#### Modelo de Session (campos completos detectados en fotos)

```
Session {
  id: String,                  // sevt_XGg9JkV
  agent_id: String,
  agent_name: String,
  environment_id: String,
  status: Running | Idle,
  duration_ms: u64,
  input_tokens: u32,
  output_tokens: u32,
  cache_read_tokens: u32,
  cache_write_tokens: u32,
  created_at: timestamp,
  events: Vec<SessionEvent>,   // debug view — eventos tipados (Running/User/Model/Agent/Idle)
}
```

#### Endpoints

```
GET    /agents
POST   /agents
GET    /agents/:id
PUT    /agents/:id                    ← crea nueva versión (Save new version)
DELETE /agents/:id
GET    /agents/:id/sessions           ← sessions filtradas por agente (tab Sessions en detalle)
GET    /agents/:id/versions           ← historial de versiones del agente

GET    /sessions
GET    /sessions/:id

GET    /skills
POST   /skills
GET    /skills/:id
GET    /skills/:id/versions
POST   /skills/:id/versions

GET    /environments
POST   /environments
GET    /environments/:id
PUT    /environments/:id
DELETE /environments/:id
POST   /environments/:id/archive      ← Archive es distinto a Delete
GET    /environments/:id/vault        ← vault como entidad propia con su ID
GET    /environments/:id/credentials
POST   /environments/:id/credentials
DELETE /environments/:id/credentials/:cred_id

GET    /mcp-registry                  ← lista de MCP servers conocidos (Airtable, Amplitude, Asana…)
```

---

### Modificaciones a `crates/trogon-actor/`

El gancho para que los skills sean procesados en runtime.

**Archivos nuevos:**
- `src/skill_loader.rs` — dado `Vec<skill_id>`, carga `Vec<SkillVersion>` del KV `CONSOLE_SKILLS`

**Archivos modificados:**
- `src/runtime.rs` — antes de llamar a `handle()`, llama `skill_loader`, concatena el contenido de skills al system prompt
- `src/context.rs` — `ActorContext` gana campo `injected_skills: Vec<String>`

**Comportamiento:**
- Sin skills asignadas → comportamiento idéntico al actual, cero regresión
- Con skills asignadas → su contenido se inyecta en el system prompt antes del LLM call

---

## Orden de implementación

| Paso | Qué | Dependencias |
|------|-----|--------------|
| 1 | Modelos (`models/`) | Ninguna — base de todo |
| 2 | Stores (`store/`) | Modelos |
| 3 | Routes + server (`trogon-console`) | Stores + modelos |
| 4 | `skill_loader` en `trogon-actor` | Modelos de skill |
| 5 | Integración actor ↔ skills en runtime | skill_loader |

---

## Gaps detectados al comparar con las fotos (añadidos al plan)

| Área | Gap | Resolución |
|------|-----|------------|
| Sessions | Faltaban 6 campos: `status`, `agent_id/name`, `environment_id`, `duration_ms`, tokens desglosados | Añadidos al modelo Session |
| Sessions | Faltaba `GET /agents/:id/sessions` (tab Sessions en detalle de agente) | Añadido a endpoints |
| Agents | Faltaba versionado — "Save new version", selector `Version: v1` | Añadidos `version`, `GET /agents/:id/versions`, PUT crea versión |
| Agents | Faltaba campo `status: Active \| Inactive` | Añadido al modelo |
| Environments | Faltaban campos `description` y `type: Cloud \| Local` | Añadidos al modelo |
| Environments | `Archive` es distinto a `Delete` | Añadido `POST /environments/:id/archive` |
| Credentials | El vault es entidad propia con ID (`v1t_011CZs2K4VxPprWqv3M28czK`) | Añadido `CredentialVault { id, env_id }` y `GET /environments/:id/vault` |
| MCP Registry | Buscador de MCP servers conocidos al añadir credencial | Añadidos `mcp_registry.rs` y `GET /mcp-registry` |

---

## Lo que NO se toca

- Crates de webhooks/sources (`trogon-source-*`)
- `trogon-router`
- `trogon-registry` (sigue siendo el registro live, es ortogonal)
- `trogon-gateway`
- `trogon-pr-actor`

---

## Resultado esperado

Una API consultable con curl o Postman que expone los cinco conceptos del plano de control, y un runtime de actor que ya sabe cargar y aplicar skills cuando existan — sin que haya ninguna skill definida todavía.
