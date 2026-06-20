# Cambio de modelo en medio de una sesion

## Objetivo

Trogonai debe permitir cambiar de modelo en medio de una sesion sin que el usuario sienta que perdio contexto, permisos, decisiones, archivos, tareas o continuidad de trabajo.

El cambio de modelo debe sentirse como:

> Sigo en la misma sesion, pero a partir de ahora responde otro modelo.

No debe sentirse como:

> Se creo otra sesion parecida, con parte del historial copiado.

Para lograrlo, la sesion debe pertenecer a Trogonai y no al runner. Los runners deben ser bindings runtime intercambiables que reciben una proyeccion de la sesion y devuelven eventos normalizados.

La forma mas solida de hacerlo en Trogonai es implementar un **Session Kernel sobre NATS/JetStream**:

- JetStream como event log append-only de la sesion;
- Event Contract con ordering e idempotencia para retries seguros;
- NATS KV como snapshots/materialized state;
- Session Lease por `session_id` para serializar operaciones mutadoras;
- object/artifact store para outputs grandes, imagenes, logs y archivos;
- subjects NATS para bindings runtime con runners;
- registry para resolver modelo -> runner -> capabilities;
- Context Twin para preservar continuidad operacional;
- Capability Negotiation para adaptar el switch sin degradaciones silenciosas;
- Switch Safety Gate para bloquear o pedir confirmacion cuando no es seguro cambiar ahora;
- Continuity Checkpoint para validar que el modelo destino entendio el estado antes de ejecutar acciones riesgosas.

## Principio central

La arquitectura correcta a largo plazo es separar diez conceptos:

1. **Sesion Trogonai**
   - Es la identidad estable de la conversacion y del trabajo.
   - Tiene un `session_id` unico y durable.
   - Contiene historial, eventos, tool calls, resultados, artefactos, permisos, configuracion portable, `compactor_model`, summaries, todos y uso de tokens.

2. **Session Kernel**
   - Es el nucleo operativo que controla la sesion.
   - Escribe eventos en JetStream.
   - Materializa snapshots en NATS KV.
   - Compila proyecciones de contexto para cada modelo.
   - Decide que estado es portable y que estado debe invalidarse.

3. **Session Lease**
   - Serializa operaciones mutadoras por `session_id`.
   - Evita prompts, switches, compactions, imports o tool mutations simultaneas sobre la misma sesion.
   - Usa TTL/renew para que una sesion no quede bloqueada si un proceso cae.

4. **Event Contract, Ordering and Idempotency**
   - Define los campos obligatorios de cada evento.
   - Garantiza orden monotono por sesion con `seq`.
   - Permite reintentos seguros con `operation_id`, `idempotency_key` y receipts de tools.

5. **Context Twin**
   - Es una vista derivada del estado operacional de la sesion.
   - Resume objetivo actual, plan activo, decisiones, archivos relevantes, constraints, errores abiertos, tests, artefactos importantes y proximos pasos.
   - No reemplaza al transcript ni al event log. Sirve para que un modelo nuevo entienda rapidamente donde esta parado el trabajo.

6. **Capability Negotiation**
   - Compara capacidades del modelo/runner actual contra el modelo/runner destino.
   - Produce un plan explicito de adaptacion antes del switch.
   - Evita degradaciones silenciosas de tools, imagenes, contexto largo, JSON schema, reasoning o formatos de tool result.

7. **Switch Safety Gate**
   - Es una compuerta previa al cambio de modelo.
   - Decide si es seguro cambiar ahora, si hace falta confirmacion o si el switch debe bloquearse temporalmente.
   - Protege contra perdida critica por tool calls en progreso, streams incompletos, artefactos sin persistir, operaciones destructivas pendientes o capabilities indispensables ausentes.

8. **Continuity Checkpoint**
   - Es una verificacion ligera posterior al switch.
   - Comprueba que el modelo destino entendio objetivo, plan, archivos relevantes, ultimo estado y proximos pasos.
   - Si hay mismatch fuerte, Trogonai repara contexto, agrega artefactos, advierte degradacion o bloquea acciones riesgosas.

9. **Runner**
   - Es el adaptador que sabe hablar con un proveedor o motor especifico.
   - Puede ser ACP, OpenRouter, xAI, Codex u otro.
   - No debe ser el dueno semantico de la sesion.
   - Debe comportarse como un binding runtime descartable o recreable.

10. **Modelo**
   - Es una capacidad seleccionable para generar el siguiente turno.
   - Puede cambiar durante la misma sesion.
   - Tiene capabilities propias: tools, imagenes, contexto largo, JSON schema, reasoning, streaming, etc.

El error de producto seria acoplar la continuidad de la sesion al estado interno de cada runner. Eso hace que cambiar de modelo sea una migracion parcial y fragil.

La sesion no debe copiarse entre runners. La sesion debe vivir en Trogonai, y cada runner debe recibir una vista compatible con el modelo actual.

## Problema del enfoque actual

El enfoque actual de cross-runner switching se parece a:

1. Exportar mensajes desde el runner actual.
2. Crear una sesion nueva en el runner destino.
3. Importar esos mensajes.
4. Continuar desde el nuevo `session_id` del runner.

Esto funciona como handoff conversacional, pero no como migracion completa de sesion.

El problema no es que sea inutil; el problema es que la semantica puede ser confundida. Si el producto lo presenta como "cambiar modelo dentro de la misma sesion", el usuario espera que todo lo importante se preserve. Pero en realidad hay estado que no viaja:

- permisos concedidos;
- policies;
- MCP servers;
- tools habilitadas/deshabilitadas;
- terminal persistente;
- cwd del terminal;
- todos;
- audit log;
- usage acumulado;
- summaries;
- response IDs del proveedor;
- thread IDs internos;
- tool inputs/outputs completos si el formato portable los resume o trunca;
- imagenes o artefactos grandes;
- pending tool calls;
- estado en memoria del runner.

Por eso, a largo plazo, el switch no debe depender de export/import entre runners como mecanismo principal. El runner destino debe hidratarse desde el Session Kernel de Trogonai.

## Comportamiento esperado de producto

Cuando el usuario cambia de modelo, Trogonai debe garantizar:

1. **Misma sesion visible**
   - El `session_id` de Trogonai no cambia.
   - La UI/CLI no debe llevar al usuario a otra sesion salvo que el usuario pida fork o branch.

2. **Historial preservado**
   - La conversacion completa sigue existiendo en la sesion canonica.
   - Si el nuevo modelo no puede recibir todo el historial por limite de contexto, Trogonai usa una proyeccion o summary, pero no destruye el historial original.

3. **Tools y permisos coherentes**
   - Las tools disponibles deben recalcularse segun el modelo y runner destino.
   - Las decisiones portables de permisos deben preservarse.
   - Las decisiones no aplicables al nuevo runner deben marcarse como no portables.

4. **Estado runtime explicito**
   - Trogonai no debe fingir que puede migrar procesos vivos, terminal IDs, provider response IDs o thread IDs.
   - Ese estado debe invalidarse o recrearse de forma controlada.

5. **Degradacion clara**
   - Si el modelo destino no soporta una capacidad usada en la sesion, Trogonai debe registrar el fallback.
   - Ejemplos:
     - imagenes convertidas a descripcion o referencia;
     - tool calls pasados convertidos a transcript textual;
     - outputs grandes reemplazados por preview + artifact ref;
     - reasoning oculto no portable;
     - JSON schema no soportado.

## Arquitectura recomendada: Session Kernel sobre NATS

### 1. Session Kernel

Crear un nucleo de sesion propio de Trogonai, independiente del runner.

El Session Kernel es la fuente de verdad para:

- metadata de sesion;
- timeline de eventos;
- event contract;
- session leases;
- mensajes;
- tool calls;
- tool results;
- artefactos;
- summaries;
- Context Twin;
- switch adaptation plans;
- switch safety decisions;
- continuity checkpoints;
- configuracion portable;
- `compactor_model`;
- permisos;
- audit log;
- todos;
- usage;
- cambios de modelo;
- bindings runtime activos.

Los runners pueden mantener caches o estado auxiliar, pero ese estado no debe ser necesario para entender o continuar la sesion.

El kernel debe ofrecer operaciones como:

```text
acquire_session_lease(session_id, operation)
renew_session_lease(session_id, lease_id)
release_session_lease(session_id, lease_id)
append_event(session_id, event)
append_event_idempotent(session_id, event, idempotency_key)
load_snapshot(session_id)
materialize_state(session_id)
update_context_twin(session_id)
create_switch_adaptation_plan(session_id, target_model)
evaluate_switch_safety(session_id, target_model)
compile_prompt(session_id, model_capabilities)
run_continuity_checkpoint(session_id, target_model)
attach_runner(session_id, runner, model)
detach_runner(session_id, runner)
```

La implementacion debe usar NATS como backend interno, no una base de datos paralela.

### 2. Session Lease

Toda operacion mutadora sobre una sesion debe tomar un lease por `session_id`.

El mecanismo base no debe inventarse de cero: el repo ya tiene `trogon_nats::lease` con `NatsKvLease`, `LeaseKey`, `LeaseTtl`, `TryAcquireLease`, `RenewLease` y `ReleaseLease`. El Session Kernel debe reutilizar esa infraestructura y agregar solo la politica especifica de sesiones.

El lease debe cubrir la operacion logica completa, no solo una escritura puntual en KV.

Ejemplos de operaciones que requieren lease:

- prompt/turn;
- switch model;
- compact/session summary;
- fork/branch;
- restore/import;
- close/delete session;
- tool calls mutadores;
- cambios de permissions/policies que afecten ejecucion.

Operaciones read-only como list/history/view pueden ejecutarse sin lease.

Flujo conceptual:

```text
acquire lease sessions.{session_id}.lock
run mutation
append events
materialize snapshot
release lease
```

El lease debe tener TTL y renovacion:

- TTL corto;
- heartbeat/renew mientras dura la operacion;
- expiracion automatica si el proceso muere;
- relectura del event log antes de reintentar.

Si el usuario intenta cambiar de modelo mientras hay una operacion en curso, Trogonai debe responder con una politica clara:

```text
La sesion esta ocupada procesando otro turno.
Opciones: esperar, cancelar o reintentar cuando sea seguro.
```

Esto evita interleavings corruptos como `model_switched` entre `tool_call_requested` y `tool_call_completed`, snapshots viejos pisando nuevos, o runner detach mientras aun llegan eventos del runner anterior.

### 3. JetStream event log

La verdad primaria debe ser un log append-only por sesion.

Subject conceptual:

```text
sessions.{session_id}.events
```

Eventos esperados:

```text
session_created
user_message_added
assistant_message_started
assistant_message_completed
tool_call_requested
tool_call_approved
tool_call_completed
tool_call_failed
artifact_created
file_changed
summary_created
context_twin_updated
switch_adaptation_plan_created
switch_safety_evaluated
continuity_checkpoint_started
continuity_checkpoint_completed
model_switched
runner_attached
runner_detached
permission_rule_added
todo_updated
session_compacted
```

Esto es mejor que depender solo de snapshots porque permite:

- reconstruir la sesion despues de un crash;
- auditar que paso y con que modelo;
- debuggear tool calls;
- hacer fork/branch real;
- regenerar snapshots;
- comparar prompt projections;
- migrar formatos en el futuro sin perder historia.

### 4. Event Contract, Ordering and Idempotency

El event log no debe ser solo append de JSON. Cada evento debe tener un contrato estable para poder reconstruir, reintentar y deduplicar sin ambiguedad.

Contrato minimo:

```json
{
  "event_id": "evt_...",
  "session_id": "sess_...",
  "seq": 42,
  "type": "tool_call_completed",
  "created_at": "2026-06-10T00:00:00Z",
  "operation_id": "op_...",
  "correlation_id": "corr_...",
  "causation_id": "evt_...",
  "idempotency_key": "idem_...",
  "actor": {
    "type": "runner",
    "id": "openrouter"
  },
  "payload": {}
}
```

Campos obligatorios:

- `event_id`: identidad unica del evento;
- `session_id`: sesion a la que pertenece;
- `seq`: orden monotono por sesion;
- `created_at`: timestamp para observabilidad, no para ordering;
- `operation_id`: operacion logica completa, como prompt turn o switch;
- `correlation_id`: agrupa eventos del mismo flujo;
- `causation_id`: evento que causo este evento;
- `idempotency_key`: evita duplicados en retries;
- `actor`: quien emitio el evento;
- `payload`: datos especificos del evento.

El ordering debe depender de `seq`, no de `created_at`. Aunque JetStream tenga sequence global, Trogonai debe mantener `seq` monotono por sesion para reconstruccion local.

Flujo de append:

```text
acquire Session Lease
load last session seq
append event with seq + 1
materialize snapshot with last_applied_seq
release Session Lease
```

Los snapshots deben guardar:

```json
{
  "session_id": "sess_...",
  "last_applied_seq": 42,
  "state": {}
}
```

Al reconstruir:

1. cargar snapshot;
2. leer eventos con `seq > last_applied_seq`;
3. aplicar en orden;
4. ignorar duplicados por `event_id` o `idempotency_key`.

Para tools, Trogonai debe usar outbox/receipts:

```text
tool_call_requested
tool_call_started
tool_call_completed
tool_call_failed
```

Cada ejecucion debe tener `tool_execution_id`. Antes de reintentar una tool, la capa de tools debe revisar si ya existe un receipt o resultado para ese `tool_execution_id`.

Regla de seguridad:

> Si una tool mutation no es claramente idempotente, Trogonai no debe reejecutarla automaticamente despues de un crash o retry.

En esos casos el estado debe marcarse como:

```text
requires_reconciliation
```

Y Trogonai debe pedir confirmacion, inspeccionar estado o continuar desde un resultado persistido.

### 5. NATS KV para snapshots

KV debe guardar estado materializado derivado del event log.

Keys conceptuales:

```text
sessions.{session_id}.state
sessions.{session_id}.summary
sessions.{session_id}.context_twin
sessions.{session_id}.switch_adaptation_plan
sessions.{session_id}.switch_safety
sessions.{session_id}.continuity_checkpoint
sessions.{session_id}.routing
sessions.{session_id}.todos
sessions.{session_id}.permissions
sessions.{session_id}.usage
```

El snapshot acelera list/load/resume, pero no debe ser la unica fuente de verdad para eventos importantes.

### 6. Artifact store

Tool outputs grandes, imagenes, logs, diffs y archivos generados no deben vivir solo dentro del mensaje.

El primer candidato de implementacion debe ser NATS Object Store usando la abstraccion existente `trogon_nats::jetstream::NatsObjectStore`. Para payloads que exceden el limite de NATS, debe reutilizarse el mecanismo claim-check existente (`ClaimCheckPublisher`) en vez de crear un artifact transport paralelo.

Se deben guardar como artefactos con:

- `artifact_id`;
- `sha256`;
- `size_bytes`;
- `mime`;
- `preview`;
- `storage_ref`;
- `created_by_event_id`;
- `truncated`.

El prompt projection puede usar previews o referencias. La sesion canonica conserva la referencia completa.

### 7. Context Twin

El Context Twin es una vista derivada y compacta del estado operacional de la sesion.

Debe responder rapidamente:

- cual es el objetivo actual;
- que plan esta activo;
- que decisiones ya se tomaron;
- que archivos son relevantes;
- que constraints o preferencias dio el usuario;
- que errores o riesgos siguen abiertos;
- que tests se ejecutaron y con que resultado;
- que tool results importan para seguir;
- que artefactos existen;
- cuales son los proximos pasos esperados.

El Context Twin no debe ser editado manualmente por el modelo como fuente de verdad. Debe derivarse del event log y guardarse como snapshot en KV.

Key conceptual:

```text
sessions.{session_id}.context_twin
```

Evento asociado:

```text
context_twin_updated
```

Esto mejora el cambio de modelo porque el runner destino no recibe solo historial. Recibe una representacion clara del estado de trabajo.

### 8. Capability Negotiation

Antes de cambiar de modelo, Trogonai debe comparar las capacidades del modelo actual y del modelo destino.

Debe producir un plan explicito:

```json
{
  "type": "switch_adaptation_plan_created",
  "from_model": "anthropic/claude-sonnet",
  "to_model": "xai/grok-code-fast",
  "adaptations": [
    { "capability": "image_input", "action": "use_artifact_refs" },
    { "capability": "tool_use", "action": "preserve_structured_tools" },
    { "capability": "context_window", "action": "use_context_twin_plus_recent_turns" }
  ],
  "warnings": []
}
```

La negociacion debe decidir:

- que se manda igual;
- que se resume;
- que se convierte a texto;
- que queda como artifact ref;
- que no es portable;
- que degradacion debe ver el usuario.

Sin este paso, el switch puede perder informacion de forma silenciosa.

### 9. Switch Safety Gate

El Switch Safety Gate decide si es seguro cambiar de modelo en este momento.

Capability Negotiation responde si el modelo destino puede recibir o usar el contexto. El Safety Gate responde una pregunta distinta:

> Es seguro cambiar ahora?

Debe evaluar:

- tool calls en progreso;
- streams incompletos;
- operaciones destructivas pendientes;
- terminales o procesos vivos que son necesarios para continuar;
- artefactos aun no persistidos;
- cambios de archivo sin registrar;
- Context Twin desactualizado;
- adaptation plan con degradacion critica;
- modelo destino sin una capability indispensable;
- checkpoint anterior fallido o sin reparar.

Resultado conceptual:

```json
{
  "type": "switch_safety_evaluated",
  "status": "allowed_with_warning",
  "reasons": [
    { "kind": "capability_degradation", "detail": "target model does not support image input" }
  ],
  "required_action": "user_confirmation"
}
```

Estados posibles:

```text
allowed
allowed_with_warning
requires_user_confirmation
blocked_until_safe
```

Si el resultado es `blocked_until_safe`, Trogonai debe terminar, cancelar o persistir el estado pendiente antes de cambiar. Si el resultado es `requires_user_confirmation`, la UI/CLI debe explicar claramente que se perdera o degradara.

Evento asociado:

```text
switch_safety_evaluated
```

### 10. Continuity Checkpoint

El Continuity Checkpoint valida que el modelo destino entendio el estado de la sesion antes de ejecutar acciones riesgosas.

No debe ser un proceso pesado para todos los switches. Debe activarse especialmente cuando:

- se cambia de proveedor;
- el modelo destino tiene menos contexto;
- el modelo destino no soporta alguna capability usada en la sesion;
- la sesion es larga;
- hay tool calls, permisos o artefactos criticos;
- el usuario esta en medio de una tarea riesgosa.

Flujo conceptual:

```text
compile prompt for target model
ask target model for state acknowledgement
compare acknowledgement with Context Twin
repair context or warn if mismatch is high
continue only when continuity is acceptable
```

El acknowledgement debe ser interno y breve. Debe cubrir:

- objetivo actual;
- plan activo;
- archivos relevantes;
- ultimo cambio importante;
- tests o validaciones recientes;
- errores o riesgos abiertos;
- siguiente paso esperado.

Eventos asociados:

```text
continuity_checkpoint_started
continuity_checkpoint_completed
```

Resultado conceptual:

```json
{
  "type": "continuity_checkpoint_completed",
  "status": "passed",
  "confidence": 0.91,
  "mismatches": [],
  "repairs_applied": []
}
```

Si falla, Trogonai puede:

- recompilar el prompt con mas contexto;
- incluir artefactos especificos;
- cambiar a summary alternativo;
- mostrar una advertencia;
- bloquear tool calls destructivos hasta que el usuario confirme.

### 11. Transcript no-lossy

El historial canonico no debe truncar datos como verdad principal.

Debe preservar:

- role;
- content blocks;
- tool use ID;
- parent tool use ID;
- tool name;
- input JSON completo;
- result completo o referencia a artefacto;
- status;
- errores;
- timestamps;
- modelo usado;
- runner usado;
- cwd;
- token usage;
- informacion de truncamiento si aplica.

El truncamiento solo debe ocurrir al construir una vista para el modelo, no al guardar la sesion.

### 12. Prompt Compiler

Cada modelo necesita una proyeccion distinta del mismo historial. Esa responsabilidad debe vivir en un Prompt Compiler, no en cada runner de forma ad hoc.

Ejemplos:

- Un modelo con tools recibe tool calls estructurados.
- Un modelo sin tools recibe una narracion textual de tool calls anteriores.
- Un modelo sin imagenes recibe referencias o descripciones.
- Un modelo con poco contexto recibe summary + ultimos turnos.
- Un modelo con contexto largo puede recibir mas transcript completo.

La proyeccion debe ser derivada, descartable y reconstruible desde la sesion canonica.

El compiler debe tomar:

```text
canonical_session + context_twin + switch_adaptation_plan + model_capabilities + runtime_policy -> provider_prompt
```

Y debe producir metadata de degradacion:

```text
images_omitted
tool_calls_textualized
artifacts_referenced
history_summarized
context_twin_included
switch_adaptation_applied
reasoning_not_portable
```

### 13. Runner bindings

Cada runner debe ser un binding runtime temporal.

Cambiar de modelo no debe copiar la sesion. Debe cambiar el binding activo:

```text
detach openrouter/claude
attach xai/grok
compile prompt for grok
continue
```

Cada runner debe implementar una interfaz conceptual parecida a:

```text
run_turn(provider_prompt, tools) -> stream/provider_events
normalize_response(provider_events) -> canonical_events
```

El runner no decide que es la sesion. Solo traduce entre Trogonai y el proveedor.

### 14. Model capabilities

El registry de modelos debe incluir capabilities reales:

- max context;
- max output;
- tool use;
- parallel tool calls;
- image input;
- file input;
- structured output / JSON schema;
- reasoning;
- streaming;
- function/tool result format;
- system prompt support;
- provider-specific restrictions.

El switch debe validar estas capabilities antes de ejecutar el siguiente turno.

### 15. Evento de cambio de modelo

Cambiar de modelo debe registrarse como evento dentro de la misma sesion:

```json
{
  "type": "model_switched",
  "from_runner": "openrouter",
  "from_model": "anthropic/claude-sonnet",
  "to_runner": "xai",
  "to_model": "grok-code-fast",
  "reason": "user_requested",
  "timestamp": "..."
}
```

Esto permite auditar por que una respuesta fue generada por otro modelo y reconstruir la historia del trabajo.

## SessionEnvelope como snapshot, no como fuente primaria

El formato portable deberia evolucionar desde mensajes resumidos hacia un envelope versionado, pero el envelope no debe reemplazar al event log.

La regla correcta:

> Event log es la fuente de verdad. SessionEnvelope es snapshot/export/cache.

Ejemplo conceptual:

```json
{
  "version": 1,
  "session": {
    "id": "session_...",
    "title": "...",
    "cwd": "...",
    "created_at": "...",
    "updated_at": "..."
  },
  "config": {
    "model": "...",
    "compactor_model": null,
    "system_prompt": null,
    "system_prompt_override": null,
    "additional_roots": [],
    "additional_read_dirs": [],
    "mcp_servers": [],
    "tool_policies": [],
    "egress_policy": null,
    "permission_rules_text": null,
    "disabled_builtin_tools": false
  },
  "conversation": [],
  "tool_calls": [],
  "artifacts": [],
  "summaries": [],
  "context_twin": {},
  "switch_adaptation_plan": {},
  "switch_safety": {},
  "continuity_checkpoint": {},
  "todos": [],
  "audit_log": [],
  "usage": {
    "input_tokens": 0,
    "output_tokens": 0,
    "cache_creation_tokens": 0,
    "cache_read_tokens": 0
  },
  "nonportable": {
    "provider_response_ids": [],
    "runner_thread_ids": [],
    "terminal_ids": [],
    "live_processes": []
  }
}
```

Este envelope no tiene que exponerse completo al usuario, pero debe existir como contrato interno para export/import, debugging y compatibilidad entre versiones.

## Estado portable vs no portable

### Portable

Estos campos deberian preservarse al cambiar de modelo:

- historial de conversacion;
- tool calls completados;
- tool results completados;
- artefactos creados;
- cwd de la sesion;
- roots;
- read dirs permitidos;
- MCP server config;
- system prompt y overrides;
- modo de sesion;
- `compactor_model`;
- policies;
- permisos portables;
- todos;
- summaries;
- Context Twin;
- switch adaptation plans;
- switch safety decisions;
- continuity checkpoint results;
- usage acumulado;
- audit log;
- eventos de cambio de modelo.

### No portable

Estos campos no deberian prometerse como migrables:

- provider response IDs;
- thread IDs internos del proveedor;
- terminal IDs vivos;
- procesos vivos;
- conexiones abiertas;
- pending tool calls a medio ejecutar;
- API keys;
- caches internas del runner;
- memoria no serializada;
- streams en curso.

El producto debe tratarlos como estado recreable o invalidado.

## Tool calls y artefactos

El formato canonico de sesion no debe usar summaries como verdad principal.

Para tool calls:

```json
{
  "id": "toolu_...",
  "parent_tool_use_id": null,
  "name": "bash",
  "input": {
    "cmd": "cargo test",
    "workdir": "/repo"
  },
  "status": "completed",
  "started_at": "...",
  "completed_at": "...",
  "result": {
    "type": "text",
    "content": "...",
    "truncated": false
  }
}
```

Para outputs grandes:

```json
{
  "type": "artifact_ref",
  "artifact_id": "artifact_...",
  "sha256": "...",
  "size_bytes": 1234567,
  "mime": "text/plain",
  "preview": "first N chars...",
  "truncated": true
}
```

Esto mantiene la sesion auditable sin forzar que todo entre en el prompt.

## Flujo correcto de cambio de modelo

1. Usuario pide cambiar a un modelo.
2. Trogonai toma el Session Lease para ese `session_id`.
3. Si la sesion esta ocupada, Trogonai espera, cancela o informa que debe reintentarse.
4. Trogonai resuelve runner/modelo destino.
5. Trogonai consulta capabilities del modelo actual y destino.
6. Trogonai actualiza el Context Twin desde el event log/snapshot canonico.
7. Trogonai crea un `switch_adaptation_plan`.
8. Trogonai ejecuta el Switch Safety Gate.
9. Si el resultado es `blocked_until_safe`, Trogonai no cambia todavia y explica que falta terminar, cancelar o persistir.
10. Si el resultado requiere confirmacion, Trogonai pide aprobacion explicita.
11. Trogonai registra `model_switched`.
12. Trogonai registra `runner_detached` si cambia de runner.
13. Trogonai invalida estado no portable del runner anterior.
14. Trogonai crea o reutiliza un binding runtime para el runner destino.
15. Trogonai registra `runner_attached`.
16. El Prompt Compiler construye una projection usando event log/snapshot, Context Twin y adaptation plan.
17. Si aplica, Trogonai ejecuta un Continuity Checkpoint contra el modelo destino.
18. Si hay mismatch fuerte, Trogonai repara contexto, advierte degradacion o bloquea acciones riesgosas.
19. El runner destino ejecuta el siguiente turno.
20. La respuesta y los tool events se normalizan como eventos canonicos.
21. El kernel materializa nuevos snapshots en KV.
22. Trogonai libera el Session Lease.

El punto clave: no se debe exportar desde un runner e importar en otro como mecanismo de verdad. El runner destino debe hidratarse desde la sesion Trogonai.

## UX recomendada

El usuario no necesita ver todos los detalles tecnicos, pero si necesita senales claras.

Ejemplos:

- `Switched from Claude Sonnet to Grok Code Fast`
- `Using Context Twin plus recent turns`
- `Switch safety: confirmation required`
- `Continuity checkpoint passed`
- `Using summarized context: 183k tokens compressed to 24k`
- `This model does not support image input; image references were preserved but not sent`
- `Terminal state was restarted for this model`
- `3 previous tool results are available as artifacts`

La transparencia evita que el cambio de modelo parezca aleatorio.


## Production Policies

Estas politicas completan la arquitectura para calidad de producto. No todas tienen que estar implementadas en fase 1, pero deben estar definidas para evitar perdida de estado, degradaciones silenciosas y fallos dificiles de recuperar.

### Artifact Store Policy

El artifact store debe separar contenido grande de metadata.

Politica recomendada:

```text
small output -> inline en evento
large output -> object store + artifact_ref
snapshot -> solo metadata y refs
```

Implementacion preferida:

- reutilizar `trogon_nats::jetstream::NatsObjectStore` para artefactos grandes;
- reutilizar claim-check existente para payloads que exceden el limite de NATS;
- NATS KV para metadata y referencias;
- contenido inline solo para payloads pequenos.

Cada artifact debe tener:

```text
artifact_id
session_id
event_id
tool_execution_id
sha256
size_bytes
mime
preview
storage_ref
created_at
retention_policy
permission_scope
encryption_status
```

Reglas:

- definir limite inline, por ejemplo 32-64 KB;
- previews siempre truncados;
- checksums obligatorios;
- artifacts inmutables por hash;
- no borrar artifacts referenciados por eventos no compactados;
- GC por retention/session;
- cifrado cuando haya datos sensibles;
- permisos por sesion/workspace.

#### Politica para imagenes URL y Base64

Si una imagen forma parte del contexto necesario para continuar la sesion, Trogonai debe convertirla en artifact propio. Ni una URL externa ni un string Base64 deben ser la verdad canonica de largo plazo.

Imagen por URL:

```text
fetch -> validate -> hash -> store bytes as artifact
keep source_url as metadata
```

Reglas:

- fetchear la URL con limites de timeout, tamano, redirects y content type;
- validar MIME real y dimensiones cuando aplique;
- calcular `sha256` sobre los bytes obtenidos;
- guardar bytes en Object Store/claim-check como artifact propio;
- preservar `source_url`, `fetched_at`, `mime`, `size_bytes`, `sha256`, `storage_ref` y `availability`;
- si no se puede fetchear por auth, timeout, tamano, politica o red, guardar solo `external_ref` degradado;
- si la imagen es indispensable y solo existe como `external_ref`, Switch Safety Gate debe bloquear o pedir confirmacion explicita.

Imagen Base64:

```text
decode -> validate -> hash -> store bytes as artifact
keep source_encoding/source_declared_mime as metadata
```

Reglas:

- decodificar Base64 a bytes crudos antes de persistir;
- validar MIME real contra MIME declarado, si existe;
- calcular `sha256` sobre los bytes, no sobre el string Base64;
- guardar bytes en Object Store/claim-check como artifact propio;
- preservar `source_encoding=base64`, `declared_mime`, `decoded_mime`, `size_bytes`, `sha256` y `storage_ref`;
- no guardar el string Base64 como artifact canonico salvo como fallback diagnostico explicitamente degradado;
- si Base64 no decodifica o excede limites, bloquear, pedir confirmacion o degradar explicitamente.

Motivo:

- una URL externa puede expirar, cambiar contenido, requerir auth o desaparecer;
- Base64 es un encoding de transporte, no una forma canonica de almacenamiento;
- bytes validados permiten deduplicacion, integridad, previews, permisos, retention y rehidratacion consistente entre modelos/proveedores.

### Schema Versioning and Migrations

Todo formato durable debe estar versionado.

Formatos versionados:

```text
SessionEvent.v1
SessionSnapshot.v1
SessionEnvelope.v1
ContextTwin.v1
CapabilitySchema.v1
ArtifactMetadata.v1
```

Reglas:

- eventos viejos no se mutan;
- snapshots pueden regenerarse desde event log;
- exports declaran `schema_version`;
- readers deben aceptar al menos la version actual y la anterior;
- breaking changes requieren nuevo version tag;
- unknown fields se ignoran o preservan de forma segura;
- migrators materializan snapshots nuevos desde eventos viejos.

La fuente de verdad sigue siendo el event log. Si cambia el formato de snapshot, se regenera.

### Failure Mode Policy

Cada fallo comun debe tener comportamiento definido.

Reglas base:

- si falla adquirir Session Lease, no ejecutar mutacion;
- si falla append event, no ejecutar side effects posteriores;
- si falla materializar KV, no perder sesion: marcar snapshot stale y regenerar luego;
- si falla artifact persistence, no registrar `completed` para un evento que depende de ese artifact;
- si falla runner despues de `runner_attached`, registrar `runner_failed`, invalidar binding y conservar session state;
- si falla Continuity Checkpoint, no ejecutar tool calls riesgosos hasta reparar, confirmar o degradar explicitamente;
- si hay crash mid-tool, usar receipts/outbox y no reejecutar tools no idempotentes automaticamente.

Regla de oro:

> Nunca marcar como completed algo cuyo evento o artifact durable no fue persistido.

Estados utiles de recovery:

```text
pending
completed
failed
requires_reconciliation
stale_snapshot
runner_failed
```

### Capability Registry Freshness

El registry no debe ser solo una lista estatica de modelos. El `trogon-registry` actual sirve como base de descubrimiento y heartbeat, pero hoy `find_by_model` depende de `metadata.models`; todavia no hay un `CapabilitySchema` fuerte por modelo/runner. El plan debe evolucionar ese registry, no reemplazarlo por una pieza desconectada.

Cada capability debe tener metadata de frescura y confianza.

Campos recomendados:

```text
model_id
runner_id
capabilities
compaction_supported
schema_version
source
last_verified_at
ttl
confidence
test_results
```

Reglas:

- capabilities con TTL;
- health checks/probes por runner;
- contract tests para tool use, image input, JSON schema, context limits, streaming y compaction model support;
- degradar a conservative defaults si la capability esta vencida;
- no asumir soporte si no esta verificado;
- registrar `capability_snapshot` usado durante el switch.

Esto permite auditar por que Trogonai decidio enviar, resumir, degradar o bloquear cierta capacidad.

### MCP and Tool Lifecycle

Separar tool config de tool runtime.

Portable:

- MCP server config;
- tool names/schemas;
- permissions/policies;
- approvals portables;
- tool results completados;
- artifact refs.

No portable:

- conexiones abiertas;
- procesos MCP vivos;
- auth ephemeral;
- pending tool calls;
- provider-specific tool IDs;
- caches internas.

Al cambiar modelo:

```text
reconnect MCP servers
reload tool schemas
validate policies
map tools to target model format
mark unavailable tools
```

Si una tool indispensable no esta disponible, Switch Safety Gate debe bloquear o pedir confirmacion.

### Terminal and Process Policy

Terminal y procesos vivos son runtime no portable, pero Trogonai debe preservar continuidad minima.

Preservar:

```text
cwd
terminal_cwd
env allowlisted
last commands summary
running process summary
dirty files
```

No preservar:

```text
terminal_id
PTY process
interactive program state
streams
shell job state
```

Politica:

- si no hay proceso critico, reiniciar terminal y avisar discretamente;
- si hay proceso critico, bloquear switch o pedir confirmacion;
- si hay dirty state no persistido, bloquear hasta guardar artifact/ref o confirmar degradacion;
- si cambia modelo dentro del mismo runner y terminal puede seguir, reutilizar;
- si cambia proveedor/runner, asumir restart salvo soporte explicito.

### Continuity Metrics and Evals

Trogonai debe medir si cambiar de modelo realmente preserva continuidad. Siguiendo ADR 0008, estas metricas, traces y logs deben emitirse con OpenTelemetry por defecto, con atributos estables como `session_id`, `operation_id`, `source_runner`, `target_runner`, `source_model`, `target_model`, `switch_result`, `capability_degradation` y `checkpoint_result`. Secretos, PII y tool payloads sensibles deben redacted antes de entrar en telemetry.

Metricas online:

```text
switch_success_rate
blocked_switch_rate
allowed_with_warning_rate
continuity_checkpoint_pass_rate
continuity_mismatch_rate
context_repair_rate
artifact_missing_rate
capability_degradation_rate
runner_attach_failure_rate
snapshot_stale_rate
lease_contention_rate
switch_latency_p50
switch_latency_p95
post_switch_user_correction_rate
post_switch_tool_failure_rate
```

Evals offline:

```text
load session
switch model
ask next step
compare answer against Context Twin
verify artifacts/tools/decisions are preserved
verify degraded capabilities are reported
```

Estas metricas deben informar el roadmap: si el mismatch rate o correction rate suben, el problema no esta resuelto aunque la arquitectura exista.


## Operational Product Policies

Estas politicas no bloquean el primer milestone, pero cualquier implementacion production-ready debe cumplirlas o tener una decision explicita de degradacion. Cubren los casos donde la calidad del producto se define: cancelaciones, overrides, limites de contexto, seguridad, operacion NATS, crecimiento del log, forks, schemas, SLOs, certificacion de proveedores y UX de errores.

### Cancellation and Abort Semantics

La cancelacion debe ser un flujo de eventos, no una interrupcion silenciosa.

Eventos minimos:

```text
operation_cancel_requested
runner_cancel_requested
runner_cancelled
operation_cancelled
operation_cancel_failed
operation_requires_reconciliation
```

Reglas:

- cancelar antes de tool calls marca la operacion como `cancelled`;
- cancelar mientras una tool corre pide cancel al runner/tool, pero espera receipt;
- si no se sabe si la tool termino, marcar `requires_reconciliation`;
- no liberar Session Lease hasta registrar `cancelled`, `failed` o `requires_reconciliation`;
- no borrar eventos parciales.

### Force Switch and User Override

Forzar un switch debe permitirse solo como accion explicita, auditada y con impacto claro.

Eventos/estados:

```text
force_switch_requested
force_switch_confirmed
force_switch_completed
force_switch_rejected
```

Reglas:

- no permitir force switch si hay mutacion destructiva no reconciliada;
- permitirlo si la perdida es contextual o una degradacion conocida;
- registrar que se pierde, que se invalida y que queda pendiente;
- invalidar el runner binding anterior;
- marcar trabajo pendiente como `requires_reconciliation` cuando aplique.

UX esperada:

```text
Cambiar ahora puede perder el estado del terminal y un tool result pendiente.
Opciones: continuar, esperar, cancelar.
```

### Token Budget, Compaction and Prompt Projection Policy

El Prompt Compiler debe tener un algoritmo deterministico de prioridad cuando el contexto no cabe.

`compactor_model` es configuracion portable de sesion y no debe perderse al cambiar el modelo principal.

Reglas de `compactor_model`:

- `None` significa usar el default, normalmente el modelo principal de la sesion;
- `Some(model_id)` significa que el usuario eligio explicitamente un modelo para compaction;
- cambiar el modelo principal no debe borrar `compactor_model`;
- si el compactor model no esta disponible, Trogonai debe registrar degradacion y usar fallback solo de forma explicita;
- si la degradacion cambia una preferencia explicita del usuario, Switch Safety Gate debe advertir o pedir confirmacion.

Eventos/degradaciones utiles:

```text
compactor_model_preserved
compactor_model_unavailable
fallback_to_default_compactor
```

El Prompt Compiler debe tener un algoritmo deterministico de prioridad cuando el contexto no cabe.

Orden recomendado:

1. system/developer/session rules;
2. safety, permissions y tool policy;
3. Context Twin;
4. switch adaptation plan;
5. continuity/force-switch warnings;
6. current user request;
7. recent turns;
8. active tool schemas necesarias;
9. relevant artifact previews;
10. unresolved errors/tests;
11. long-term summaries;
12. older transcript si todavia cabe.

Reglas:

- nada critico se omite sin metadata de degradacion;
- cada projection registra `projection_id`, token estimate, included blocks y excluded blocks;
- si no caben Context Twin, current request y policies, bloquear o pedir un modelo con mas contexto;
- el ordering de bloques debe ser estable para facilitar debugging.

### Security, Secrets and Sanitized Exports

Trogonai debe tratar artifacts, tool outputs y exports como superficies de seguridad.

Reglas:

- detectar secretos en tool outputs/artifacts antes de persistir o exportar;
- exports sanitizados por defecto;
- raw exports solo con confirmacion explicita;
- no enviar API keys al modelo salvo allowlist explicita;
- artifact access scoped por workspace/session/user;
- registrar `redaction_applied` cuando se modifique contenido;
- PII/secrets nunca deben aparecer en previews si se detectan.

### NATS Operational Policy

La arquitectura debe definir parametros operacionales de NATS/JetStream/KV/Object Store.

Debe especificar:

```text
stream names
subjects
retention
max message size
max bytes
replicas
ack policy
deliver policy
KV bucket history
Object Store bucket
TTL
backpressure behavior
```

Reglas:

- eventos pequenos en JetStream;
- artifacts grandes en Object Store;
- KV solo snapshots/metadata;
- si hay backpressure, bloquear mutaciones nuevas antes de perder eventos;
- si un payload excede max message size, persistir como artifact y emitir ref.

### Event Log Compaction and Retention

El event log es fuente de verdad, pero necesita politica de crecimiento.

Reglas:

- snapshots periodicos;
- archive de eventos antiguos;
- retention por workspace/session policy;
- artifacts referenciados no se borran;
- borrar artifacts solo cuando no haya referencias o la retention lo permita;
- mantener audit trail minimo de eventos criticos.

Eventos utiles:

```text
snapshot_created
events_archived
artifact_gc_marked
artifact_gc_deleted
```

### Fork and Branch Semantics

Fork/branch debe ser parte del modelo de eventos.

Reglas:

- branch crea `child_session_id`;
- child apunta a `parent_session_id`;
- guardar `branched_at_seq`;
- artifacts se comparten por ref/hash;
- nuevos eventos del child empiezan en su propio `seq`;
- parent y child divergen despues del branch;
- snapshot del child puede iniciar desde parent snapshot + delta.

Evento conceptual:

```json
{
  "type": "session_branched",
  "parent_session_id": "sess_parent",
  "child_session_id": "sess_child",
  "branched_at_seq": 128
}
```

### Schema Governance

Los schemas son contratos internos y deben vivir versionados en el repo.

Reglas:

- validacion obligatoria antes de append;
- runner event invalido se rechaza y registra `invalid_event_rejected`;
- migrators con tests;
- golden fixtures por version;
- compatibilidad minima N-1;
- cambios breaking requieren nuevo major schema;
- CI debe validar schemas, fixtures y migrators.

### Continuity SLOs

Las metricas deben tener objetivos.

Objetivos iniciales sugeridos:

```text
artifact_missing_rate = 0
event_duplicate_side_effect_rate = 0
switch_success_rate > 99%
switch_latency_p95 < 5s sin checkpoint
switch_latency_p95 < 20s con checkpoint
continuity_checkpoint_pass_rate > 95%
requires_reconciliation_rate < 1%
runner_attach_failure_rate < 1%
```

Estos valores pueden ajustarse con datos reales, pero el producto debe tener targets explicitos. Los SLOs deben mapearse a instrumentos OpenTelemetry y dashboards/alerts del runtime que opere Trogonai.

### Provider and Tool Certification Matrix

No todos los modelos deben tratarse como igualmente switch-safe.

Matriz minima:

```text
model
runner
text
tool_use
parallel_tools
image_input
json_schema
long_context
streaming
artifact_refs
mcp_tools
switch_from
switch_to
certified_level
last_verified_at
```

Niveles:

```text
experimental
basic
switch-safe
production
```

Switch Safety Gate debe usar esta certificacion para bloquear, advertir o permitir cambios.

### Error UX Policy

Los errores deben mapearse a estados de producto consistentes.

Estados UX:

```text
session_busy
switch_blocked
confirmation_required
capability_missing
checkpoint_failed
artifact_unavailable
runner_failed
snapshot_stale
requires_reconciliation
```

Cada estado debe incluir:

- explicacion corta;
- impacto real;
- accion recomendada;
- opciones disponibles;
- si es seguro continuar.

Esto evita que el usuario vea fallos tecnicos ambiguos cuando lo que necesita es saber si puede seguir trabajando.


## Rust and NATS Implementation Contracts

Trogonai usa Rust y NATS como backend interno. Por eso, las politicas anteriores deben implementarse como contratos tipados de Rust sobre JetStream, KV y Object Store, no como una arquitectura abstracta.

### Durable Contracts and Rust Types

Los formatos durables internos deben seguir ADR 0009: la fuente de verdad del wire/persistence contract debe ser Protocol Buffers cuando Trogonai controla el contrato. Esto aplica a `SessionEvent`, `SessionSnapshot`, `ContextTwin`, `ArtifactMetadata`, `CapabilitySchema`, runner bindings y valores estructurados guardados en KV/Object Store metadata.

Ejemplo conceptual del contrato fuente:

```proto
message SessionEventV1 {
  string event_id = 1;
  string session_id = 2;
  uint64 seq = 3;
  string operation_id = 4;
  string correlation_id = 5;
  optional string causation_id = 6;
  string idempotency_key = 7;
  google.protobuf.Timestamp created_at = 8;
  Actor actor = 9;
  SessionEventPayloadV1 payload = 10;
}
```

Rust debe consumir tipos generados desde `.proto` o wrappers/newtypes validados alrededor de esos tipos. No deben existir dos contratos durables paralelos (`serde` JSON por un lado y protobuf por otro) para el mismo valor.

Reglas:

- guardar `.proto` bajo `proto/` con paquete y namespace versionados;
- preferir protobuf binary para JetStream/KV/Object Store metadata interna;
- usar protobuf JSON solo para diagnostico, UI, export humano o interoperabilidad que lo requiera;
- no usar strings sueltos para estados criticos en el contrato fuente;
- usar enums para switch/cancel/recovery/failure states;
- reservar field numbers/names eliminados;
- mantener fixtures golden por version y tests de compatibilidad N-1;
- convertir tipos generados a domain newtypes cuando hagan falta invariantes fuertes (`SessionId`, `EventId`, `OperationId`, etc.).

### Suggested Rust Module Boundaries

Los nombres exactos pueden variar, pero los limites deberian ser claros:

```text
trogonai-session-contracts -> protobuf-owned session/event/snapshot/capability contracts
trogonai-session-kernel    -> session lease policy, append, materialization, recovery
trogonai-session-projection -> Context Twin, Prompt Compiler, token budgeting
trogonai-artifacts         -> artifact metadata, refs, NatsObjectStore/claim-check integration
trogonai-capabilities      -> model capabilities, probes, certification sobre trogon-registry
trogonai-switching         -> switch state machine, safety gate, checkpoint
```

La regla importante no es el nombre exacto de la crate, sino que los contratos durables no queden dispersos en runners y que los nombres sigan ADR 0002. Evitar nombres vagos como `core` salvo que el boundary sea realmente claro; para contratos propios de este producto/repo, preferir `trogonai-*`. `trogon-transcript` puede inspirar patrones append-only, pero no debe absorber el Session Kernel tal cual: su modelo actual es audit trail de actores y no contiene `seq`, `idempotency_key`, snapshots, runner bindings, leases ni prompt projection.

### NATS Mapping

Mapeo recomendado:

```text
JetStream stream  -> ACP_SESSION_EVENTS o <PREFIX>_SESSION_EVENTS
NATS KV           -> ACP_SESSION_SNAPSHOTS
NATS KV           -> ACP_CONTEXT_TWINS
NATS KV           -> ACP_RUNNER_BINDINGS
NATS KV           -> ACP_SESSION_LEASES, respaldado por trogon_nats::lease
NATS KV           -> ACP_SESSION_USAGE
Object Store      -> ACP_SESSION_ARTIFACTS, via NatsObjectStore/claim-check
Registry KV       -> AGENT_REGISTRY + typed capability schema extension
NATS subjects     -> runner request/reply and streaming
```

Reglas:

- JetStream guarda eventos pequenos y durables;
- KV guarda snapshots/materialized state/metadata;
- Object Store guarda contenido grande;
- subjects NATS coordinan runners, pero no son fuente de verdad;
- runners no deben persistir la sesion canonica por su cuenta.

### Config Defaults in Rust

La configuracion debe seguir ADR 0007. Los defaults viven en Rust, pero la resolucion efectiva debe respetar esta precedencia:

```text
1. built-in defaults
2. TOML config file
3. environment variables
4. CLI arguments
```

Feature flags, limites, TTLs, retention, SLOs y rollout deben tener un nombre canonico tipado en config Rust. El TOML es el formato humano primario; los secretos no deben vivir en config files.

Ejemplo conceptual:

```rust
pub struct SessionKernelConfig {
    pub inline_artifact_limit_bytes: usize,
    pub max_event_payload_bytes: usize,
    pub max_snapshot_bytes: usize,
    pub lease_ttl: Duration,
    pub lease_renew_interval: Duration,
    pub checkpoint_latency_budget: Duration,
    pub switch_latency_budget: Duration,
}
```

Valores iniciales sugeridos:

```text
inline_artifact_limit = 32-64 KB
max_event_payload = 256 KB
max_snapshot = 2-5 MB
lease_ttl = 30s
lease_renew_interval = 10s
checkpoint_latency_budget = 20s
switch_latency_budget = 5s without checkpoint
```

### Prompt Compiler Trait

El Prompt Compiler debe ser deterministico y testeable.

```rust
pub trait PromptCompiler {
    fn compile(&self, input: ProjectionInput) -> Result<PromptProjection>;
}
```

`PromptProjection` debe incluir:

```text
projection_id
session_id
model_id
token_estimate
included_blocks
excluded_blocks
degradation_metadata
capability_snapshot
created_at
```

Esto permite reproducir por que un modelo recibio cierto contexto y que quedo fuera.

### State Machines as Rust Enums

Los flujos criticos deben modelarse como state machines explicitas.

Ejemplo conceptual:

```rust
pub enum SwitchState {
    Requested,
    LeaseAcquired,
    AdaptationPlanned,
    SafetyEvaluated,
    ModelSwitched,
    RunnerDetached,
    RunnerAttached,
    ProjectionCompiled,
    CheckpointPassed,
    Completed,
    Failed,
    RequiresReconciliation,
}
```

Debe existir algo equivalente para:

- cancel;
- force switch;
- runner attach/detach;
- tool execution;
- continuity checkpoint;
- artifact persistence.

### Migration from Current State

No debe hacerse una migracion big bang desde `state.messages` y `session/export`.

Plan recomendado:

1. **Shadow mode**
   - seguir usando `state.messages` como fuente operacional;
   - emitir eventos en paralelo;
   - materializar snapshots desde eventos;
   - comparar snapshot materializado contra estado actual.

2. **Dual-read / event-primary for new sessions**
   - sesiones nuevas usan event log como primary;
   - sesiones viejas se leen con adapter desde `state.messages`.

3. **On-demand migration**
   - migrar sesiones viejas cuando se abren o se modifican;
   - mantener fallback `session/export` como compatibilidad.

4. **Remove fallback**
   - solo despues de metricas estables y baja tasa de reconciliation.

### Feature Flags

El rollout debe estar protegido por feature flags.

```text
session_kernel_enabled
event_log_shadow_mode
prompt_projection_enabled
switch_safety_gate_enabled
continuity_checkpoint_enabled
artifact_store_enabled
runner_binding_mode
```

Reglas:

- si `event_log_shadow_mode` falla, no romper el flujo actual;
- si el event log es primary y falla append, no mutar;
- si projection falla, bloquear o usar fallback con warning explicito;
- si safety gate no esta disponible, bloquear switches riesgosos.

### Rust Testing Strategy

Tests minimos:

- unit tests para schemas y validation;
- property tests para idempotency/dedup cuando aplique;
- golden fixtures por schema version;
- tests del Prompt Compiler con snapshots fijos;
- mock NATS para leases, KV y event append;
- integration tests con NATS real/JetStream/KV/Object Store;
- crash/retry tests para tool receipts;
- migration tests desde `state.messages`;
- provider certification tests por runner/model/capability.

### Design Rule

Todo estado durable y todo contrato interno entre componentes debe ser:

```text
source-of-truth en protobuf cuando Trogonai controla el contrato
generado o convertido a tipos Rust validados
versionado
validado antes de persistir
compatible con evolucion de schema
reconstruible desde NATS/JetStream/KV/Object Store
testeado con fixtures, migrators y NATS integration tests
```

Esa regla mantiene la propuesta alineada con la arquitectura real de Trogonai.


## MVP vs Production

El documento describe la arquitectura de largo plazo. No todo debe implementarse antes del primer milestone.

### MVP recomendado

El primer milestone debe enfocarse en continuidad basica sin perdida silenciosa:

- mantener `session_id` estable de Trogonai;
- preservar configuracion portable, incluido `compactor_model`;
- dejar de truncar tool input/output como verdad canonica;
- agregar Session Lease para prompt/switch;
- agregar Event Contract basico con `event_id`, `seq`, `operation_id` e `idempotency_key`;
- guardar snapshots canonicos en NATS KV;
- agregar Context Twin basico;
- implementar Capability Negotiation minima;
- implementar Switch Safety Gate para casos obviamente inseguros;
- mantener fallback al handoff actual cuando el kernel este en shadow mode;
- agregar tests de switch entre runners principales.

### Production-ready

Para production-ready se requieren las politicas completas:

- event log como fuente primaria;
- artifact store con Object Store/refs/checksums;
- outbox/receipts para tools;
- Prompt Compiler deterministico con included/excluded blocks;
- Continuity Checkpoint para switches de alto riesgo;
- schema governance y migrators;
- NATS operational policy;
- MCP/tool lifecycle formal;
- terminal/process policy;
- SLOs y provider certification matrix;
- rollout con feature flags y shadow mode.

La regla de rollout:

> El MVP puede ser incremental, pero no debe crear una semantica falsa de "migracion completa" si todavia esta haciendo handoff conversacional.

## Non-goals

Esta arquitectura no intenta prometer cosas que no son portables entre modelos/proveedores.

No objetivos:

- no hacer que todos los modelos se comporten igual;
- no migrar hidden reasoning del proveedor;
- no migrar provider response IDs;
- no migrar thread IDs internos;
- no migrar procesos vivos ni PTYs;
- no preservar streams en curso;
- no reejecutar automaticamente tools no idempotentes;
- no ocultar degradaciones de capability;
- no reemplazar el servicio `trogon-compactor`;
- no eliminar la necesidad de tests por proveedor/modelo.

El objetivo correcto es preservar todo lo que Trogonai puede poseer, serializar, auditar y reinyectar, y bloquear o degradar explicitamente lo que no sea portable.

## End-to-End Example

Ejemplo: el usuario esta trabajando en una sesion con OpenRouter/Claude y tiene `compactor_model = xai/grok-code-fast`. La sesion contiene tool calls completadas, artifact refs y un Context Twin actualizado. El usuario cambia el modelo principal a xAI/Grok.

Flujo esperado:

1. Usuario ejecuta cambio de modelo a `xai/grok-code-fast`.
2. Trogonai toma el Session Lease de `session_id`.
3. Trogonai lee snapshot KV y eventos recientes desde JetStream.
4. Trogonai preserva `compactor_model` porque es configuracion portable de sesion.
5. Trogonai actualiza Context Twin con objetivo actual, plan, archivos relevantes, errores abiertos y proximos pasos.
6. Capability Negotiation compara Claude/OpenRouter vs Grok/xAI.
7. Se crea `switch_adaptation_plan` con degradaciones si aplica.
8. Switch Safety Gate verifica que no hay tool calls en progreso, artifacts sin persistir, streams incompletos o terminal critico.
9. Trogonai registra `model_switched`.
10. Si cambia de runner, Trogonai registra `runner_detached` para OpenRouter e invalida estado no portable del runner anterior.
11. Trogonai crea o reutiliza el runner binding de xAI.
12. Trogonai registra `runner_attached`.
13. Prompt Compiler construye una projection para Grok usando Context Twin, ultimos turnos, policies, tool history relevante y artifact previews.
14. Tool calls completadas se preservan en el transcript canonico; si Grok no puede consumirlas estructuradas, se textualizan en la projection.
15. Outputs grandes se pasan como previews + artifact refs, no como blobs inline.
16. Si el switch es de alto riesgo, se ejecuta Continuity Checkpoint.
17. Si el checkpoint pasa, el usuario continua con Grok en la misma sesion visible.
18. El kernel materializa snapshot nuevo en KV con el runner binding actualizado.
19. Trogonai libera el Session Lease.

Resultado esperado para el usuario:

```text
Switched from OpenRouter/Claude to xAI/Grok.
Using Context Twin plus recent turns.
compactor_model preserved: xai/grok-code-fast.
3 previous tool results are available as artifacts.
```

Lo importante: el cambio no se implementa copiando una sesion de runner a otra. Se implementa rehidratando el runner destino desde la sesion canonica de Trogonai.

## Roadmap pragmatico

### Fase 1: corregir semantica y evitar perdida innecesaria

- Agregar Session Lease por `session_id` para prompt, switch, compact, import, fork y tool mutations.
- Mantener un `session_id` estable de Trogonai al cambiar de modelo.
- Registrar eventos `model_switched`.
- Renombrar internamente el mecanismo actual como handoff conversacional si sigue usando export/import.
- Separar `canonical transcript` de `prompt projection`.
- Dejar de truncar tool input/output en el formato canonico.
- Marcar explicitamente estado no portable.
- Agregar Context Twin basico con objetivo, plan, archivos relevantes y proximos pasos.
- Agregar tests de cambio de modelo entre runners principales.

### Fase 2: snapshots canonicos en NATS KV

- Introducir `SessionEnvelopeV1`.
- Incluir configuracion portable, incluyendo `compactor_model`.
- Incluir tool calls completos.
- Incluir artifact refs.
- Migrar summaries y todos al store canonico.
- Implementar capability checks por modelo.
- Agregar fallback por modelos sin tools, sin imagenes o con contexto pequeno.
- Guardar snapshots/materialized state en NATS KV.
- Guardar `context_twin`, `switch_adaptation_plan`, `switch_safety` y resultados de continuity checkpoint en KV.

### Fase 3: event log en JetStream

- Introducir `SessionEvent`.
- Publicar eventos append-only por `sessions.{session_id}.events`.
- Agregar eventos `context_twin_updated`, `switch_adaptation_plan_created`, `switch_safety_evaluated`, `continuity_checkpoint_started` y `continuity_checkpoint_completed`.
- Agregar Event Contract con `event_id`, `seq`, `operation_id`, `causation_id`, `correlation_id` e `idempotency_key`.
- Agregar tests de lease expiration, renew y reintento desde event log.
- Agregar tests de deduplicacion de eventos y tool receipts.
- Materializar snapshots desde eventos.
- Agregar idempotency keys/event IDs.
- Agregar migracion desde sesiones antiguas basadas en `state.messages`.
- Agregar tests de replay despues de crash.

### Fase 4: runners como bindings/adapters

- Mover ownership de sesion fuera de los runners.
- Hacer que cada runner reciba una projection y devuelva canonical events.
- Reducir el estado persistente propio de cada runner a caches o bindings runtime.
- Unificar import/export alrededor del envelope, no alrededor de mensajes sueltos.
- Hacer que el switch cambie el binding activo en vez de copiar sesiones.

### Fase 5: calidad avanzada

- Replay/debug tooling.
- Diff de prompt projection por modelo.
- Auditoria completa de tool calls.
- Compaction incremental.
- Artifact browser.
- Resume robusto despues de crash.
- Fork/branch real de sesiones.
- Inspector de eventos de sesion.
- Tests de degradacion por capabilities.
- Evaluaciones de continuidad usando Context Twin vs historial solo.
- Switch Safety Gate para bloquear switches inseguros.
- Continuity Checkpoint para switches de alto riesgo.
- Self-healing de contexto cuando el checkpoint detecta mismatch.


## Implementation Plan

Esta seccion convierte la arquitectura en una secuencia de entrega. No reemplaza el roadmap; lo baja a PRs pequenos, feature flags y criterios de salida.

### Current Implementation vs Target Architecture

| Actual | Target |
| --- | --- |
| `session/export` / `session/import` como mecanismo de handoff | Session Kernel como fuente canonica y runner projection como output |
| Runner-owned session state | Trogonai-owned canonical session state |
| Portable session V2 con summaries/truncamiento | Canonical transcript no-lossy + PromptProjection derivada |
| `active_sessions` map como routing runtime | Runner binding snapshot en KV + eventos `runner_attached`/`runner_detached` |
| `state.messages` como historial principal | Event log + snapshots materializados |
| Tool IO resumido/truncado para portabilidad | Tool calls/results completos o artifact refs |
| Switch best-effort entre runners | Switch bajo Session Lease + Safety Gate + capability negotiation |
| Config portable implicita | `SessionEnvelope.config` explicito, incluyendo `compactor_model` |
| Runner import interpreta historia a su manera | Prompt Compiler adapta contexto por capabilities |

### Decisiones cerradas para implementar el switch canonico

Estas decisiones son parte del plan de implementacion. No son preferencias opcionales: definen cuando el cambio de modelo cumple el objetivo de continuar la misma sesion al cambiar a cualquier modelo soportado por los runners existentes.

#### Decisiones arquitectonicas

1. **Session Kernel es la fuente de verdad**
   - La sesion canonica pertenece a Trogonai.
   - El estado durable debe vivir en event log, snapshots, Context Twin, artifact refs y metadata canonica.
   - Los runners pueden mantener caches o sesiones internas, pero no pueden ser la fuente semantica de continuidad.
   - Criterio de implementacion: despues de un switch, el `session_id` visible de Trogonai no cambia y el estado reconstruible sale del Session Kernel, no del runner anterior.

2. **`session/export` / `session/import` es compatibilidad temporal**
   - Export/import puede seguir existiendo para shadow mode, migracion y fallback de sesiones legacy.
   - No cuenta como cumplimiento final del switch canonico.
   - Criterio de implementacion: mientras `event_log_primary` este desactivado, export/import puede ser fallback con warning; cuando `event_log_primary` este activo para una sesion, un fallo de runner binding canonico debe producir error reparable o rollback controlado, no handoff silencioso.

3. **Identidad visible estable**
   - El usuario sigue viendo el mismo `session_id` de Trogonai.
   - El runner puede crear, reemplazar o descartar un runtime binding interno.
   - Criterio de implementacion: UI/CLI/API no deben exponer un nuevo `session_id` como si fuera una sesion nueva salvo que el usuario pida fork/branch.

4. **Modelo destino permitido solo si existe runner y capability contract**
   - El target puede ser cualquier modelo de cualquier proveedor soportado por los runners existentes.
   - "Soportado" significa: runner resoluble, credenciales/configuracion disponibles, capabilities conocidas y adaptacion posible para el estado actual.
   - Criterio de implementacion: si falta runner, credencial, capability indispensable o certificacion requerida, el switch debe bloquearse o pedir confirmacion explicita segun politica.

5. **Capability Negotiation es obligatoria antes del attach**
   - El sistema debe comparar capacidades del modelo actual y destino antes de hidratar el runner destino.
   - Debe producir un adaptation plan durable con degradaciones visibles.
   - Criterio de implementacion: ningun switch canonico puede registrar `runner_attached` sin un `switch_adaptation_plan_created` asociado.

6. **Switch Safety Gate protege el momento del cambio**
   - Capability Negotiation decide si el destino puede representar el contexto.
   - Safety Gate decide si es seguro cambiar ahora.
   - Criterio de implementacion: tools en progreso, artifacts no persistidos, terminal/proceso indispensable vivo, stream incompleto o capability indispensable ausente deben producir `block` o `requires_confirmation`.

7. **Continuity Checkpoint real para switches de riesgo**
   - `internal_echo` solo es valido para tests, shadow mode o MVP explicitamente marcado.
   - En production/high-risk, el checkpoint debe ejecutarse contra el modelo/runner destino o bloquear acciones riesgosas posteriores hasta reparar contexto.
   - Criterio de implementacion: un switch de alto riesgo no puede considerarse completo sin `continuity_checkpoint_completed` con pass, repaired o degraded acknowledged.

8. **Artifacts y tool outputs no se pierden**
   - Outputs grandes, imagenes, logs y resultados no portables deben persistirse como artifact refs antes del switch.
   - Criterio de implementacion: si un dato necesario para continuar existe solo inline/truncado y excede limites, Safety Gate debe bloquear hasta persistirlo o marcar degradacion confirmada.

9. **Terminales, procesos vivos y estado provider-specific no son portables**
   - No se deben fingir migraciones de PTYs, procesos vivos, provider response IDs, hidden reasoning ni thread IDs internos.
   - Criterio de implementacion: el switch preserva cwd, comandos relevantes, outputs persistidos y estado resumible; invalida lo vivo/no portable con degradacion visible.

10. **MCP/tools se recalculan por runner/modelo**
    - La configuracion portable de MCP/tools y permisos viaja en la sesion canonica.
    - La disponibilidad efectiva se recalcula para el runner/modelo destino.
    - Criterio de implementacion: si una tool indispensable no esta disponible despues del cambio, Safety Gate bloquea o pide confirmacion.

11. **Session Kernel owns contracts**
    - Schemas, migraciones, compatibility fixtures y capability registry pertenecen al Session Kernel/Core.
    - Los runners adaptan/proyectan; no definen contratos canonicos divergentes.
    - Criterio de implementacion: todo evento/snapshot/projection usado por mas de un runner debe vivir en contratos versionados.

#### Politicas configurables

Estas decisiones no deben hardcodearse como verdades permanentes. Deben vivir en config/feature flags con defaults conservadores y telemetria.

- **Umbral de divergencia shadow**: porcentaje/maximo de sesiones donde snapshot, replay y estado legacy pueden diferir antes de habilitar event-primary.
- **Block vs confirmation**: matriz por riesgo para decidir si una degradacion bloquea o requiere confirmacion del usuario.
- **Fallback budget**: numero o tasa maxima de fallbacks a handoff antes de considerar el rollout no saludable.
- **Certification level por modelo/proveedor**: modelos con tools, artifacts o acciones destructivas deben requerir certificacion mas fuerte que modelos de texto simple.
- **Checkpoint strictness**: switches simples pueden usar checkpoint ligero; switches con tools, artifacts, terminal o cambios de proveedor deben usar checkpoint real contra destino.
- **Artifact inline limit**: limite de bytes para inline payloads antes de exigir Object Store/claim-check.
- **Retention/TTL/SLOs**: duracion de eventos, snapshots, artifacts, leases y retry windows.

Cada politica debe tener:

- default documentado;
- ubicacion de config (ADR-0007: nombre canonico tipado con precedencia defaults < TOML < env `TROGON_*` < CLI, validacion despues del merge, secretos fuera de config); ningun valor declarado configurable aqui puede quedar hardcodeado;
- evento o metrica asociada (ADR-0008: OpenTelemetry, instrument con nombre semantico y `service.name`; el binario host debe inicializar el provider OTel para que la metrica realmente se exporte, no solo se registre en un meter no-op);
- comportamiento de rollback;
- criterio para cambiar el default.

#### Conformidad con ADRs de plataforma (PR #207)

Este diseno no inventa convenciones donde ya existe una decision de plataforma: adopta los ADRs del repositorio. La conformidad es parte del criterio de "listo", no opcional.

- **ADR-0009 (Protocol Buffers):** todos los contratos canonicos durables y de persistencia schemaless (event log, snapshots KV, artifact metadata, context twin, capability schema, runner binding, prompt projection, `SwitchResult` y el resultado visible) son `.proto` versionados bajo `proto/` con encoding binario en paths internos; los tipos generados viven en un crate con sufijo `-proto` y se convierten a value-objects de dominio en el borde. JSON solo en bordes exentos (export/import legacy, diagnostico), nunca como verdad canonica.
- **ADR-0002 / ADR-0001 (crate boundaries / taxonomy):** los crates del kernel usan prefijo `trogonai-*` (especificos de producto) y sufijo `-proto` para tipos generados. Los crates de dominio no dependen de apps, servicios, runners ni CLIs; los adapters dependen del dominio, no al reves. Un Service (lifecycle propio, identidad OTel) vive en `services/`, no como cualquier binario.
- **ADR-0003 (transport taxonomy):** NATS/JetStream es el backbone interno, no el transporte publico por defecto; no se introduce gRPC/REST/HTTP entre componentes internos. Si el resultado del switch se expone como service API publica, debe justificarse por ADR-0003 y usar contrato protobuf por ADR-0009. El nombre del protocolo (ACP/MCP) se mantiene separado del nombre del transporte (NATS/stdio/HTTP/WebSocket).
- **ADR-0007 (configuration sources):** flags de rollout, thresholds, budgets, TTLs, strictness y perfiles siguen la precedencia y el naming de ADR-0007; TOML es la fuente primaria human-editable.
- **ADR-0008 (OpenTelemetry):** metricas, trazas y logs por OpenTelemetry (nombres semanticos, `service.name`, export OTLP via collector), setup compartido tipo `trogon-telemetry`, observabilidad bajo modulos `telemetry/`; el host que ejecuta el kernel debe inicializar el provider.

#### Contrato formal de resultado del switch

El switch debe devolver y persistir un resultado normalizado. No debe depender de strings libres en logs o mensajes de UI.

Resultado minimo:

```text
switched
blocked
requires_confirmation
degraded
repaired
rolled_back
failed_recoverable
failed_terminal
```

Semantica esperada:

- `switched`: el modelo destino quedo activo sin degradaciones relevantes.
- `blocked`: el switch no se ejecuto porque Safety Gate encontro una condicion no segura.
- `requires_confirmation`: el switch puede ejecutarse, pero requiere confirmacion explicita por perdida/degradacion.
- `degraded`: el switch se ejecuto con degradaciones visibles y registradas.
- `repaired`: el checkpoint detecto mismatch y Trogonai reparo la proyeccion antes de continuar.
- `rolled_back`: Trogonai intento cambiar, fallo una etapa posterior y restauro el binding anterior.
- `failed_recoverable`: el switch fallo, pero la sesion canonica sigue consistente y se puede reintentar.
- `failed_terminal`: el switch fallo por una condicion que requiere intervencion o cambio de configuracion antes de reintentar.

Criterio de implementacion:

- cada resultado debe estar representado en tipos/proto;
- cada resultado debe tener evento canonico o campo durable asociado;
- cada resultado debe mapear a metricas y al contrato de resultado visible del switch;
- los tests deben validar que Safety Gate, runner binding, checkpoint y rollback producen resultados consistentes.

#### Contrato de resultado visible del switch

Este contrato reemplaza la frase "UX/API" cuando pueda ser ambigua. Aqui, "API" significa cualquier contrato que expone el resultado del switch: CLI, TUI, evento NATS, struct Rust, protocolo interno o API futura. No implica necesariamente REST/HTTP. Si el resultado se expone como service API publica, debe seguir ADR 0003 y ADR 0009.

Toda superficie que exponga cambio de modelo debe mostrar o devolver suficiente informacion para no crear continuidad falsa.

Respuesta minima:

```json
{
  "session_id": "sess_...",
  "result": "switched",
  "from_model": "openrouter/claude-sonnet",
  "to_model": "xai/grok-code-fast",
  "from_runner": "openrouter",
  "to_runner": "xai",
  "runner_changed": true,
  "degradations": [],
  "lost_capabilities": [],
  "fallback_used": false,
  "fallback_reason": null,
  "checkpoint": {
    "required": true,
    "status": "passed"
  },
  "next_action": null
}
```

Reglas del contrato visible:

- si `fallback_used=true`, el usuario debe ver que fue handoff/fallback y no switch canonico completo;
- si `result=blocked`, la respuesta debe incluir `next_action`, por ejemplo esperar tool, persistir artifact, cancelar proceso o elegir otro modelo;
- si `result=requires_confirmation`, la respuesta debe listar degradaciones y capabilities faltantes;
- si `result=degraded`, la degradacion debe quedar en snapshot/event log y ser visible en CLI/UI;
- si `result=failed_recoverable` o `rolled_back`, el `session_id` visible debe seguir estable y el estado canonico debe quedar consistente;
- si `result=failed_terminal`, la respuesta debe indicar la causa concreta: runner inexistente, credencial faltante, capability indispensable ausente, schema incompatible o artifact irreparable.

Criterio de implementacion:

- el contrato debe existir antes de activar `runner_binding_mode=canonical`;
- CLI, TUI, eventos NATS, structs Rust y cualquier API futura deben consumir el mismo tipo de resultado;
- logs y metricas deben derivarse del resultado estructurado, no de parsear texto;
- golden tests deben cubrir al menos `switched`, `blocked`, `requires_confirmation`, `degraded`, `rolled_back` y `failed_recoverable`.

#### Compatibilidad temporal permitida

Estas excepciones son aceptables solo durante MVP/shadow/rollout:

- legacy store sigue siendo operational source mientras `event_log_primary=false`;
- export/import puede hidratar runners mientras `runner_binding_mode=handoff` o `shadow`;
- `internal_echo` puede simular Continuity Checkpoint en tests y shadow mode;
- runner-owned sessions pueden existir como bindings internos;
- handoff fallback puede usarse para sesiones legacy con warning explicito.

Estas excepciones deben eliminarse o quedar desactivadas por defecto antes de declarar el switch canonico como production-ready.

### Implementation Phases

| Fase | Objetivo | Cambios de codigo | Feature flag | Tests requeridos | Criterio de salida |
| --- | --- | --- | --- | --- | --- |
| 1 | Contratos durables y tipos Rust | Crear `.proto` versionados, generar tipos Rust, agregar wrappers/newtypes y fixtures | `session_kernel_enabled=false` | proto compatibility/golden fixtures | contratos versionados generan Rust y mantienen compatibilidad N-1 |
| 2 | Session Lease por `session_id` | Reutilizar `trogon_nats::lease` y agregar politica session-scoped en prompt/switch | `session_lease_enabled` | lease acquire/renew/expire | no hay dos mutaciones simultaneas por sesion |
| 3 | Canonical snapshot en KV | Escribir/leer `SessionSnapshotV1` paralelo al estado actual | `canonical_snapshot_enabled` | load/save/migration tests | snapshot preserva config portable y `compactor_model` |
| 4 | Shadow event log | Emitir `SessionEventV1` en JetStream sin usarlo aun como primary | `event_log_shadow_mode` | compare snapshot vs `state.messages` | eventos se emiten sin romper flujo actual |
| 5 | Canonical tool IO/artifact refs | Persistir tool input/output completo o refs usando `NatsObjectStore` + claim-check | `artifact_store_enabled` | tool result/artifact tests | tool input/output no se trunca como verdad canonica |
| 6 | PromptProjection basica | Agregar `PromptCompiler` determinista con included/excluded blocks | `prompt_projection_enabled` | deterministic projection tests | included/excluded blocks son reproducibles |
| 7 | Switch Safety Gate minimo | Validar estado operativo, pending tools, artifacts y capabilities antes del switch | `switch_safety_gate_enabled` | busy/tool-running/capability tests | switches inseguros se bloquean o advierten |
| 8 | Runner binding desde sesion canonica | Guardar binding en KV y adjuntar runner desde snapshot/projection | `runner_binding_mode=canonical` | cross-runner integration tests | switch no depende de copiar runner session state |
| 9 | Continuity Checkpoint alto riesgo | Ejecutar checkpoint y registrar pass/fail/degradation events | `continuity_checkpoint_enabled` | checkpoint pass/fail tests | high-risk switches validan continuidad o degradan |
| 10 | Politicas operativas, resultado y certificacion | Configurar block/confirm, divergence thresholds, fallback budget, checkpoint strictness, `SwitchResult`, contrato visible de resultado y matriz inicial de modelos | `switch_policy_profile` | policy matrix/result contract/certification tests | decisiones configurables tienen defaults, metrics, contrato visible y rollback |
| 11 | Event log primary para sesiones nuevas | Usar replay + snapshots como fuente de verdad para sesiones opt-in | `event_log_primary_new_sessions` | replay/recovery tests | sesiones nuevas reconstruyen desde event log |

### First PRs

1. **PR 1: Session contracts crate/module**
   - Agregar `.proto` para IDs, `SessionEventV1`, `SessionSnapshotV1`, `ArtifactMetadataV1`, `ContextTwinV1` y `CapabilitySchemaV1`.
   - Generar tipos Rust y agregar wrappers/newtypes validados donde hagan falta invariantes.
   - Agregar fixtures protobuf binary y, si hace falta, protobuf JSON diagnostico.
   - Sin cambiar runtime behavior.

2. **PR 2: Preserve portable config explicitly**
   - Asegurar que `compactor_model` viaja en canonical snapshot/config.
   - Agregar tests de switch/model config preservation.

3. **PR 3: Session Lease**
   - Reutilizar `trogon_nats::lease` para acquire/renew/release sobre NATS KV.
   - Agregar wrapper/politica `session_id` en el Session Kernel.
   - Integrar primero en prompt/switch paths.
   - Agregar tests de contention y expiry.

4. **PR 4: Canonical snapshot writer**
   - Escribir snapshot KV paralelo al estado actual.
   - Mantener `state.messages` como operational source.
   - Comparar y loggear divergencias.

5. **PR 5: Shadow event log**
   - Emitir eventos canonicos en JetStream en paralelo.
   - No usar todavia como primary.
   - Agregar replay test basico.

6. **PR 6: Canonical tool calls/results**
   - Guardar tool IO completo o artifact refs.
   - Evitar truncamiento en canonical truth.
   - Mantener truncamiento solo en prompt projection.

7. **PR 7: PromptProjection v1**
   - Compilar Context Twin + recent turns + policies + artifact previews.
   - Registrar included/excluded blocks.
   - Tests deterministas.

8. **PR 8: Switch Safety Gate v1**
   - Bloquear switch si hay operation in progress, tool pending, artifact missing o capability critical missing.
   - Agregar UX/error states minimos.

9. **PR 9: Canonical runner binding**
   - Guardar binding en KV.
   - Adjuntar runner destino desde snapshot/projection.
   - Mantener fallback a handoff actual solo detras de feature flag y con warning explicito.
   - Garantizar que el `session_id` visible de Trogonai no cambia.

10. **PR 10: Switch policy and certification matrix**
   - Agregar config para block vs confirmation, fallback budget, checkpoint strictness y artifact inline limit.
   - Definir `SwitchResult` en tipos/proto y mapearlo a eventos, metricas, CLI y API.
   - Agregar contrato de resultado visible para exponer degradaciones, capabilities perdidas, fallback y checkpoint.
   - Definir matriz inicial de runners/modelos certificados.
   - Registrar metrics para shadow divergence, canonical attach success, handoff fallback rate y checkpoint result.
   - Documentar defaults y rollback.

11. **PR 11: Event-primary new sessions**
   - Activar event log como primary solo para sesiones nuevas y opt-in.
   - Mantener adapter para sesiones viejas.
   - Desactivar handoff silencioso para sesiones kernel-primary.

### PR Dependency Order

El orden de implementacion importa. Estos PRs no deben tratarse como una lista paralela:

- PR 1 bloquea PR 4, PR 5, PR 6, PR 7, PR 8, PR 9, PR 10 y PR 11, porque define los contratos canonicos que todos consumen.
- PR 2 debe completarse antes de validar switches cross-runner, porque `compactor_model` y la configuracion portable forman parte del estado de continuidad.
- PR 3 debe completarse antes de activar PR 8, PR 9 o PR 11, porque prompt, switch, cancelacion y attach/detach necesitan serializacion por `session_id`.
- PR 4 debe existir antes de PR 5 en modo util, porque el snapshot permite comparar materializacion contra replay del event log.
- PR 5 debe existir antes de PR 11, porque event-primary no debe activarse sin haber corrido event log en shadow mode.
- PR 6 debe completarse antes de PR 7 y PR 9, porque PromptProjection y runner binding no deben depender de tool IO truncado.
- PR 7 debe completarse antes de PR 9, porque el runner destino debe recibir una proyeccion canonica y no una importacion best-effort.
- PR 8 debe completarse antes de PR 9, porque el attach del runner destino debe estar protegido por Safety Gate.
- PR 9 debe completarse antes de PR 11, porque event-primary necesita bindings canonicos para no volver al ownership del runner.
- PR 10 debe completarse antes de PR 11, porque event-primary necesita politicas operativas cerradas para bloquear, confirmar, degradar y hacer rollback de forma consistente.

### No-Lossy Contract

`No-lossy` significa que la verdad canonica de la sesion no se resume ni se trunca. La reduccion de contexto solo puede ocurrir en proyecciones derivadas para prompts, UI o exports sanitizados.

Campos que nunca deben truncarse como verdad canonica:

- texto de usuario y asistente;
- tool call name, input JSON completo, output completo, error completo y status;
- `tool_call_id`, `tool_execution_id`, `operation_id`, `event_id`, `seq`, `causation_id`, `correlation_id` e `idempotency_key`;
- configuracion portable de sesion, incluyendo `model`, `runner`, `compactor_model`, roots, permisos, MCP/tool configuration portable y policy settings;
- metadata de artifacts: checksum, byte size, content type, encryption state, owner/workspace scope y retention;
- estado reconciliable de operaciones: pending, cancelled, failed, completed, requires_reconciliation.

Contenido grande puede salir del evento o snapshot inline, pero no puede perderse. Debe moverse a artifact refs con checksum, content length y permisos. Los summaries, previews o truncamientos se permiten solamente en:

- PromptProjection;
- UI compacta;
- logs no canonicos;
- exports explicitamente sanitizados;
- continuity reports explicativos.

La regla de implementacion es simple: si un dato seria necesario para rearmar, auditar o continuar la sesion despues de cambiar de modelo, no puede vivir solo en un summary.

### Complex Session Fixture

Antes de activar switching canonico por defecto debe existir al menos un fixture end-to-end que represente una sesion realista. Ese fixture debe poder ejecutarse contra snapshot, event log replay y runner projection.

El fixture minimo debe incluir:

- varios turnos de usuario/asistente;
- una tool call idempotente exitosa;
- una tool call no idempotente con receipt;
- un tool result grande guardado como artifact ref;
- un artifact missing simulado;
- un cambio de modelo entre proveedores;
- `compactor_model` configurado y preservado;
- una compaction;
- un retry que no duplica tool calls;
- una cancelacion;
- un fork de sesion;
- un runner attach/detach;
- una capability faltante que produce degradation explicita.

El test pasa solo si:

- replay del event log reconstruye el mismo snapshot canonico;
- PromptProjection es determinista;
- no se pierde tool IO canonico;
- los artifacts referenciados existen o producen estado `artifact_unavailable`;
- `compactor_model` sigue intacto despues del switch;
- los retries no duplican eventos ni efectos externos;
- el switch genera Safety Gate result y, cuando aplica, Continuity Checkpoint.

### Estado de decisiones de implementacion

Esta tabla reemplaza la lista generica de decisiones abiertas. Algunas decisiones ya estan cerradas por la implementacion actual; otras son correctas solo para MVP/shadow y deben corregirse antes de declarar switching canonico production-ready.

| Decision | Estado | Decision correcta para producto final | Trabajo restante |
| --- | --- | --- | --- |
| Ubicacion de contratos y tipos Rust | Cerrada | Usar `trogonai-session-contracts` para protos/tipos y `trogonai-session-kernel` para log, snapshots, leases y materializacion. No mover el kernel dentro de `trogon-transcript`. | Mantener compatibility fixtures y migrators por version. |
| Separacion de kernel/capabilities/switching/artifacts | Cerrada | Mantener crates separados: Session Kernel/Core owns contracts; runners solo adaptan/proyectan. | Evitar que runners definan contratos canonicos propios. |
| Nombres de streams y buckets NATS | Cerrada para implementacion inicial | Usar prefijo configurable con defaults `ACP_*`: session events, snapshots, leases, usage y artifacts. | Documentar nombres por ambiente y validar collision/retention antes de production. |
| Feature flags de rollout | Cerrada como secuencia | Defaults conservadores: kernel off o shadow, handoff legacy, canonical opt-in, event-primary opt-in para sesiones nuevas. | Implementar enforcement de promocion/rollback basada en metricas. |
| Rollout inicial | Cerrada como direccion | Shadow mode -> canonical opt-in -> event-primary para sesiones nuevas -> migracion de sesiones legacy. | No activar default sin thresholds de shadow divergence y fallback budget. |
| Runners/modelos iniciales de certificacion | Cerrada como proceso | Baseline Claude/Grok puede existir como `Basic`/manual; `SwitchSafe` o `Production` solo por probes/contract tests. | Implementar probes y promocion verificable de la matriz. |
| Capability baseline manual | Cerrada | Baseline manual debe ser usable para negociacion conservadora, pero no debe equivaler a certificacion real. | Marcar manual/unverified en UX/metrics/policy y bloquear o confirmar si falta verificacion. |
| Limites, TTLs, retention y SLOs iniciales | Cerrada con valores iniciales revisables | Defaults conservadores para inline artifact, payload, snapshots, leases, retention, replicas y SLOs. | Mantener los defaults actuales como iniciales y ajustarlos solo con datos de shadow. |
| Ownership de schemas, migraciones y capability registry | Cerrada | Session Kernel/Core owns schemas, migrations, compatibility fixtures y capability registry semantics. | Mantener review centralizado de schema changes. |
| Fallback de canonical switch a `export/import` | Cerrada; pendiente de implementacion | `export/import` solo es permitido en legacy/shadow/MVP con warning. En `runner_binding_mode=canonical` o sesiones event-primary, fallo canonical debe producir rollback, `failed_recoverable` o `blocked`, no handoff silencioso. | Cambiar `cross_runner` para condicionar fallback por modo y emitir `SwitchResult` estructurado. |
| Continuity Checkpoint | Cerrada; pendiente de implementacion production | `internal_echo` solo para tests/shadow/MVP. Switches high-risk o production deben validar contra el runner/modelo destino. | Hacer checkpoint real configurable por risk policy y bloquear risky tools si falla. |
| `SwitchResult` formal | Cerrada como contrato | Debe existir como contrato estructurado con `switched`, `blocked`, `requires_confirmation`, `degraded`, `repaired`, `rolled_back`, `failed_recoverable`, `failed_terminal`. | Agregar enum/tipo compartido en proto/Rust y mapearlo a eventos, metrics, CLI y API. |
| Contrato de resultado visible del switch | Cerrada como contrato minimo | Toda superficie visible debe exponer `session_id`, `result`, modelos/runners origen/destino, degradations, lost capabilities, fallback, checkpoint y `next_action`. | Crear respuesta unica de switch y golden tests para estados principales. |
| Block vs confirmation | Cerrada como politica inicial | Lo irreversible, no portable o indispensable faltante bloquea; degradacion aceptable requiere confirmacion. | Implementar matriz configurable por risk/capability/artifact/tool state. |
| Fallback budget y shadow divergence thresholds | Cerrada con valores iniciales revisables | Canonical no puede promoverse si cae demasiado a handoff o si replay/snapshot divergen mas que el umbral definido. | Implementar thresholds iniciales, metricas y rollback automatico/manual. |

Las decisiones de direccion quedan cerradas. Los items marcados como pendientes de implementacion no son debates abiertos: son trabajo requerido para que el producto cumpla la semantica final de "cambiar modelo dentro de la misma sesion".

### Decisiones finales cerradas

Estas reglas cierran las decisiones de producto para el objetivo final del documento: cambiar en medio de una sesion a cualquier modelo soportado por cualquier proveedor de los runners existentes y continuar el trabajo sin continuidad falsa.

| Area | Decision final |
| --- | --- |
| Feature flags de rollout | El default debe ser conservador. `handoff` y `shadow` sirven para migrar; `canonical` y `event-primary` se activan por opt-in y solo se promueven por metricas. |
| Certificacion de runners/modelos | La matriz baseline puede arrancar en `Basic`, pero `SwitchSafe` y `Production` solo se asignan por probes, contract tests o validacion explicita. |
| Capability baseline manual | Puede alimentar Capability Negotiation, pero siempre cuenta como manual/no verificada. No debe habilitar switches sin warning cuando la capability es indispensable. |
| Limites y SLOs | Los defaults iniciales son aceptables solo como punto de partida. Promotion a default requiere datos de shadow y criterios documentados para cambiar limites, TTLs, retention y SLOs. |
| Fallback canonical -> handoff | Handoff automatico solo se permite en legacy/shadow/MVP con warning. En sesiones canonical/event-primary, el fallo debe devolver rollback, `failed_recoverable` o `blocked`; nunca handoff silencioso. |
| Continuity Checkpoint | `internal_echo` solo es valido para tests/shadow/MVP. En production o high-risk, el checkpoint debe ejecutarse contra el runner/modelo destino. |
| `SwitchResult` | Debe ser un contrato compartido y durable, no un struct local ambiguo. Debe cubrir `switched`, `blocked`, `requires_confirmation`, `degraded`, `repaired`, `rolled_back`, `failed_recoverable` y `failed_terminal`. |
| Resultado visible del switch | Debe existir una respuesta unica para CLI/TUI/eventos NATS/structs Rust/API futura con `session_id`, `result`, modelos/runners origen y destino, degradaciones, capabilities perdidas, fallback, checkpoint y `next_action`. |
| Block vs confirmation | Lo irreversible, no portable o indispensable faltante bloquea. Lo portable con degradacion aceptable requiere confirmacion explicita. La politica debe ser configurable y testeada. |
| Fallback budget y shadow divergence | Canonical no se promueve si el fallback a handoff supera el presupuesto o si replay/snapshot divergen mas que el umbral. Ambos umbrales deben existir antes de activar default. |

Regla de revision:

- si una implementacion usa `export/import`, `internal_echo`, baseline manual o matriz `Basic`, debe marcarlo como temporal en eventos, metricas y UX;
- si el usuario ve `switched`, no puede haber handoff silencioso, checkpoint pendiente critico ni degradacion no reconocida;
- si el sistema no puede preservar una capability indispensable, debe bloquear o pedir confirmacion, no continuar como si la sesion fuera equivalente;
- si una politica no tiene threshold o criterio de cierre, no puede usarse para promover canonical/event-primary a default.

Valores iniciales cerrados y revisables:

- **Shadow divergence**: cero tolerancia para perdida canonica, tool IO truncado como verdad, `session_id` cambiado, artifact ref perdido o capability indispensable omitida; divergencias menores de proyeccion pueden permitirse solo si quedan registradas y bajo presupuesto de rollout.
- **Fallback budget**: fallback a handoff permitido solo en legacy/shadow/MVP; cualquier fallback en una sesion kernel-primary/canonical bloquea promocion a default hasta corregirse.
- **SLO/TTL/retention/limits**: usar los defaults actuales de implementacion como valores iniciales; cualquier cambio requiere datos de shadow, impacto de costo/UX y actualizacion documentada.

### Reglas de interpretacion semantica

Estas reglas evitan lecturas permisivas de secciones MVP, roadmap o ejemplos. Aplican a todo el documento.

1. **Handoff no cumple la semantica final**
   - `session/export` / `session/import` puede existir en legacy, shadow, MVP o migracion.
   - No cuenta como cambio canonico dentro de la misma sesion.
   - Cualquier uso de handoff debe quedar visible en eventos, metricas y UX.

2. **High-risk requiere checkpoint real o resolucion explicita**
   - Para production/high-risk, `internal_echo` no es suficiente.
   - El switch debe terminar con checkpoint real pasado, checkpoint reparado, bloqueo, rollback o confirmacion explicita de degradacion.
   - Una advertencia pasiva no basta para continuar con acciones riesgosas.

3. **Configurable no significa abierto**
   - La regla de producto esta cerrada.
   - Lo configurable son valores como thresholds, budgets, TTLs, SLOs, strictness y perfiles de rollout.
   - Todo valor configurable debe tener default, metrica, criterio de cambio y rollback.

4. **`switched` no puede ocultar degradacion**
   - `switched` significa cambio completado sin degradaciones relevantes, sin handoff y sin checkpoint critico pendiente.
   - Si hubo handoff, fallback, perdida de capability, checkpoint reparado o confirmacion requerida, el resultado debe ser `degraded`, `repaired`, `requires_confirmation`, `rolled_back`, `failed_recoverable` o `blocked`, segun corresponda.

5. **"Cualquier modelo/proveedor" significa soportado por Trogonai**
   - El objetivo no significa cualquier modelo disponible en internet.
   - Significa cualquier modelo resoluble por un runner existente, con credenciales/configuracion disponibles, capabilities conocidas y politica/certificacion suficiente.
   - Si falta una capability indispensable, el switch bloquea o requiere confirmacion explicita de degradacion.

### Implementation Backlog

Este backlog contiene solo el trabajo que queda despues de clasificar las decisiones anteriores. Los items cerrados por la implementacion actual no deben volver a tratarse como investigacion; deben mantenerse con tests, fixtures y review de compatibilidad.

| Backlog item | Owner sugerido | Depende de | Resultado esperado | Bloquea |
| --- | --- | --- | --- | --- |
| Mantener governance de contratos y migraciones | Session Kernel/Core | `trogonai-session-contracts` y migrators existentes | Compatibility fixtures por version, schema review y migrators documentados | Event-primary, upgrades, runners adapters |
| Documentar namespaces NATS por ambiente | Platform/Infra | Defaults `ACP_*` actuales | Tabla de streams/buckets/object stores por ambiente, retention y collision policy | Production readiness |
| Implementar thresholds de shadow divergence | Core/Product Infra | Shadow event log + snapshot comparison | Umbral inicial: cero perdida canonica tolerada; divergencias no canonicas deben quedar bajo presupuesto documentado antes de promocion | PR 11, default rollout |
| Implementar fallback budget | Product/Core | Metrics de canonical attach y handoff fallback | Presupuesto inicial: fallback a handoff solo en legacy/shadow; cualquier fallback en kernel-primary bloquea promocion | Canonical rollout |
| Corregir fallback canonical -> handoff | Runtime/CLI | `SwitchResult`, feature flags y runner binding mode | Handoff permitido solo en legacy/shadow; kernel-primary usa rollback/error reparable | PR 9, PR 11, semantica final |
| Implementar checkpoint real para high-risk | Runtime/Switching | Runner checkpoint API y risk policy | `internal_echo` limitado a tests/shadow; production/high-risk valida contra modelo destino | Production switching, risky tools |
| Promover certificacion por probes | Runtime/Capabilities | Baseline Claude/Grok `Basic` | Probes/contract tests que suben modelos a `SwitchSafe` o `Production` | Switch sin warning, provider confidence |
| Formalizar `SwitchResult` compartido | Session Kernel/Switching | Contratos proto/Rust | Enum/tipo durable con estados finales, eventos y metrics | Resultado visible, rollback, policy tests |
| Crear respuesta visible unica de switch | CLI/TUI/API/Product | `SwitchResult` formal | Respuesta con modelos/runners, degradations, lost capabilities, fallback, checkpoint y `next_action` | Producto final, golden tests |
| Implementar matriz block vs confirmation | Product/Core | Safety Gate y capability/adaptation plan | Politica configurable: irreversible/no portable/indispensable bloquea; degradacion aceptable confirma | PR 10, Safety Gate consistency |
| Ajustar limites y SLOs con datos reales | Platform/Core | Shadow metrics y NATS deployment target | Defaults revisados para inline limit, TTL, retention, replicas y latency SLOs | Production readiness |

Cada item debe producir una decision o cambio verificable. Si una decision queda temporal, debe incluir la condicion concreta para revisarla y no debe bloquear el modo shadow.

### Nota para implementaciones parciales

Si ya existe una implementacion parcial, migrarla incrementalmente detras de feature flags. No romper sesiones legacy innecesariamente, pero tampoco presentar legacy, handoff o `session/export` / `session/import` como switching canonico final.

La compatibilidad temporal debe quedar visible en eventos, metricas y UX, y debe converger hacia `SwitchResult`, respuesta visible unica, rollback/error reparable en canonical/event-primary y checkpoint real para high-risk.

### Rollback Strategy

- Cada fase debe estar detras de feature flag.
- Shadow mode nunca debe bloquear el flujo actual salvo corrupcion detectada.
- Si event append falla en modo primary, no mutar.
- Si canonical projection falla en legacy/shadow, usar fallback solo con warning explicito.
- Si canonical projection falla en una sesion event-primary, no mutar y devolver `failed_recoverable` o `blocked`.
- Si runner binding canonical falla en legacy/shadow, volver al handoff actual solo mientras exista compatibilidad y con warning explicito.
- Si runner binding canonical falla en `runner_binding_mode=canonical` para una sesion kernel-primary, hacer rollback del binding anterior o devolver error reparable; no hacer handoff silencioso.

### Implementation Exit Criteria

El cambio puede considerarse listo para default cuando:

- shadow event log y snapshot coinciden con el estado actual en sesiones reales;
- `compactor_model` se preserva en switches cross-runner;
- no hay duplicacion de tool calls en retries;
- switches inseguros se bloquean o requieren confirmacion;
- tool IO canonico no se trunca;
- al menos dos runners externos pasan la matriz basica de switching;
- rollback por feature flag fue probado;
- el codigo cumple los ADRs de plataforma (PR #207): contratos protobuf versionados (ADR-0009), boundaries y naming de crates (ADR-0002/0001), transporte NATS interno sin gRPC/REST entre internos (ADR-0003), configuracion por precedencia ADR-0007 sin valores configurables hardcodeados, y observabilidad OpenTelemetry con provider inicializado en el binario host (ADR-0008).

## Criterios de aceptacion

Un cambio de modelo esta bien implementado si:

- el usuario permanece en la misma sesion visible;
- cada evento tiene `event_id`, `session_id`, `seq`, `operation_id`, `correlation_id`, `causation_id`, `idempotency_key`, `created_at` y `actor`;
- los snapshots guardan `last_applied_seq`;
- los retries no duplican eventos ni tool calls;
- las tools no idempotentes usan receipts y `requires_reconciliation`;
- no se pierde historial canonico;
- no hay dos operaciones mutadoras simultaneas sobre el mismo `session_id`;
- el Session Lease se renueva durante operaciones largas y expira si el proceso cae;
- no se truncan tool inputs/outputs como verdad almacenada;
- el runner destino puede construir contexto desde la sesion Trogonai;
- las capabilities del modelo destino se respetan;
- `compactor_model` se preserva al cambiar el modelo principal, salvo fallback/degradacion auditada;
- existe un Context Twin actualizado antes del switch;
- existe un switch adaptation plan antes de adjuntar el runner destino;
- existe una evaluacion de Switch Safety antes de registrar `model_switched`;
- los switches inseguros se bloquean o piden confirmacion;
- para switches de alto riesgo existe un Continuity Checkpoint real pasado, reparado, bloqueo, rollback o confirmacion explicita de degradacion;
- el estado no portable se invalida de forma explicita;
- los artefactos grandes siguen disponibles;
- los permisos y policies portables se preservan;
- el cambio queda auditado;
- existe un `model_switched` en el event log;
- existen `context_twin_updated`, `switch_adaptation_plan_created`, `switch_safety_evaluated` y eventos de continuity checkpoint cuando aplica;
- los snapshots de KV pueden regenerarse desde eventos;
- hay tests reales entre proveedores.

## Conclusion

Para calidad de producto, Trogonai no debe depender de que cada runner migre su propio estado. Eso produciria comportamiento inconsistente y dificil de explicar.

La direccion correcta es un Session Kernel de Trogonai sobre NATS/JetStream, con Session Lease, Event Contract, event log canonico, snapshots en KV, artifact store, Context Twin, Capability Negotiation, Switch Safety Gate, Continuity Checkpoint, Prompt Compiler y runners como bindings runtime intercambiables. El usuario debe sentir continuidad; internamente, Trogonai debe tratar cada proveedor como un backend que puede cambiar sin controlar la identidad ni la verdad de la sesion.

El sistema puede empezar con un handoff conversacional, pero no debe quedarse ahi si el objetivo es soportar cambio de modelo confiable entre cualquier proveedor.
