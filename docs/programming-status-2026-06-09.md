# Estado de programación — hechos verificados (2026-06-09)

Alcance: funcionalidad de programación (CLI + IDE, los 4 runners, cambio de modelo
en media sesión) en las ramas `platform`, `platform-pro` y `feat/cli-fixes`.

**Este documento contiene SOLO lo verificado contra el código.** La clasificación
en estados de proceso ("en curso / en revisión / listo para tomar / no empezado")
NO está aquí porque no es derivable del código — requiere el board/issues del equipo.

## Método de verificación

Cada afirmación de abajo fue verificada por uno de: lectura directa del código
(file:line), grep de identificador exacto, o ejecución de `cargo check` / `cargo test`.
No se verificó comportamiento en vivo contra APIs (Claude/Grok/OpenRouter) ni con un
stack NATS corriendo.

### Build + tests (ejecutado)

| Rama | `cargo check` | `cargo test` (lib) |
|---|---|---|
| `platform` | ✅ exit 0 | ✅ 900 tests, 0 fallos |
| `platform-pro` | ✅ exit 0 | ✅ 679 tests, 0 fallos |
| `feat/cli-fixes` | ✅ exit 0 | ✅ 1.416 tests, 0 fallos |

Crates cubiertos: trogon-cli, trogon-acp-runner, trogon-xai-runner,
trogon-openrouter-runner, trogon-codex-runner, trogon-acp, trogon-runner-tools,
trogon-tools.

---

## COMPLETADO en `platform` (mergeado + compila + tests verdes)

> "Completado" aquí = el código existe en `platform`, compila y pasa sus tests.
> NO significa "sin pendientes" — ver la sección de bugs reales al final.

### Los 4 runners
- **acp-runner (Claude)**: loop agéntico, tools, bash stateful, TROGON.md,
  permisos, compactación 85% single-path (`agent.rs:630-633`), export V2.
- **xai-runner (Grok)**: bash stateful (`terminal_id`/`terminal_wasm_prefix`),
  TROGON.md (`agent.rs:1616`), gate de permisos (`with_permissions`/`check_tool_permission`),
  spawn_agent advertised (`agent.rs:1744`), export/import V1+V2.
- **openrouter-runner**: bash stateful, permisos, TROGON.md, export/import,
  spawn_agent — verificado por grep directo en `agent.rs`.
- **codex-runner**: wrapper de subproceso sobre `codex app-server` (`agent.rs:133-136`);
  acumula historial y exporta turnos completos (`session/export`, `agent.rs:829`).

### CLI (`trogon-cli`)
- REPL con ~22 slash commands activos (`/model /mode /compact /compact-model /init
  /resume /sessions /status /rename /mcp /memory /agents /tasks /plan /vim
  /allowed-tools /review /pr-comments /cd /pwd /clear /doctor`).
- `--print` con exit codes (`print.rs:28-34`: `maxTurnRequests`→2, `error`/`cancelled`→1).
- Resume: `--continue` / `--session-id` vía `SessionIndex::get_last` (`main.rs:391`).
- MCP cableado (`McpManager::load`, `repl.rs:428`).
- `@mentions`; `render_diff` maneja `str_replace`/`write_file` (`session.rs:858`).

### Cambio de modelo en media sesión
- `CrossRunnerSwitcher::switch_model` (`cross_runner.rs:79-162`): `find_by_model`
  → `session/export` → `new_session` → `session/import`. Corre en `LocalSet`.
- El switcher es **agnóstico al formato**: pipea el JSON del export sin deserializar
  (`cross_runner.rs:132-142`). La fidelidad la decide cada runner.
- `registry.find_by_model` (`registry.rs:141`, match exacto en `metadata["models"]`) + 9 tests.

### IDE (`trogon-acp`)
- Servidor ACP stdio + `multi_runner` con switch cross-runner: `set_session_model`
  (`multi_runner.rs:469`) migra historial export→import al cambiar de runner
  (`multi_runner.rs:525-527`; lossy entre proveedores por diseño).

### Fidelidad del export cross-runner
- acp emite **V2 estructurado** (`tool_use`/`tool_result`/`thinking`, `agent.rs:1771`).
- Al importar en un runner de chat (xai/OR), V2 se convierte vía `v2_to_messages`:
  los tool-calls se renderizan a texto (no se pierden; sin replay estructurado).

---

## Hechos de código de `platform-pro` (verificados; estado de proceso aparte)

- 74 commits **detrás** de `platform` (ramificó de base vieja → requiere forward-port).
- `140c2126` **no** es ancestro de `platform`.
- Compactación single-path model-aware: `compaction.rs:102`
  (`estimate_tokens > context_window * 85/100`); el camino env (`maybe_compact`/
  `compaction_settings_from_env`/`trim_history`) está **removido** de su `agent.rs`.
- Propaga `bypassPermissions` a sub-sesiones (`mode: session_mode.clone()`).
- Compila + 679 tests verdes.

## Hechos de código de `feat/cli-fixes` (verificados; estado de proceso aparte)

- PR #205 (base `platform`) **CERRADO sin merge** el 2026-06-08. 11 commits detrás de platform.
- **Ausentes en `platform`** (identificador exacto = 0 ocurrencias), presentes aquí:
  - CFG-1 `effective_model` (modelo por defecto desde settings.json)
  - CFG-2 parse de `_meta.env` al terminal bash en xai/OR
  - CFG-3 parse de `meta.permissionRules` en xai/OR
  - TOOL-1 búsqueda regex (`RegexBuilder`)
  - TOOL-2 background jobs + `bash_output`
  - AGT-1 `tool_allowlist` de sub-agentes + herencia de permisos
  - AGT-2 param `agent` en schema de spawn
  - NEW-3 `DISALLOW`/`NOT ALLOWED` en parse_verdict
  - NEW-4 contención de lectura en bash read-only (`collect_bash_path_args`, `FIND_UNSAFE`)
- **Refinamientos** (platform ya tiene una versión previa): NEW-5 (symlink, fs.rs difiere
  54 líneas), NEW-6 (`--print` truncado; platform tiene `from_stop_reason` sin el arm
  `incomplete`), TOOL-2 timeout configurable.
- Compila + 1.416 tests verdes.

---

## Bugs reales verificados, vivos en `platform`

1. **Compactación doble-camino en xai/openrouter**: camino env pre-turn no model-aware
   (`xai agent.rs:1591-1608`; `openrouter agent.rs:909`) + camino model-aware post-turn
   (`xai agent.rs:2580-2614`; `openrouter agent.rs:1606`). El env compacta prematuro a
   presupuesto fijo modelos de ventana grande (grok-4-fast 2M). `platform-pro` lo unifica.
2. **Reglas de `settings.json` descartadas en xai/OR**: el checker de permisos existe,
   pero xai/OR hardcodean `permission_rules_text=None` y no parsean `meta.permissionRules`
   (solo acp las honra). `feat/cli-fixes` CFG-3 lo arregla (12 líneas).
