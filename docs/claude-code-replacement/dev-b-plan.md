# Dev B — Plan de trabajo
# CLI track (superficie del developer)

> Todo el trabajo es aditivo. NATS permanece como backend sin cambios.
> Los nombres de todos los tools y el contrato de ToolContext están fijados en la sección "Contrato de tools" al final de este documento.

---

## Resumen de waves

| Wave | PRs | Esfuerzo |
|---|---|---|
| Wave 1 | PR 3 + wiring dispatch | 1–2 semanas |
| Wave 2 | PR 5 | 1 semana |
| Wave 3 | PR 8 + PR 9 | 2–3 semanas |
| Wave 4 | PR 10 (no-interactivo) + PR 12 | 2–3 semanas |
| **Total** | | **6–9 semanas** |

---

## Wave 1

### PR 3 — `trogon-cli` core

**`crates/trogon-cli/Cargo.toml` — crate nueva**
```toml
rustyline = "14"
# acp-nats, clap, axum — ya en el workspace
```

**`crates/trogon-cli/src/main.rs`**
- Parsing de argumentos con clap
- Lee `TROGON_NATS_URL` (default `nats://localhost:4222`)
- NATS autostart:
  - Si la conexión falla, lanza `nats-server -p 4222` como proceso hijo
  - Reintenta hasta 3s con intervalos de 200ms
  - Si `nats-server` no está en PATH: imprime instrucciones de instalación y sale con error claro
  - Al salir: mata el proceso hijo solo si fue este proceso quien lo lanzó

**`crates/trogon-cli/src/session.rs`**
- Gestión de sesión ACP via NATS
- Crea sesión con `cwd = std::env::current_dir()`
- Cierra sesión limpiamente al salir (Ctrl+D)

**`crates/trogon-cli/src/repl.rs`**
- Loop REPL con rustyline
- Recibe stream de eventos ACP e imprime `TextDelta` en tiempo real
- Ctrl+C envía cancel a la sesión activa y limpia el estado
- Ctrl+D sale limpiamente cerrando la sesión
- Historial persistido en `~/.local/share/trogon/history`

**`crates/trogon-cli/src/print.rs`**
- Stub vacío por ahora (se completa en PR 10)

**Rama:** `feat/cli-core`

---

### Wiring en `trogon-agent` — Wave 1

**`crates/trogon-agent/src/tools/mod.rs`**

Registrar en `dispatch_tool()` todos los tools que Dev A implementa en PR 1:
- `"read_file"`
- `"write_file"`
- `"list_dir"`
- `"glob"`
- `"str_replace"`
- `"git_status"`
- `"git_diff"`
- `"git_log"`
- `"fetch_url"`
- `"notebook_edit"`

> Esperar confirmación de Dev A el día 1 con los nombres exactos y la firma de `ToolContext` antes de hacer este PR.

**Rama:** parte de `feat/cli-core` o rama propia `feat/dispatch-wiring`

---

## Wave 2

### PR 5 — Extensibilidad del agente

**`crates/trogon-cli/src/repl.rs` — `@mentions` de archivos**
- Antes de enviar el prompt al agente, escanear el texto en busca de tokens `@<path>`
- Para cada match:
  - Resolver el path relativo a `cwd`
  - Leer el contenido del archivo
  - Sustituir `@<path>` por bloque de código con el contenido
- Si el path no existe: dejar el token sin modificar y advertir al usuario
- Tab-completion del path en rustyline via un `Helper` personalizado

**`crates/trogon-agent-core/src/agent_loop.rs` — herramientas paralelas**

Reemplazar ejecución secuencial por paralela:
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

**Rama:** `feat/mentions-parallel`

---

## Wave 3

### PR 8 — TUI

**`crates/trogon-cli/src/repl.rs`**
- Mostrar diffs coloreados (antes/después) en cada operación `str_replace` y `write_file`
- Ctrl+C cancela la operación NATS activa y limpia el estado de sesión
- Consumir `UsageSummary` events (ya emitidos por el platform branch) y mostrar tokens y $ acumulados por sesión
- Input multilinea

**Rama:** `feat/cli-tui`

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

**Rama:** `feat/slash-commands`

---

## Wave 4

### PR 10 — Modo no-interactivo

**`crates/trogon-cli/src/print.rs`**
- Activado con `trogon --print "haz X"` o `trogon -p "haz X"`
- Lee prompt desde argumento o desde stdin: `trogon --print "explica" < error.log`
- Imprime solo `TextDelta` a stdout (sin colores ni UI interactiva)
- Exit code 0 si completa sin error, 1 si el agente devuelve error
- Útil para pipes y CI/CD

**Rama:** `feat/cli-noninteractive`

---

### PR 12 — Plugin JetBrains (`trogon-jetbrains`)

- Mismo concepto que la extensión VS Code (que hace Dev A)
- API de plugins de JetBrains (IntelliJ, GoLand, RustRover, etc.)
- Panel de chat dentro del editor
- Inline diffs con aceptar/rechazar cambios
- Slash commands desde el editor
- Comunica con `trogon-cli` o directamente via NATS/ACP

**Rama:** `feat/jetbrains`

---

## Ramas de trabajo

```
feat/claude-code-replacement    ← rama base compartida con Dev A
  feat/cli-core                 ← PR 3 + wiring dispatch
  feat/mentions-parallel        ← PR 5
  feat/cli-tui                  ← PR 8
  feat/slash-commands           ← PR 9
  feat/cli-noninteractive       ← PR 10
  feat/jetbrains                ← PR 12
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

## Contrato de tools — nombres definitivos

Los nombres están fijados aquí. **No hay coordinación pendiente.** Dev A los implementa exactamente así, Dev B los registra exactamente así.

### Nombres de tools

| Tool | String en `dispatch_tool()` |
|---|---|
| Leer archivo | `"read_file"` |
| Escribir archivo | `"write_file"` |
| Listar directorio | `"list_dir"` |
| Buscar por patrón | `"glob"` |
| Editar con reemplazo | `"str_replace"` |
| Git status | `"git_status"` |
| Git diff | `"git_diff"` |
| Git log | `"git_log"` |
| Fetch URL | `"fetch_url"` |
| Editar notebook | `"notebook_edit"` |
| Crear/actualizar tarea | `"todo_write"` |
| Leer tareas | `"todo_read"` |
| Lanzar sub-agente | `"spawn_agent"` |

### Cambio en `ToolContext`

Dev A agrega el campo `cwd` en `crates/trogon-agent-core/src/tools/mod.rs`:

```rust
pub struct ToolContext {
    pub proxy_url: String,
    pub cwd: String,           // directorio de trabajo — viene de session.cwd = current_dir()
    pub http_client: reqwest::Client,
}
```

### Wiring que hace Dev B — código exacto

Abrir `crates/trogon-agent/src/tools/mod.rs`, agregar los imports y los match arms en `dispatch_tool()`:

```rust
use trogon_agent_core::tools::{fs, editor, git, web};

// en dispatch_tool() — agregar junto a los tools existentes de GitHub/Linear/Slack:
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
"todo_write"     => todo::write(ctx, input).await,
"todo_read"      => todo::read(ctx, input).await,
"spawn_agent"    => agent::spawn(ctx, input).await,
```

Dev B puede hacer este PR aunque las implementaciones de Dev A no estén listas — mientras los módulos existan (aunque vacíos), compila.

### Archivo compartido con riesgo de conflicto

`trogon-agent-core/src/tools/mod.rs` — Dev A y Dev B agregan entries a `dispatch_tool()`. Coordinarse puntualmente si coinciden en ese archivo al mismo tiempo.
