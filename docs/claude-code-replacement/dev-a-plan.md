# Dev A — Plan de trabajo
# Services track (backend NATS)

> Todo el trabajo es aditivo. NATS permanece como backend sin cambios.
> Coordinación con Dev B: día 1, definir nombres de tools y contrato ToolContext.cwd (medio día).

---

## Resumen de waves

| Wave | PRs | Esfuerzo |
|---|---|---|
| Wave 1 | PR 1 + PR 2 | 1–2 semanas |
| Wave 2 | PR 4 + PR 6 | 1 semana |
| Wave 3 | PR 7 + PR 10 (permisos) | 1–2 semanas |
| Wave 4 | PR 11 | 4–6 semanas |
| **Total** | | **7–11 semanas** |

---

## Wave 1

### PR 1 — Core tools en `trogon-agent-core`

**`crates/trogon-agent-core/Cargo.toml` — dependencias nuevas**
- `globset = "0.4"`
- `ignore = "0.4"`
- `walkdir = "2"`
- `html2text = "0.12"`

**`crates/trogon-agent-core/src/tools/mod.rs`**
- Agregar campo `cwd: String` a `ToolContext`
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
  - `"notebook_edit"` → `fs::notebook_edit`

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
- Escritura atómica: primero escribe a `.tmp`, luego `rename`
- Devuelve `"OK"` o mensaje de error

`list_dir`
- Parámetros: `path?: String` (default `"."`)
- Usa el crate `ignore` para respetar `.gitignore` automáticamente
- Devuelve árbol de directorios en texto estilo `tree`
- Limita a 500 entradas

`glob`
- Parámetros: `pattern: String`, `path?: String`
- Usa `globset` para matching (`src/**/*.rs`, `**/*.test.ts`)
- Filtra entradas ignoradas por `.gitignore` via `ignore::Walk`
- Devuelve lista de rutas relativas a `cwd`

`notebook_edit`
- Parámetros: `path: String`, `cell_index: u32`, `content: String`, `cell_type?: String`
- Lee `.ipynb` (JSON puro), edita la celda por índice, escribe el archivo de vuelta

**`crates/trogon-agent-core/src/tools/editor.rs` — archivo nuevo**

`str_replace`
- Parámetros: `path: String`, `old_str: String`, `new_str: String`
- Lee el archivo completo
- Verifica que `old_str` aparezca **exactamente una vez** — si hay 0 o >1 ocurrencias, devuelve error con el conteo exacto
- Reemplaza y escribe el resultado
- Devuelve diff de las líneas cambiadas con contexto ±3 líneas

**`crates/trogon-agent-core/src/tools/git.rs` — archivo nuevo**

`git_status`, `git_diff`, `git_log`
- Ejecutan `git status --short`, `git diff`, `git log --oneline -20`
- Usan `tokio::process::Command` con `current_dir(&ctx.cwd)`
- Output limitado a ~4KB; si excede, truncan con aviso al final

**`crates/trogon-agent-core/src/tools/web.rs` — archivo nuevo**

`fetch_url`
- Parámetros: `url: String`, `raw?: bool`
- Usa `ctx.http_client` (reutiliza el cliente existente)
- Si `raw: false` (default): convierte HTML a texto plano con `html2text`
- Trunca a 8KB con aviso si la respuesta es más grande

**Rama:** `feat/core-tools`

---

### PR 2 — Bash con estado real

**`crates/trogon-acp-runner/src/wasm_bash_tool.rs`**
- Agregar campo `sandbox_dir: PathBuf` al struct `WasmRuntimeBashTool`
- Pasar `sandbox_dir` como `cwd` en `CreateTerminalRequest`
- `sandbox_dir` se inicializa desde `session.cwd` en `new_session`

**`crates/trogon-acp-runner/src/session_store.rs`**
- Agregar `terminal_id: Option<String>` a `SessionState` (almacenado en NATS KV)
- Primera llamada a bash: crear terminal, guardar `terminal_id` en `SessionState`
- Llamadas posteriores: reusar el mismo terminal con `WriteToTerminalRequest`
- Protocolo de demarcación: `<comando>; echo "__EXIT_$?__"` — leer stream hasta encontrar `__EXIT_N__`, extraer exit code
- Timeout configurable, default 30s; al expirar, devuelve error con output parcial

**Rama:** `feat/bash-stateful`

---

## Wave 2

### PR 4 — Contexto y memoria

**`crates/trogon-acp-runner/src/trogon_md.rs` — archivo nuevo**
- Buscar `TROGON.md` desde `cwd` hacia la raíz del filesystem
- Cargar `~/.config/trogon/TROGON.md` como configuración global del usuario
- Concatenar en orden: global → raíz del repo → directorio actual
- Inyectar en `system_prompt` al crear o cargar una sesión

**`crates/trogon-acp-runner/src/agent.rs`**
- Cambiar compact incondicional por threshold del 85%:
```rust
let token_estimate = estimate_token_count(&session.messages); // heurística len_bytes / 4
if token_estimate > TOKEN_BUDGET * 85 / 100 {
    compact_messages(&mut session).await?;
}
// TOKEN_BUDGET configurable en SessionState, default 200_000
```

**Rama:** `feat/trogon-md`

---

### PR 6 — MCP stdio como proxy HTTP del CLI

**`crates/trogon-cli/src/stdio_mcp_bridge.rs` — archivo nuevo**
- Lanzar proceso MCP stdio como hijo (ej. `npx @modelcontextprotocol/server-filesystem ./`)
- Levantar servidor axum en puerto local aleatorio
- Proxy JSON-RPC ↔ stdin/stdout del proceso hijo
- Registrar en la sesión como `StoredMcpServer::Http { url: "http://127.0.0.1:<port>" }`
- Al terminar el CLI: matar proceso hijo y liberar puerto
- El backend NATS no cambia ninguna línea

**Rama:** `feat/mcp-stdio-bridge`

---

## Wave 3

### PR 7 — Tools adicionales y agentes especializados

**`crates/trogon-agent-core/src/tools/mod.rs`**

`todo_write`
- Parámetros: `id: String`, `content: String`, `status: String` (pending / in_progress / completed)
- Almacena en NATS KV bucket por sesión

`todo_read`
- Devuelve lista de tareas activas de la sesión actual

**`crates/trogon-agent/src/tools/mod.rs`**

`spawn_agent`
- Crear `spawn_agent` ToolDef
- Dispatch: NATS request-reply a `{prefix}.agent.spawn`
- El registry resuelve el agente correcto y devuelve el resultado al turn actual del modelo

**`trogon-registry`**
- Registrar agente `Explore` con skill: solo lee archivos y responde preguntas, nunca edita ni ejecuta
- Registrar agente `Plan` con skill: solo planifica, no ejecuta herramientas destructivas

**Rama:** `feat/extra-tools`

---

### PR 10 — Permisos granulares

**`crates/trogon-acp-runner`**
- Allowlist/denylist por path (ej. `allow: src/**, deny: .env`)
- Allowlist/denylist por comando bash (ej. `allow: cargo test, deny: rm -rf`)
- Mismo mecanismo de approval gates del platform branch (NATS request-reply)
- Configurable en `TROGON.md` y via `/config`

**Rama:** `feat/permissions`

---

## Wave 4

### PR 11 — Extensión VS Code (`trogon-vscode`)

- Panel de chat dentro del editor
- Inline diffs con aceptar/rechazar cambios por archivo
- Slash commands desde el editor
- Comunica con `trogon-cli` o directamente via NATS/ACP

**Rama:** `feat/vscode`

---

## Ramas de trabajo

```
feat/claude-code-replacement    ← rama base compartida con Dev B
  feat/core-tools               ← PR 1
  feat/bash-stateful            ← PR 2
  feat/trogon-md                ← PR 4
  feat/mcp-stdio-bridge         ← PR 6
  feat/extra-tools              ← PR 7
  feat/permissions              ← PR 10
  feat/vscode                   ← PR 11
```

Cada rama feature hace PR a `feat/claude-code-replacement`, no a `platform` directamente.

### Flujo cuando Dev B depende de algo de Dev A

1. Dev A termina su PR, lo mergea a `feat/claude-code-replacement`
2. Dev A avisa a Dev B
3. Dev B sincroniza su rama local:
   ```bash
   git fetch origin
   git merge origin/feat/claude-code-replacement
   ```
4. Dev B ya tiene los módulos de Dev A disponibles y puede compilar

`feat/claude-code-replacement` es la fuente de verdad compartida — cada developer sincroniza desde ahí cuando necesita lo que hizo el otro.

---

## Coordinación con Dev B

### Día 1 — antes de escribir una sola línea de implementación

Dev A comunica a Dev B los siguientes dos puntos. Hacerlo **antes** de empezar a implementar para que los nombres sean definitivos, no provisionales.

---

**1. Lista exacta de nombres de tools**

Estos son los strings que irán en `dispatch_tool()`. Dev B los necesita exactamente así:

```
"read_file"
"write_file"
"list_dir"
"glob"
"str_replace"
"git_status"
"git_diff"
"git_log"
"fetch_url"
"notebook_edit"
```

---

**2. Cambio en `ToolContext`**

Dev A agrega el campo `cwd` a la struct en `trogon-agent-core/src/tools/mod.rs`:

```rust
// ANTES
pub struct ToolContext {
    pub proxy_url: String,
    pub http_client: reqwest::Client,
}

// DESPUÉS
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // ← Dev A agrega este campo
    pub http_client: reqwest::Client,
}
```

`cwd` se popula desde `session.cwd`, que es el `std::env::current_dir()` que el CLI pasa al crear la sesión ACP.

---

### Qué hace Dev B con esto

Con esa información, Dev B agrega los match arms en `crates/trogon-agent/src/tools/mod.rs`:

```rust
// tools nuevos de Dev A — Dev B agrega estos en dispatch_tool()
"read_file"      => fs::read_file(ctx, input).await,
"write_file"     => fs::write_file(ctx, input).await,
"list_dir"       => fs::list_dir(ctx, input).await,
"glob"           => fs::glob_files(ctx, input).await,
"str_replace"    => editor::str_replace(ctx, input).await,
"git_status"     => git::status(ctx, input).await,
"git_diff"       => git::diff(ctx, input).await,
"git_log"        => git::log(ctx, input).await,
"fetch_url"      => web::fetch_url(ctx, input).await,
"notebook_edit"  => fs::notebook_edit(ctx, input).await,
```

Dev B puede hacer ese PR el mismo día 1 aunque las implementaciones de Dev A no estén listas — mientras los módulos existan (aunque vacíos), compila.

---

### Por qué es crítico hacerlo bien

Si Dev B usa un nombre distinto al que Dev A implementó (ej. `"readFile"` en vez de `"read_file"`), el agente llama al tool y recibe `"Unknown tool: readFile"` en runtime. **No hay error de compilación** — falla silenciosamente.

---

### Archivo compartido con riesgo de conflicto

`trogon-agent-core/src/tools/mod.rs` — los dos pueden agregar entries a `dispatch_tool()`. Coordinarse puntualmente si coinciden en ese archivo al mismo tiempo.
