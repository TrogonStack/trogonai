# TrogonAI — Programming Assistant: Implementation Status

Branch: `programming-imple`  
Base plan: [`programming.md`](programming.md)  
Last updated: 2026-05-21

---

## Status por PR

### Required PRs

| PR | Título | Estado |
|----|--------|--------|
| PR 1 | Core tools in `trogon-agent-core` | ✅ Implementado |
| PR 2 | Stateful bash | ✅ Implementado |
| PR 3 | `trogon-cli` REPL | ✅ Implementado |
| PR 4 | Auto-compact + TROGON.md injection a todos los runners | ✅ Implementado (ver gap PR 4 abajo) |
| PR 5 | `ext_method` scaffold en xai, codex, openrouter | ✅ Implementado |
| PR 6 | Registry capabilities + model list | ✅ Implementado |
| PR 7 | Cross-runner session export/import | ✅ Implementado + extendido en esta rama |
| PR 8 | `CrossRunnerSwitcher` en `trogon-cli` | ✅ Implementado |
| PR 9 | Slash commands incluyendo `/model` | ✅ Implementado |

### Optional PRs

| PR | Título | Estado |
|----|--------|--------|
| PR 10 | Agent extensibility: @mentions y parallel tools | ✅ Implementado |
| PR 11 | MCP stdio como CLI HTTP proxy | ✅ Implementado |
| PR 12 | Todo tools y specialized sub-agents | ✅ Implementado |
| PR 13 | TUI colored diffs | ✅ Implementado |
| PR 14 | Non-interactive mode y granular permissions | ✅ Implementado |
| PR 15 | `_meta.systemPrompt` en xai-runner y openrouter-runner | ✅ Implementado |
| PR 16 | Spawn handler en xai-runner y openrouter-runner | ✅ Implementado |

---

## Gaps encontrados y arreglados en esta rama

### Fix 1 — Placeholder para mensajes pure-ToolUse en `acp-runner` export

**Archivo:** `trogon-acp-runner/src/agent.rs`

**Problema:** Cuando Claude responde con solo bloques `ToolUse` (sin texto), el export
producía `text: ""`. Al importar ese mensaje en otro runner, la API rechazaba el bloque
de texto vacío.

**Arreglo:** Si después del filtrado de bloques el texto queda vacío, se usa el
placeholder `"[tool call]"`:

```rust
let text = if text.is_empty() { "[tool call]".to_string() } else { text };
```

---

### Fix 2 — PortableBlock: export/import estructurado con tool calls

**Archivos:** `trogon-runner-tools/src/portable_session.rs`, `trogon-acp-runner/src/agent.rs`,
`trogon-xai-runner/src/agent.rs`, `trogon-openrouter-runner/src/agent.rs`,
`trogon-codex-runner/src/agent.rs`

**Problema:** `PortableMessage` solo tenía `{ role, text }`. Al hacer un switch de runner
durante una sesión con tool calls activos, toda la estructura de herramientas (qué tool
se llamó, con qué parámetros, qué devolvió) se perdía. El runner receptor veía el
historial como texto plano.

**Arreglo:** Se añadió `PortableBlock` y el campo `blocks: Vec<PortableBlock>` a
`PortableMessage`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortableBlock {
    Text { text: String },
    ToolCall { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_call_id: String, content: String },
}

pub struct PortableMessage {
    pub role: String,
    pub text: String,   // fallback plano para runners text-only y backward compat
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<PortableBlock>,
}
```

Cada runner actualizado:

| Runner | Export | Import |
|--------|--------|--------|
| `acp-runner` | `ToolUse` → `PortableBlock::ToolCall`, `ToolResult` → `PortableBlock::ToolResult`, `Thinking` descartado | `PortableBlock::ToolCall` → `ContentBlock::ToolUse`, `PortableBlock::ToolResult` → `ContentBlock::ToolResult` |
| `openrouter-runner` | `tool_calls` → `PortableBlock::ToolCall`, `role:"tool"` → `PortableBlock::ToolResult` | `PortableBlock::ToolResult` → `Message::tool_result` × N (flat_map), `PortableBlock::ToolCall` → `Message::assistant_tool_calls` |
| `xai-runner` | `PortableBlock::Text` (history es text-only por diseño — stateful API server-side) | Extrae texto de todos los bloques; `ToolCall` → `"[called: name]"` |
| `codex-runner` | History es text-only; usa `PortableMessage::text_only()` | Sin cambios |

**Backward compat:** El campo `blocks` lleva `#[serde(default, skip_serializing_if = "Vec::is_empty")]`.
El formato antiguo `{"role":"...","text":"..."}` sigue deserializando correctamente.

**Known limitations (sin cambios):**
- `ContentBlock::Thinking` sigue descartándose en export — no existe equivalente en ninguna otra API.
- `xai-runner` no exporta tool calls porque no los guarda en history local; usa `last_response_id`
  para statefulness server-side en la xAI Responses API.

---

### Fix 3 — Normalización de `role:"tool"` → `role:"user"` en `acp-runner` import

**Archivo:** `trogon-acp-runner/src/agent.rs`

**Problema:** `openrouter-runner` exporta mensajes de tool result con `role: "tool"` (formato
OpenAI). Al importar en `acp-runner`, se creaban mensajes `Message { role: "tool", ... }`.
La Anthropic API solo acepta `"user"` y `"assistant"` — rechaza `"tool"`.

**Arreglo:** En el import de acp-runner, si el contenido reconstruido contiene bloques
`ToolResult`, el role se normaliza a `"user"`:

```rust
let role = if content.iter().any(|b| matches!(b, AgentContentBlock::ToolResult { .. })) {
    "user".to_string()
} else {
    m.role
};
Message { role, content }
```

La dirección inversa (acp → openrouter) ya era correcta: `openrouter-runner` import
detecta `PortableBlock::ToolResult` y llama a `Message::tool_result(...)` que siempre
fuerza `role: "tool"` independientemente del role del PortableMessage.

---

---

### Fix 4 — codex-runner: tool calls no se guardaban en history

**Archivo:** `trogon-codex-runner/src/agent.rs`

**Problema:** `CodexEvent::ToolStarted { id, name, input }` y `CodexEvent::ToolCompleted { id, output }`
ya existían en el protocolo y se procesaban para enviar `SessionUpdate::ToolCall` al stream ACP,
pero no se escribían en `s.history`. Al hacer export desde codex, el historial nunca incluía
las herramientas llamadas — solo el texto final.

**Arreglo:** Se acumulan dos vecs durante el turn (`tool_call_blocks`, `tool_result_blocks`).
En `TurnCompleted` se escriben tres entradas separadas en `s.history`:

1. `role:"assistant"`, `text:"[tool call]"`, `blocks:[PortableBlock::ToolCall, ...]` — si hubo tool calls
2. `role:"user"`, `text:""`, `blocks:[PortableBlock::ToolResult, ...]` — si hubo resultados
3. `role:"assistant"`, `text:"<texto final>"` — siempre

Esta separación respeta el formato esperado por los importadores:
- `acp-runner`: assistant con `ToolUse` + user con `ToolResult` — formato Anthropic válido
- `openrouter-runner`: `flat_map` explota los ToolResult en mensajes `role:"tool"` individuales

---

### Fix 5 — codex-runner `session/import`: no escribía en `s.history`

**Archivo:** `trogon-codex-runner/src/agent.rs`

**Problema:** `session/import` solo seteaba `s.pending_history`. Si se llamaba a
`session/export` antes del primer prompt (para inspectar o re-exportar la sesión
recién importada), devolvía una lista vacía — inconsistente con todos los demás runners.

**Arreglo:**

```rust
s.history = messages.clone();
s.pending_history = Some(messages);
```

---

### Fix 6 — xai-runner `session/import`: no limpiaba `last_response_id`

**Archivo:** `trogon-xai-runner/src/agent.rs`

**Problema:** Al importar una sesión en xai-runner, `last_response_id` quedaba con el
valor de la sesión anterior. En el siguiente prompt, xai enviaba solo el nuevo mensaje
usando el thread del servidor xAI en lugar de enviar el historial completo importado —
lo que causaba que el modelo ignorara el contexto importado.

**Arreglo:**

```rust
s.last_response_id = None;
```

---

## Gap pendiente (no implementado en esta rama)

### Gap PR 4 — codex-runner: no inyecta TROGON.md en el primer turno

**Archivo:** `trogon-codex-runner/src/agent.rs` (líneas 524–547)

El campo `first_turn` existe y se actualiza a `false`, pero su valor no se captura
antes del set. El `cwd` tampoco se extrae en el mismo lock block. La rama `else if first_turn`
que debería llamar a `load_trogon_md` nunca se ejecuta.

**Fix requerido:**

```rust
let (thread_id, model, pending_history, first_turn, cwd) = {
    let mut sessions = self.sessions.lock().await;
    let s = sessions.get_mut(&session_id)...;
    let ph = s.pending_history.take();
    let ft = s.first_turn;           // capturar ANTES de poner false
    s.first_turn = false;
    s.history.push(...);
    (s.thread_id.clone(), s.model.clone(), ph, ft, s.cwd.clone())
};

let user_input = if let Some(prior) = pending_history {
    format!("Prior conversation:\n{formatted}\n\n---\n\n{user_input}")
} else if first_turn {
    match trogon_runner_tools::trogon_md::load_trogon_md(&cwd).await {
        Some(md) => format!("{md}\n\n{user_input}"),
        None => user_input,
    }
} else {
    user_input
};
```

---

## Gaps verificados como ya implementados

### Gap A — Tool execution en xai-runner y openrouter-runner

**Verificado:** Ambos runners tienen tool execution completo — no era un gap pendiente.

| Runner | Implementación |
|--------|---------------|
| `xai-runner` | `ToolSpec::Function` desde `trogon_tools::all_tool_defs()` → envío al Responses API → parseo de `function_call` blocks → `dispatch_tool` → follow-up con `previous_response_id` + `function_call_output` |
| `openrouter-runner` | `ToolDef` desde `trogon_tools::all_tool_defs()` → chat completions con `tool_defs` → parseo de `tool_calls` SSE deltas → `dispatch_tool` → `role:"tool"` result messages |

Las sesiones se crean con todos los tools habilitados por defecto. La nota del gap describía el plan que ya había sido ejecutado antes de esta rama.

---

## Resumen de archivos modificados en esta rama

| Archivo | Cambios |
|---------|---------|
| `trogon-runner-tools/src/portable_session.rs` | Añadido `PortableBlock`, campo `blocks`, constructor `text_only()`, tests |
| `trogon-acp-runner/src/agent.rs` | Export: bloques estructurados + placeholder; Import: reconstrucción desde blocks + normalización role "tool"→"user"; Tests actualizados |
| `trogon-xai-runner/src/agent.rs` | Export: añade `PortableBlock::Text`; Import: extrae texto desde blocks; limpia `last_response_id` |
| `trogon-openrouter-runner/src/agent.rs` | Export: serializa `tool_calls`/`tool_call_id` como `PortableBlock`; Import: flat_map para ToolResult explosion, reconstrucción tool_calls |
| `trogon-codex-runner/src/agent.rs` | Struct literals → `text_only()`; acumula `tool_call_blocks`/`tool_result_blocks`; escribe 3 entradas en history en `TurnCompleted` |
