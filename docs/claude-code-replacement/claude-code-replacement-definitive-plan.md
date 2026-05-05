# Trogon AI — Plan de implementación definitivo
# Sustitución completa de Claude Code para programación

> Respeta la arquitectura NATS como backend sin excepción.
> Fuentes: `claude-code-replacement-plan.md` + `plan.txt`

---

## Lo que ya está listo (no tocar)

| Capacidad | Estado |
|---|---|
| Streaming de tokens en tiempo real | ✅ platform branch |
| Vision / imágenes | ✅ |
| Extended thinking | ✅ |
| Prompt caching | ✅ |
| Approval gates (herramientas sensibles) | ✅ platform branch |
| Token tracking por sesión | ✅ platform branch |
| `.trogon/memory.md` base | ✅ (se mejora en PR 4) |
| Context compaction | ✅ (se mejora en PR 4) |
| Multi-modelo (Claude + Grok + OpenAI) | ✅ |
| Vault de credenciales cifrado | ✅ |
| Multi-agente distribuido | ✅ |

---

## Bloques y tiempos

| Bloques | Qué da | Esfuerzo acumulado |
|---|---|---|
| Bloque 1 | Puede leer, escribir, ejecutar, usar git — funciona para programar | 3–4 semanas |
| Bloque 1 + 2 | Paridad de funcionalidad completa con Claude Code | 5–7 semanas |
| Bloque 1 + 2 + 3 | Sustitución 100% en terminal (sin IDE) | 8–11 semanas |
| Bloque 1 + 2 + 3 + 4 | **Sustitución 100% incluyendo IDE** | **3–4 meses** |

---

## Bloque 1 — Execution layer (~3–4 semanas)

### PR 1 — Core tools en `trogon-agent-core`

**`crates/trogon-agent-core/Cargo.toml` — dependencias nuevas**
- `globset = "0.4"`
- `ignore = "0.4"`
- `walkdir = "2"`
- `html2text = "0.12"`

**`crates/trogon-agent-core/src/tools/mod.rs` — ampliar `ToolContext` y `dispatch_tool()`**
- Agregar campo `cwd: String` a `ToolContext` (directorio de trabajo de la sesión)
- Completar `dispatch_tool()` con routing real:
  - `"read_file"` → `fs::read_file`
  - `"write_file"` → `fs::write_file`
  - `"list_dir"` → `fs::list_dir`
  - `"glob"` → `fs::glob_files`
  - `"str_replace"` → `editor::str_replace`
  - `"git_status"` → `git::status`
  - `"git_diff"` → `git::diff`
  - `"git_log"` → `git::log`
  - `"fetch_url"` → `web::fetch_url`

**`crates/trogon-agent-core/src/tools/fs.rs` — archivo nuevo**

`read_file`
- Parámetros: `path: String`, `offset?: u32`, `limit?: u32`
- Resuelve ruta relativa a `ctx.cwd`
- Rechaza rutas fuera de `cwd` (protección contra path traversal)
- Devuelve contenido con números de línea estilo `cat -n`
- Si `offset`/`limit` presentes, devuelve solo ese rango de líneas
- Si el archivo no existe, devuelve error legible

`write_file`
- Parámetros: `path: String`, `content: String`
- Crea directorios intermedios con `tokio::fs::create_dir_all`
- Escritura atómica: primero escribe a `.tmp`, luego `rename` (previene corrupción si el proceso muere a mitad)
- Devuelve `"OK"` o mensaje de error

`list_dir`
- Parámetros: `path?: String` (default `"."`)
- Usa el crate `ignore` para respetar `.gitignore` automáticamente
- Devuelve árbol de directorios en texto estilo `tree`
- Limita a 500 entradas para no saturar el contexto del agente

`glob`
- Parámetros: `pattern: String`, `path?: String`
- Usa `globset` para matching de patrones (`src/**/*.rs`, `**/*.test.ts`)
- Filtra entradas ignoradas por `.gitignore` via `ignore::Walk`
- Devuelve lista de rutas relativas a `cwd`

`notebook_edit`
- Parámetros: `path: String`, `cell_index: u32`, `content: String`, `cell_type?: String`
- Lee `.ipynb` (JSON puro)
- Edita la celda por índice
- Escribe el archivo de vuelta

**`crates/trogon-agent-core/src/tools/editor.rs` — archivo nuevo**

`str_replace`
- Parámetros: `path: String`, `old_str: String`, `new_str: String`
- Lee el archivo completo
- Verifica que `old_str` aparezca **exactamente una vez** — si hay 0 o >1 ocurrencias, devuelve error descriptivo con el conteo exacto
- Reemplaza y escribe el resultado
- Devuelve diff de las líneas cambiadas con contexto ±3 líneas

**`crates/trogon-agent-core/src/tools/git.rs` — archivo nuevo**

`git_status`, `git_diff`, `git_log`
- Ejecutan `git status --short`, `git diff`, `git log --oneline -20` respectivamente
- Usan `tokio::process::Command` con `current_dir(&ctx.cwd)`
- Output limitado a ~4KB; si excede, truncan con aviso al final
- Sin nuevas dependencias (solo tokio)

**`crates/trogon-agent-core/src/tools/web.rs` — archivo nuevo**

`fetch_url`
- Parámetros: `url: String`, `raw?: bool`
- Usa `ctx.http_client` (reutiliza el cliente existente, no crea uno por llamada)
- Si `raw: false` (default): convierte HTML a texto plano con `html2text`
- Trunca a 8KB con aviso si la respuesta es más grande

---

### PR 2 — Bash con estado real

**`crates/trogon-acp-runner/src/wasm_bash_tool.rs`**
- Agregar campo `sandbox_dir: PathBuf` al struct `WasmRuntimeBashTool`
- Pasarlo como `cwd` en `CreateTerminalRequest` para que bash ejecute en el directorio real del proyecto
- `sandbox_dir` se inicializa desde `session.cwd` en `new_session`

**`crates/trogon-acp-runner/src/session_store.rs`**
- Agregar `terminal_id: Option<String>` a `SessionState` (almacenado en NATS KV)
- En la primera llamada a bash: crear terminal, guardar `terminal_id` en `SessionState`
- En llamadas posteriores: reusar el mismo terminal con `WriteToTerminalRequest`
- Sin esto, cada llamada bash arranca un proceso nuevo y el estado (`cd`, variables de entorno) no persiste entre llamadas
- Protocolo de demarcación de fin de comando: `<comando>; echo "__EXIT_$?__"`
- Leer stream hasta encontrar `__EXIT_N__`, extraer código de salida
- Timeout configurable, default 30s; al expirar, devuelve error con output parcial

---

### PR 3 — `trogon-cli` core

**`crates/trogon-cli/Cargo.toml` — crate nueva**
- `rustyline = "14"`
- `acp-nats`, `clap`, `axum` — ya en el workspace

**`crates/trogon-cli/src/main.rs`**
- Parsing de argumentos con clap
- Lee `TROGON_NATS_URL` (default `nats://localhost:4222`)
- NATS autostart: si la conexión falla, lanza `nats-server -p 4222` como proceso hijo
  - Reintenta hasta 3s con intervalos de 200ms
  - Si `nats-server` no está en PATH: imprime instrucciones de instalación y sale con error claro
  - Al salir: mata el proceso hijo solo si fue este proceso quien lo lanzó

**`crates/trogon-cli/src/session.rs`**
- Gestión de sesión ACP via NATS
- Crea sesión con `cwd = std::env::current_dir()`
- Cierra sesión limpiamente al salir

**`crates/trogon-cli/src/repl.rs`**
- Loop REPL con rustyline
- Ctrl+C envía cancel a la sesión activa y limpia el estado
- Ctrl+D sale limpiamente cerrando la sesión
- Historial persistido en `~/.local/share/trogon/history`
- Recibe stream de eventos ACP e imprime `TextDelta` en tiempo real

**`crates/trogon-cli/src/print.rs`**
- Modo no interactivo básico (se completa en PR 10)

**`crates/trogon-agent/src/tools/mod.rs` — wiring de los tools nuevos**
- Registrar en `dispatch_tool()` todos los tools del PR 1: `read_file`, `write_file`, `list_dir`, `glob`, `str_replace`, `git_status`, `git_diff`, `git_log`, `fetch_url`, `notebook_edit`
- Coordinar con Dev A el día 1 para alinear nombres de tools y contrato `ToolContext.cwd`

---

## Bloque 2 — Developer tools (~2–3 semanas)

### PR 4 — Contexto y memoria

**`crates/trogon-acp-runner/src/trogon_md.rs` — archivo nuevo**

TROGON.md jerárquico:
- Buscar `TROGON.md` desde `cwd` hacia la raíz del filesystem (mismo patrón que Claude Code con CLAUDE.md)
- También cargar `~/.config/trogon/TROGON.md` como configuración global del usuario
- Concatenar en orden: global → raíz del repo → más específico al directorio actual
- Inyectar en `system_prompt` al crear o cargar una sesión

**`crates/trogon-acp-runner/src/agent.rs` — auto-compact con threshold**

Cambiar de compact incondicional a threshold del 85%:
```rust
let token_estimate = estimate_token_count(&session.messages); // heurística len_bytes / 4
if token_estimate > TOKEN_BUDGET * 85 / 100 {
    compact_messages(&mut session).await?;
}
// TOKEN_BUDGET configurable en SessionState, default 200_000
```

---

### PR 5 — Extensibilidad del agente

**`crates/trogon-cli/src/repl.rs` — `@mentions` de archivos**
- Antes de enviar el prompt, escanear en busca de tokens `@<path>`
- Para cada match: resolver el path relativo a `cwd`, leer el contenido, sustituir `@<path>` por bloque de código con el contenido del archivo
- Si el path no existe, dejar el token sin modificar y advertir al usuario
- Tab-completion del path en rustyline via un `Helper` personalizado

**`crates/trogon-agent-core/src/agent_loop.rs` — herramientas paralelas**

Reemplazar ejecución secuencial por paralela con `join_all`:
```rust
// antes (secuencial)
for call in &tool_calls {
    let result = dispatch_tool(&ctx, &call.name, &call.input).await;
}

// después (paralelo)
let futures: Vec<_> = tool_calls.iter()
    .map(|call| dispatch_tool(&ctx, &call.name, &call.input))
    .collect();
let results = futures::future::join_all(futures).await;
```
Restricción: si hay un `PermissionChecker` interactivo activo, serializar las llamadas para no preguntar dos cosas simultáneamente al usuario.

---

### PR 6 — MCP stdio como proxy HTTP del CLI

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — archivo nuevo**

El backend NATS no cambia ninguna línea — solo ve servidores MCP HTTP normales:
- El CLI lanza el proceso MCP stdio como hijo (ej. `npx @modelcontextprotocol/server-filesystem ./`)
- Levanta un servidor axum en un puerto local aleatorio que hace proxy JSON-RPC ↔ stdin/stdout del proceso hijo
- Registra el servidor en la sesión como `StoredMcpServer::Http { url: "http://127.0.0.1:<port>" }`
- Al terminar el CLI: mata el proceso hijo y libera el puerto

---

### PR 7 — Tools adicionales y agentes especializados

**`crates/trogon-agent-core/src/tools/mod.rs` — todo management**
- Tool `todo_write`: parámetros `id: String`, `content: String`, `status: String` (pending / in_progress / completed) — almacena en NATS KV bucket por sesión
- Tool `todo_read`: devuelve lista de tareas activas de la sesión

**`crates/trogon-agent/src/tools/mod.rs` — `spawn_agent`**
- Crear `spawn_agent` ToolDef
- Dispatch: NATS request-reply a `{prefix}.agent.spawn`
- El registry resuelve el agente correcto y devuelve el resultado al turn actual del modelo

**`trogon-registry` — sub-agentes especializados**
- Registrar agente `Explore` con skill: solo lee archivos y responde preguntas, nunca edita ni ejecuta
- Registrar agente `Plan` con skill: solo planifica, no ejecuta herramientas destructivas

---

## Bloque 3 — CLI UX (~3–4 semanas)

### PR 8 — TUI

**`crates/trogon-cli/src/repl.rs`**
- Mostrar diffs coloreados (antes/después) en cada operación `str_replace` y `write_file`
- Ctrl+C cancela la operación NATS activa y limpia el estado de sesión
- Mostrar costo en tokens y $ por sesión consumiendo los `UsageSummary` events que el platform branch ya emite
- Historial navegable con teclas arriba/abajo (rustyline ya lo maneja con el historial de PR 3)
- Input multilinea

---

### PR 9 — Slash commands completos

**`crates/trogon-cli/src/repl.rs`**

Los comandos se ejecutan localmente, no se envían al agente:

| Comando | Acción |
|---|---|
| `/clear` | Limpia historial de mensajes de la sesión via NATS KV |
| `/compact` | Fuerza compactación del contexto ahora (`trogon.compactor.compact`) |
| `/cost` | Muestra acumulado de tokens y $ de la sesión actual |
| `/help` | Lista todos los comandos disponibles |
| `/config` | Lee/escribe config local del CLI |
| `/model <id>` | Cambia el modelo usado en la sesión sin reiniciarla |
| `/init` | Analiza el proyecto con LLM via ACP y genera `TROGON.md` en el directorio actual |

---

### PR 10 — Modo no-interactivo y permisos granulares

**`crates/trogon-cli/src/print.rs` — modo no-interactivo**
- Activado con `trogon --print "haz X"` o `trogon -p "haz X"`
- Lee prompt desde argumento o desde stdin: `trogon --print "explica" < error.log`
- Imprime solo `TextDelta` a stdout (sin colores ni UI interactiva)
- Exit code 0 si completa sin error, 1 si el agente devuelve error
- Útil para pipes y CI/CD

**`crates/trogon-acp-runner` — permisos granulares**
- Allowlist/denylist por path (ej. `allow: src/**, deny: .env`)
- Allowlist/denylist por comando bash (ej. `allow: cargo test, deny: rm -rf`)
- Mismo mecanismo de approval gates del platform branch (NATS request-reply)
- Configurable en `TROGON.md` y via `/config`

---

## Bloque 4 — IDE integration (~2–3 meses)

### PR 11 — Extensión VS Code (`trogon-vscode`)

- Panel de chat dentro del editor
- Inline diffs con aceptar/rechazar cambios por archivo
- Slash commands desde el editor
- Comunica con `trogon-cli` o directamente via NATS/ACP

**Esfuerzo: 4–6 semanas**

### PR 12 — Plugin JetBrains (`trogon-jetbrains`)

- Mismo concepto sobre la API de plugins de JetBrains (IntelliJ, GoLand, RustRover, etc.)

**Esfuerzo: 4–6 semanas**

---

## Lista completa de tareas

### Bloque 1 — Execution layer

**PR 1 — `trogon-agent-core/src/tools/`**
- `mod.rs`: agregar `cwd: String` a `ToolContext`
- `mod.rs`: completar `dispatch_tool()` con routing para 9 tools nuevos
- `fs.rs` (nuevo): `read_file` con offset/limit, path traversal protection, cat-n
- `fs.rs`: `write_file` atómico (.tmp → rename) con create_dir_all
- `fs.rs`: `list_dir` respetando .gitignore, árbol, límite 500 entradas
- `fs.rs`: `glob` con globset + ignore::Walk
- `fs.rs`: `notebook_edit` para archivos .ipynb
- `editor.rs` (nuevo): `str_replace` con exactly-once validation + diff ±3 líneas
- `git.rs` (nuevo): `git_status`, `git_diff`, `git_log` con límite 4KB
- `web.rs` (nuevo): `fetch_url` con html2text y límite 8KB
- `Cargo.toml`: agregar globset, ignore, walkdir, html2text

**PR 2 — Bash stateful**
- `wasm_bash_tool.rs`: agregar `sandbox_dir: PathBuf`, pasar `cwd` a `CreateTerminalRequest`
- `session_store.rs`: agregar `terminal_id: Option<String>` a `SessionState`
- `wasm_bash_tool.rs`: reusar terminal con `WriteToTerminalRequest` en llamadas posteriores
- `wasm_bash_tool.rs`: protocolo demarcación `echo "__EXIT_$?__"` + extracción exit code
- `wasm_bash_tool.rs`: timeout configurable default 30s

**PR 3 — `trogon-cli` core**
- `Cargo.toml` (nuevo): rustyline, acp-nats, clap, axum
- `main.rs`: clap + TROGON_NATS_URL + NATS autostart con retry 3s/200ms
- `main.rs`: imprimir instrucciones si nats-server no está en PATH
- `main.rs`: matar proceso hijo solo si fue lanzado por este proceso
- `session.rs`: crear sesión ACP con cwd = current_dir(), cerrar limpiamente
- `repl.rs`: loop rustyline + imprimir TextDelta en tiempo real
- `repl.rs`: Ctrl+C cancela sesión, Ctrl+D sale limpiamente
- `repl.rs`: historial en ~/.local/share/trogon/history
- `trogon-agent/src/tools/mod.rs`: registrar todos los tools de PR 1 en dispatch_tool()

---

### Bloque 2 — Developer tools

**PR 4 — Contexto y memoria**
- `trogon_md.rs` (nuevo): buscar TROGON.md desde cwd hacia raíz del filesystem
- `trogon_md.rs`: cargar ~/.config/trogon/TROGON.md como config global del usuario
- `trogon_md.rs`: concatenar en orden global → raíz → directorio actual
- `trogon_md.rs`: inyectar en system_prompt al crear/cargar sesión
- `agent.rs`: cambiar compact incondicional por threshold del 85% con heurística len_bytes/4
- `agent.rs`: TOKEN_BUDGET configurable en SessionState, default 200_000

**PR 5 — Extensibilidad**
- `repl.rs`: scan de `@<path>` en el prompt antes de enviar al agente
- `repl.rs`: resolver path relativo a cwd, leer archivo, inyectar contenido
- `repl.rs`: advertir si el path no existe, no fallar
- `repl.rs`: tab-completion de paths para @mentions via rustyline Helper
- `agent_loop.rs`: reemplazar for secuencial por join_all paralelo
- `agent_loop.rs`: serializar si PermissionChecker interactivo está activo

**PR 6 — MCP stdio**
- `stdio_mcp_bridge.rs` (nuevo): spawnear proceso MCP stdio como hijo
- `stdio_mcp_bridge.rs`: levantar servidor axum en puerto local aleatorio
- `stdio_mcp_bridge.rs`: proxy JSON-RPC ↔ stdin/stdout del proceso hijo
- `stdio_mcp_bridge.rs`: registrar como StoredMcpServer::Http en la sesión
- `stdio_mcp_bridge.rs`: matar proceso hijo y liberar puerto al salir del CLI

**PR 7 — Tools adicionales y agentes**
- `mod.rs`: tool `todo_write` (NATS KV bucket por sesión, estados: pending/in_progress/completed)
- `mod.rs`: tool `todo_read` (lista tareas activas de la sesión)
- `mod.rs`: `spawn_agent` ToolDef → NATS request-reply a {prefix}.agent.spawn
- `trogon-registry`: registrar agente Explore (solo lee, nunca edita ni ejecuta)
- `trogon-registry`: registrar agente Plan (solo planifica, no ejecuta herramientas destructivas)

---

### Bloque 3 — CLI UX

**PR 8 — TUI**
- `repl.rs`: renderizar diffs coloreados antes/después en str_replace y write_file
- `repl.rs`: Ctrl+C cancela operación NATS activa y limpia estado
- `repl.rs`: consumir UsageSummary events → mostrar tokens y $ acumulados por sesión
- `repl.rs`: input multilinea

**PR 9 — Slash commands**
- `repl.rs`: `/clear` — limpia historial via NATS KV
- `repl.rs`: `/compact` — fuerza compactación ahora
- `repl.rs`: `/cost` — muestra acumulado tokens/$ de la sesión
- `repl.rs`: `/help` — lista comandos disponibles
- `repl.rs`: `/config` — lee/escribe config local
- `repl.rs`: `/model <id>` — cambia modelo sin reiniciar sesión
- `repl.rs`: `/init` — analiza proyecto con LLM via ACP, genera TROGON.md

**PR 10 — No-interactivo y permisos**
- `print.rs`: flag `--print`/`-p`, leer desde argumento
- `print.rs`: leer prompt desde stdin si no se pasa texto (`trogon --print < error.log`)
- `print.rs`: imprimir solo TextDelta a stdout, sin colores
- `print.rs`: exit code 0 en éxito, 1 en error del agente
- `trogon-acp-runner`: allowlist/denylist por path
- `trogon-acp-runner`: allowlist/denylist por comando bash
- `trogon-acp-runner`: configurable en TROGON.md y via /config

---

### Bloque 4 — IDE integration

**PR 11 — VS Code (`trogon-vscode`)**
- Panel de chat dentro del editor
- Inline diffs con aceptar/rechazar cambios por archivo
- Slash commands desde el editor
- Comunicación con trogon-cli o directamente via NATS/ACP

**PR 12 — JetBrains (`trogon-jetbrains`)**
- Mismo concepto sobre la API de plugins de JetBrains

---

## División del trabajo

### Dev A — Services track

**Wave 1 (~1–2 semanas)**
- PR 1 completo: todos los tools en `trogon-agent-core/src/tools/` (fs, editor, git, web)
- PR 2 completo: bash stateful con terminal_id en SessionState

**Wave 2 (~1 semana)**
- PR 4 completo: TROGON.md jerárquico + auto-compact 85%
- PR 6 completo: MCP stdio proxy HTTP en `stdio_mcp_bridge.rs`

**Wave 3 (~1–2 semanas)**
- PR 7 completo: todo_write/read, spawn_agent, sub-agentes Explore y Plan
- PR 10 (permisos): allowlist/denylist por path y comando en `trogon-acp-runner`

**Wave 4 (~4–6 semanas)**
- PR 11: extensión VS Code

---

### Dev B — CLI track

**Wave 1 (~1–2 semanas)**
- PR 3 completo: trogon-cli core — REPL, sesión ACP, streaming, NATS autostart, Ctrl+C/D, historial
- Wiring en `trogon-agent/src/tools/mod.rs`: registrar en dispatch_tool() los tools de PR 1 (coordinar con Dev A el día 1 para alinear nombres y contrato ToolContext.cwd)

**Wave 2 (~1 semana)**
- PR 5 completo: @mentions con tab-completion + herramientas paralelas join_all

**Wave 3 (~2–3 semanas)**
- PR 8 completo: TUI — diffs coloreados, Ctrl+C, costo por sesión, multilinea
- PR 9 completo: slash commands /clear, /compact, /cost, /help, /config, /model, /init

**Wave 4 (~2–3 semanas)**
- PR 10 (no-interactivo): --print, stdin, exit codes
- PR 12: plugin JetBrains

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

### Flujo y dependencias

Las waves son orden de prioridad, no sincronización de tiempo. Cada developer avanza a su ritmo — cuando termina su wave arranca la siguiente sin esperar al otro.

**Única dependencia dura — día 1:**

Dev A comunica a Dev B dos cosas **antes de escribir una sola línea de implementación**:

1. Los nombres exactos de los tools tal como quedarán en `dispatch_tool()`:
```
"read_file", "write_file", "list_dir", "glob", "str_replace",
"git_status", "git_diff", "git_log", "fetch_url", "notebook_edit"
```

2. El cambio en `ToolContext` — Dev A agrega el campo `cwd`:
```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // ← campo nuevo
    pub http_client: reqwest::Client,
}
```

Con esa información, Dev B agrega los match arms en `crates/trogon-agent/src/tools/mod.rs` y los imports `use trogon_agent_core::tools::{fs, editor, git, web}`. Dev B puede hacer ese PR el mismo día 1 aunque Dev A no haya terminado las implementaciones — mientras los módulos existan (aunque vacíos), compila.

**Riesgo si no se coordina:** si Dev B usa un nombre distinto al que Dev A implementó (ej. `"readFile"` en vez de `"read_file"`), el agente devuelve `"Unknown tool: readFile"` en runtime sin error de compilación — falla silenciosamente.

**Único archivo con riesgo de conflicto:** `trogon-agent-core/src/tools/mod.rs` — los dos pueden agregar entries a `dispatch_tool()`. Coordinarse puntualmente si coinciden en ese archivo al mismo tiempo.

---

### Organización de ramas

```
platform
└── feat/dev-tools                ← rama base compartida
    ├── feat/core-tools           ← Dev A — PR 1
    ├── feat/bash-stateful        ← Dev A — PR 2
    ├── feat/trogon-md            ← Dev A — PR 4
    ├── feat/mcp-stdio-bridge     ← Dev A — PR 6
    ├── feat/extra-tools          ← Dev A — PR 7
    ├── feat/permissions          ← Dev A — PR 10 (permisos)
    ├── feat/vscode               ← Dev A — PR 11
    ├── feat/cli-core             ← Dev B — PR 3
    ├── feat/mentions-parallel    ← Dev B — PR 5
    ├── feat/cli-tui              ← Dev B — PR 8
    ├── feat/slash-commands       ← Dev B — PR 9
    ├── feat/cli-noninteractive   ← Dev B — PR 10 (no-interactivo)
    └── feat/jetbrains            ← Dev B — PR 12
```

PRs cortos por feature → `feat/dev-tools`. Un solo PR final `feat/dev-tools` → `platform` cuando todo esté listo.

---

## Por qué todo respeta la arquitectura NATS

| Patrón | PRs |
|---|---|
| Extensión de crates existentes sin nuevo servicio NATS | PR 1, PR 2, PR 4, PR 5, PR 7 |
| Nuevo cliente NATS (no toca el backend) | PR 3, PR 8, PR 9, PR 10, PR 11, PR 12 |
| Bridge que oculta complejidad al backend | PR 6 — el runner solo ve un servidor MCP HTTP normal |

Los tools de filesystem y git viven en `trogon-agent-core` y acceden al filesystem directamente via `ctx.cwd`. La comunicación NATS ocurre a nivel de sesión y agente, no a nivel de cada lectura de archivo — más eficiente y más simple.
