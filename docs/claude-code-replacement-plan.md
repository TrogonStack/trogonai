# Trogon AI — Plan de sustitución de Claude Code al 100%

> Análisis de qué le falta a Trogon AI para reemplazar completamente a Claude Code como herramienta de programación, respetando la arquitectura NATS como backend.

---

## Lo que ya está listo (no requiere trabajo)

| Capacidad | Estado |
|---|---|
| Streaming de tokens en tiempo real | ✅ platform branch |
| Vision / imágenes | ✅ |
| Extended thinking | ✅ |
| Prompt caching | ✅ |
| Approval gates (herramientas sensibles) | ✅ platform branch |
| Token tracking por sesión | ✅ platform branch |
| `.trogon/memory.md` (equivalente CLAUDE.md) | ✅ |
| Context compaction | ✅ ventaja sobre Claude Code |
| Multi-modelo (Claude + Grok + OpenAI) | ✅ |
| Vault de credenciales cifrado | ✅ ventaja sobre Claude Code |
| Multi-agente distribuido | ✅ ventaja sobre Claude Code |

---

## Bloque 1 — Execution layer

> Fundación. Sin esto el agente no puede leer, escribir ni ejecutar en el proyecto real del developer.

| Gap | Solución NATS | Esfuerzo |
|---|---|---|
| Bash en entorno real del dev | `trogon-local-runner` — implementa el mismo protocolo NATS que `trogon-wasm-runtime` (`{prefix}.session.{id}.client.terminal.*`) pero ejecuta en el host real | 1–2 semanas |
| Filesystem real (read/write/edit/list) | Handlers en `trogon-local-runner` + tools en `dispatch_tool()` del agent | incluido arriba |
| Git CLI local (`log`, `diff`, `commit`, `stash`, `blame`...) | Handlers en `trogon-local-runner` | incluido arriba |
| Persistencia cross-session | `trogon-local-runner` opera sobre el directorio real del proyecto, no sandbox efímero | incluido arriba |
| Web search | `trogon-web-tools` — nuevo crate, sujeto NATS `{prefix}.web.search` | 2–3 días |
| Web fetch | `trogon-web-tools` — sujeto NATS `{prefix}.web.fetch` | incluido arriba |
| CLI instalable para developers | `trogon-cli` — arranca local runner + NATS + envía prompts via ACP | 1–2 semanas |

**Subtotal: ~3–4 semanas**

---

## Bloque 2 — Developer tools

> Paridad de funcionalidad completa con Claude Code.

| Gap | Solución NATS | Esfuerzo |
|---|---|---|
| MCP local (stdio) | Extender `trogon-mcp` con transporte stdio — spawnea proceso MCP local, expone sus tools al mesh NATS igual que los HTTP MCP servers | 3–4 días |
| Todo / task management | NATS KV por sesión + tools `todo_write` / `todo_read` en dispatch | 1 día |
| Jupyter notebooks | `notebook_edit` tool en `trogon-local-runner` — lee/edita `.ipynb` JSON por índice de celda | 1–2 días |
| Operación offline | NATS server embebido en `trogon-cli`, arranca automático si no hay NATS remoto configurado | 1–2 días |
| Sub-agentes como tool calls mid-turn | `spawn_agent` ToolDef → NATS request-reply a `{prefix}.agent.spawn` | 3–4 días |
| Sub-agentes especializados (Explore, Plan) | Agentes registrados en `trogon-registry` con skills dedicados — Explore solo lee, Plan solo planifica sin ejecutar | 1 semana |

**Subtotal: ~2–3 semanas**

---

## Bloque 3 — CLI UX

> Lo que hace la diferencia entre "funciona" y "un developer lo prefiere sobre Claude Code".

| Gap | Solución NATS | Esfuerzo |
|---|---|---|
| TUI rico — diffs coloreados en cada file edit | `trogon-cli` renderiza los cambios de archivo como diffs antes/después | 1–2 semanas |
| TUI rico — Ctrl+C interrumpe tool call en curso | `trogon-cli` cancela la operación NATS activa y limpia el estado | incluido arriba |
| TUI rico — costo en tokens/$ por sesión visible | `trogon-cli` consume los `UsageSummary` events que el platform branch ya emite vía NATS | incluido arriba |
| Slash commands — `/clear`, `/compact`, `/cost`, `/help` | `trogon-cli` — interfaces a capacidades que Trogon ya tiene en NATS | 1 semana |
| Slash commands — `/init` (auto-genera `.trogon/memory.md`) | `trogon-cli` — llama al LLM via ACP/NATS con el árbol del proyecto, escribe el archivo | 2–3 días |
| Slash commands — `/config` (settings del agente) | `trogon-cli` — lee/escribe config local | incluido arriba |
| Modo no-interactivo (`--print`, `--output-format json`) | `trogon-cli` flags — para scripting y CI/CD | 2–3 días |
| Permisos granulares para filesystem/bash local | Allowlist/denylist por path y por comando en `trogon-local-runner`, usando el mismo mecanismo de approval gates del platform branch | 1 semana |

**Subtotal: ~3–4 semanas**

---

## Bloque 4 — IDE integration

> Paridad total de ecosistema.

| Gap | Solución | Esfuerzo |
|---|---|---|
| Extensión VS Code | Plugin que expone el CLI dentro del editor — inline diffs, aceptar/rechazar cambios, panel de chat. Habla con `trogon-cli` o directamente con NATS/ACP | 4–6 semanas |
| Extensión JetBrains | Mismo concepto sobre la API de plugins de JetBrains | 4–6 semanas |

**Subtotal: ~2–3 meses**

---

## Resumen de esfuerzo total

| Bloques | Qué da | Esfuerzo acumulado |
|---|---|---|
| Bloque 1 | Puede escribir, leer, ejecutar, usar git — funciona para programar | 3–4 semanas |
| Bloque 1 + 2 | Paridad de funcionalidad completa con Claude Code | 5–7 semanas |
| Bloque 1 + 2 + 3 | Sustitución 100% en terminal (sin IDE) | 8–11 semanas |
| Bloque 1 + 2 + 3 + 4 | **Sustitución 100% incluyendo IDE** | **3–4 meses** |

---

## Lista de tareas por bloque

### Bloque 1 — Execution layer (~3–4 semanas)

**`trogon-local-runner` (crate nueva)**
- Implementar el mismo protocolo NATS que `trogon-wasm-runtime` (`{prefix}.session.{id}.client.terminal.*`)
- Ejecutar bash en el entorno real del developer (con su PATH, cargo, npm, python, etc.)
- Handlers de filesystem: `read_file`, `write_file`, `edit_file`, `list_dir`
- Handlers de git: `git log`, `git diff`, `git commit`, `git stash`, `git blame`, `git status`
- Operar sobre el directorio real del proyecto (no sandbox efímero)

**`trogon-web-tools` (crate nueva)**
- Sujeto NATS `{prefix}.web.search` → tool `web_search`
- Sujeto NATS `{prefix}.web.fetch` → tool `web_fetch`

**`trogon-cli` (crate nueva)**
- CLI instalable que el developer corre en su proyecto
- Arranca `trogon-local-runner` apuntando al directorio actual
- Se conecta a NATS y envía prompts via ACP
- Recibe streaming via NATS (ya existe el protocolo en platform branch)

**`trogon-agent` — agregar tools al dispatch**
- Registrar `read_file`, `write_file`, `edit_file`, `list_dir` en `dispatch_tool()`
- Registrar `web_search`, `web_fetch` en `dispatch_tool()`
- Registrar los handlers de git en `dispatch_tool()`

---

### Bloque 2 — Developer tools (~2–3 semanas)

**`trogon-mcp` — extender con transporte stdio**
- Spawnear proceso MCP local como hijo (stdin/stdout)
- Hablar MCP JSON-RPC sobre esa pipe
- Exponer los tools descubiertos al mesh NATS igual que los HTTP MCP servers

**`trogon-local-runner` — agregar `notebook_edit`**
- Leer `.ipynb` (JSON puro)
- Editar celdas por índice
- Escribir el archivo de vuelta

**`trogon-local-runner` — agregar `todo_write` / `todo_read`**
- NATS KV bucket por sesión
- Tool `todo_write`: crear/actualizar/completar tarea
- Tool `todo_read`: listar tareas activas

**`trogon-cli` — soporte offline**
- Detectar si no hay NATS remoto configurado
- Arrancar NATS server embebido automáticamente en localhost

**`trogon-agent` — `spawn_agent` como tool call**
- Crear `spawn_agent` ToolDef
- Dispatch: NATS request-reply a `{prefix}.agent.spawn`
- El registry resuelve el agente correcto

**`trogon-registry` — sub-agentes especializados**
- Registrar agente `Explore` con skill: solo lee archivos, nunca edita ni ejecuta
- Registrar agente `Plan` con skill: solo planifica, no ejecuta herramientas destructivas

---

### Bloque 3 — CLI UX (~3–4 semanas)

**`trogon-cli` — TUI**
- Mostrar diffs coloreados (antes/después) en cada operación de archivo
- Ctrl+C cancela la operación NATS activa y limpia el estado de sesión
- Mostrar costo en tokens y $ por sesión (consume `UsageSummary` events del platform branch)
- Historial navegable con teclas arriba/abajo
- Input multilinea

**`trogon-cli` — slash commands**
- `/clear` — resetea la sesión NATS actual
- `/compact` — llama a `trogon.compactor.compact` (ya existe)
- `/cost` — muestra acumulado de tokens/$ de la sesión
- `/help` — lista comandos disponibles
- `/config` — lee/escribe config local del CLI
- `/init` — llama al LLM via ACP con el árbol del proyecto, genera `.trogon/memory.md`

**`trogon-cli` — modo no-interactivo**
- Flag `--print "prompt"` para uso desde scripts
- Flag `--output-format json` para consumo en CI/CD

**`trogon-local-runner` — permisos granulares**
- Allowlist/denylist por path (ej. `allow: src/**, deny: .env`)
- Allowlist/denylist por comando bash (ej. `allow: cargo test, deny: rm -rf`)
- Mismo mecanismo de approval gates del platform branch (NATS request-reply)

---

### Bloque 4 — IDE integration (~2–3 meses)

**`trogon-vscode` (extensión nueva)**
- Panel de chat dentro de VS Code
- Inline diffs con aceptar/rechazar cambios
- Slash commands desde el editor
- Comunica con `trogon-cli` o directo a NATS/ACP

**`trogon-jetbrains` (plugin nuevo)**
- Mismo concepto sobre la API de plugins de JetBrains

---

## División del trabajo entre dos developers

### Dev A — Services track (backend NATS)

**Wave 1 — lo más crítico**
- `trogon-local-runner`: bash real + filesystem + git + persistencia

**Wave 2**
- `trogon-web-tools`: `web_search` + `web_fetch`
- `trogon-mcp`: extender con transporte stdio

**Wave 3**
- `trogon-local-runner`: permisos granulares (allowlist/denylist path y comando)
- `trogon-local-runner`: `notebook_edit` + `todo_write` / `todo_read`
- `trogon-registry`: registrar sub-agentes Explore y Plan con sus skills

**Wave 4**
- `trogon-vscode`: extensión VS Code

---

### Dev B — CLI track (superficie del developer)

**Wave 1 — lo más crítico**
- `trogon-cli`: conexión NATS, envío de prompts via ACP, recepción de streaming

**Wave 2**
- `trogon-agent`: registrar en `dispatch_tool()` los tools del local-runner
- `trogon-agent`: `spawn_agent` ToolDef
- `trogon-cli`: soporte offline (NATS embebido)

**Wave 3**
- `trogon-cli`: TUI — diffs coloreados, Ctrl+C, costo por sesión, historial, input multilinea
- `trogon-cli`: slash commands (`/clear`, `/compact`, `/cost`, `/help`, `/config`, `/init`)

**Wave 4**
- `trogon-cli`: modo no-interactivo (`--print`, `--output-format json`)
- `trogon-jetbrains`: extensión JetBrains

---

### Esfuerzo por developer

| | Dev A | Dev B |
|---|---|---|
| Wave 1 | 1–2 semanas | 1–2 semanas |
| Wave 2 | 1 semana | 1 semana |
| Wave 3 | 1–2 semanas | 2–3 semanas |
| Wave 4 | 4–6 semanas | 2–3 semanas |
| **Total** | **7–11 semanas** | **6–9 semanas** |

---

### Cómo funciona el flujo

Las waves son orden de prioridad, no sincronización de tiempo. Cada developer avanza a su ritmo — cuando termina su Wave 1 arranca la Wave 2 sin esperar al otro.

La única dependencia dura entre los dos es al inicio del día 1: Dev A define los sujetos NATS de `trogon-local-runner` (el contrato de la API):

```
{prefix}.local.fs.read
{prefix}.local.fs.write
{prefix}.local.fs.edit
{prefix}.local.git.diff
...
```

Esto toma medio día. Dev B necesita esos nombres para wiring en `dispatch_tool()` (Wave 2). Es el único momento donde uno bloquea al otro.

Después de ese punto los dos avanzan completamente independientes.

---

### Organización de ramas

Una rama base compartida `feat/dev-tools` desde `platform`. Cada developer trabaja en ramas cortas por crate o feature y hace PR a esa base. Cuando todo esté listo, un solo PR de `feat/dev-tools` → `platform`.

```
platform
└── feat/dev-tools          ← rama base compartida
    ├── feat/local-runner   ← Dev A
    ├── feat/web-tools      ← Dev A
    ├── feat/cli-core       ← Dev B
    ├── feat/cli-tui        ← Dev B
    └── ...
```

El único archivo donde los dos pueden tener conflictos es `trogon-agent/src/tools/mod.rs` — los dos agregan tools al mismo `dispatch_tool()`. Se resuelve coordinando ese PR puntualmente cuando coincidan en el tiempo. No es un problema estructural.

---

## Por qué todo esto respeta la arquitectura NATS

Ningún item rompe la arquitectura. Cada uno sigue uno de estos tres patrones:

| Patrón | Ejemplos |
|---|---|
| Nuevo servicio NATS que implementa un protocolo existente | `trogon-local-runner`, `trogon-web-tools` |
| Extensión de un servicio NATS existente | `trogon-mcp` + stdio, permission system en local runner |
| Cliente NATS nuevo (no toca el backend) | `trogon-cli`, extensiones IDE |

El punto clave: `wasm_bash_tool.rs` ya demostró el patrón — el bash tool manda mensajes NATS a `{prefix}.session.{id}.client.terminal.*` sin saber qué hay detrás. `trogon-local-runner` implementa el mismo contrato pero en el entorno real. El ACP runner no cambia una línea.
