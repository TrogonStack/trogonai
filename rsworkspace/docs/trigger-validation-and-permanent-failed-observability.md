# Plan de implementación — validación de trigger y observabilidad de PermanentFailed

Dos PRs independientes en la rama `platform`.

---

## PR 1 — Validación del trigger en `trogon-automations`

**Problema**: Un typo en el campo `trigger` de una `Automation` se guarda en KV sin error.
La automation aparece activa pero nunca dispara. No hay log ni señal de error.

**Rama**: platform  
**Archivos**: `trogon-automations/src/trigger.rs`, `trogon-automations/src/store.rs`, `trogon-automations/src/api.rs`

---

### `crates/trogon-automations/src/trigger.rs`

Agregar al final del archivo, después de `matches`:

```rust
/// Return `Err` if `trigger` has an invalid format.
///
/// Rules:
///   - Must not be empty.
///   - Must not contain whitespace.
///   - If it contains `:`, neither subject nor action can be empty.
pub fn validate(trigger: &str) -> Result<(), String> {
    if trigger.is_empty() {
        return Err("trigger must not be empty".to_string());
    }
    if trigger.chars().any(|c| c.is_whitespace()) {
        return Err(format!("trigger must not contain whitespace: {trigger:?}"));
    }
    if let Some((subj, action)) = trigger.split_once(':') {
        if subj.is_empty() {
            return Err("trigger subject must not be empty".to_string());
        }
        if action.is_empty() {
            return Err(
                "trigger action must not be empty (omit `:` to match any action)".to_string(),
            );
        }
    }
    Ok(())
}
```

No se valida contra un allowlist de subjects conocidos — eso se rompe cada vez
que se añade una integración nueva.

**Tests a añadir en la sección `#[cfg(test)]` de `trigger.rs`**:

```rust
#[test]
fn validate_rejects_empty() {
    assert!(validate("").is_err());
}

#[test]
fn validate_rejects_whitespace() {
    assert!(validate("github.push opened").is_err());
    assert!(validate("github.push\t").is_err());
}

#[test]
fn validate_rejects_empty_action() {
    assert!(validate("github.push:").is_err());
}

#[test]
fn validate_rejects_empty_subject() {
    assert!(validate(":opened").is_err());
}

#[test]
fn validate_accepts_valid_triggers() {
    assert!(validate("github.push").is_ok());
    assert!(validate("github.pull_request:opened").is_ok());
    assert!(validate("cron").is_ok());
    assert!(validate("cron.my-job-id").is_ok());
    assert!(validate("linear.Issue:create").is_ok());
}
```

---

### `crates/trogon-automations/src/store.rs`

`StoreError` es `struct StoreError(pub String)` — no un enum. La API mapea
**todo** `StoreError` a `500 INTERNAL_SERVER_ERROR` (líneas 252, 310, 373).
Si la validación vive solo en `store.put()`, un trigger inválido devuelve 500
al cliente — incorrecto. La validación del trigger es un error de input (4xx),
no un error de infraestructura.

Mantener la validación en `store.put()` como cinturón y tirantes para cualquier
call site que evite el API. Añadir el import al inicio de `store.rs`:

```rust
use crate::trigger;
```

Y en `put()`, antes de serializar:

```rust
trigger::validate(&automation.trigger).map_err(StoreError)?;
```

Pero el 400 lo devuelven los handlers del API — ver sección siguiente.

---

### `crates/trogon-automations/src/api.rs`

La validación primaria del trigger va en los dos handlers que aceptan input de usuario.
El handler de enable (línea ~372) no cambia el trigger — no necesita re-validar.

**Handler create (línea ~248, antes de `state.store.put`):**

```rust
if let Err(e) = crate::trigger::validate(&automation.trigger) {
    return err(StatusCode::BAD_REQUEST, e);
}
```

**Handler update (línea ~306, antes de `state.store.put`):**

```rust
if let Err(e) = crate::trigger::validate(&updated.trigger) {
    return err(StatusCode::BAD_REQUEST, e);
}
```

**Tests a añadir en `api.rs`** (usan `MockAutomationStore` — la validación
ocurre en el handler, antes de llegar al store, por lo que el mock no interfiere):

```rust
#[tokio::test]
async fn create_automation_invalid_trigger_returns_400() {
    // POST /automations con trigger: "" → 400 Bad Request
    let body = json!({
        "name": "bad automation",
        "trigger": "",
        "prompt": "do something",
        "model": "claude-3-5-sonnet-20241022"
    });
    // ... assert_eq!(resp.status(), StatusCode::BAD_REQUEST)
}

#[tokio::test]
async fn create_automation_whitespace_trigger_returns_400() {
    // POST con trigger: "github.push opened" (espacio) → 400
}

#[tokio::test]
async fn update_automation_invalid_trigger_returns_400() {
    // PUT con trigger: "github.push:" (action vacía) → 400
}
```

---

## PR 2 — OTel counter + NATS event en PermanentFailed (`trogon-agent`)

**Problema**: Cuando una automation se marca `PermanentFailed` (después de 5 intentos
de recovery), solo se emite un `warn!()`. Sin métrica ni evento, las fallas
permanentes son invisibles en producción.

**Solución**: Un counter OTel (`trogon.agent.automation.permanent_failed`) y un
evento NATS en `trogon.agent.permanent_failed` publicados en los 4 paths de
PermanentFailed del runner.

**Rama**: platform  
**Archivos**: `trogon-agent/src/telemetry/metrics.rs` (nuevo),
`trogon-agent/src/telemetry/mod.rs` (nuevo), `trogon-agent/src/runner.rs`,
`trogon-agent/Cargo.toml`

---

### `crates/trogon-agent/src/telemetry/metrics.rs` (archivo nuevo)

Patrón idéntico al de `trogon-router/src/telemetry/metrics.rs`:

```rust
use opentelemetry::{KeyValue, metrics::Counter};
use std::sync::OnceLock;

fn meter() -> opentelemetry::metrics::Meter {
    opentelemetry::global::meter("trogon-agent")
}

static AUTOMATION_PERMANENT_FAILED: OnceLock<Counter<u64>> = OnceLock::new();

fn automation_permanent_failed() -> &'static Counter<u64> {
    AUTOMATION_PERMANENT_FAILED.get_or_init(|| {
        meter()
            .u64_counter("trogon.agent.automation.permanent_failed")
            .with_description("Automation runs marked PermanentFailed")
            .build()
    })
}

pub fn inc_automation_permanent_failed(automation_id: &str, reason: &str) {
    automation_permanent_failed().add(
        1,
        &[
            KeyValue::new("automation_id", automation_id.to_string()),
            KeyValue::new("reason", reason.to_string()),
        ],
    );
}
```

---

### `crates/trogon-agent/src/telemetry/mod.rs` (archivo nuevo)

```rust
pub mod metrics;
```

---

### `crates/trogon-agent/src/lib.rs`

Añadir junto a los otros módulos (línea ~42):

```rust
pub mod telemetry;
```

---

### `crates/trogon-agent/src/runner.rs`

**Import a añadir al inicio de `runner.rs`** (junto a los otros `use crate::...`):

```rust
use crate::telemetry;
```

**Struct del evento** (añadir cerca de los imports):

```rust
#[derive(serde::Serialize)]
struct PermanentFailedEvent<'a> {
    promise_id: &'a str,
    automation_id: &'a str,
    tenant_id: &'a str,
    reason: &'a str,              // categoría: "max_recovery_attempts" | "automation_disabled_or_deleted" | "unknown_subject"
    failure_reason: Option<&'a str>,
    recovery_count: u32,
}
```

**Helper** (añadir como función libre en el módulo):

```rust
async fn emit_permanent_failed(
    nc: Option<&async_nats::Client>,   // None en tests
    promise_id: &str,
    automation_id: &str,
    tenant_id: &str,
    failure_reason: Option<&str>,
    recovery_count: u32,
    reason: &str,
) {
    telemetry::metrics::inc_automation_permanent_failed(automation_id, reason);
    let Some(nc) = nc else { return };
    let payload = PermanentFailedEvent {
        promise_id,
        automation_id,
        tenant_id,
        reason,
        failure_reason,
        recovery_count,
    };
    if let Ok(bytes) = serde_json::to_vec(&payload) {
        nc.publish("trogon.agent.permanent_failed", bytes.into())
            .await
            .ok();   // publicación fire-and-forget: no fallar si NATS no está disponible
    }
}
```

**`prepare_agent_with_promise` — añadir `nats: Option<&async_nats::Client>`**

Path 1 está dentro de `prepare_agent_with_promise` (línea ~913), no en `run()`
directamente. La función no tiene acceso a `nats` actualmente — hay que añadirlo.
Se usa una referencia (no valor) porque la función no hace spawn: el borrow vive
durante toda la llamada sin necesidad de transferir ownership.

```rust
// Firma (línea ~913) — añadir último parámetro
async fn prepare_agent_with_promise(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    tenant_id: &str,
    promise_id: &str,
    automation_id: &str,
    nats_subject: &str,
    trigger: &serde_json::Value,
    nats: Option<&async_nats::Client>,   // ← añadir
) -> Option<Arc<AgentLoop>>
```

**Call sites en `run()` (7 spawn closures, líneas ~434, 500, 556, 612, 668, 769, 830)**

Cada spawn hace `async move`. Clonar `nats` antes del spawn y pasar la referencia:

```rust
let nats_ref = nats.clone();   // ← añadir antes del tasks.spawn
tasks.spawn(async move {
    // ...
    let Some(agent) = prepare_agent_with_promise(
        &agent, &promise_store, &tenant_id,
        &promise_id_prefix, "", subject, &pv,
        Some(&nats_ref),   // ← añadir
    ).await else { break 'handler; };
});
```

**Call sites en `recover_stale_promises` (2 call sites, líneas ~1370, ~1776)**

Dentro del spawn de `recover_stale_promises`, `nats` ya es `Option<async_nats::Client>`.
Pasar `nats.as_ref()` que da `Option<&async_nats::Client>`:

```rust
let Some(agent) = prepare_agent_with_promise(
    ...,
    nats.as_ref(),   // ← añadir
).await else { ... };
```

**~12 call sites en tests** — pasar `None` al final de cada llamada.

---

**`recover_stale_promises` — usar `Option<async_nats::Client>`**

Los paths 2–4 están dentro de `recover_stale_promises`. La función tiene
~40 call sites en los tests — añadir `&async_nats::Client` obligatorio
rompería todos. En su lugar, usar `Option<async_nats::Client>`: los tests
pasan `None`, producción pasa `Some(nats.clone())`. El helper no publica si
recibe `None`.

```rust
// Firma (línea ~1115)
async fn recover_stale_promises<A: AutomationRepository, R: RunRepository>(
    agent: &Arc<AgentLoop>,
    promise_store: &Arc<dyn PromiseRepository>,
    automation_store: &Arc<A>,
    run_store: &Arc<R>,
    tenant_id: &str,
    skill_loader: Option<Arc<dyn crate::skill_loader::SkillLoading>>,
    nats: Option<async_nats::Client>,   // ← añadir
) -> Option<tokio::task::JoinHandle<()>>
```

```rust
// Call site en run() (línea ~258)
let _ = recover_stale_promises(
    &agent,
    &promise_store,
    &store,
    &run_store,
    &tenant_id,
    skill_loader.clone(),
    Some(nats.clone()),   // ← añadir
)
.await;
```

Los ~40 call sites en tests pasan `None` — no necesitan cambios de lógica,
solo añadir el argumento `None` al final de cada llamada.

`nats: Option<async_nats::Client>` se recibe por valor y se mueve directamente
al `tokio::spawn` — no hace falta `.clone()` dentro de la función. El clone
ocurre en el call site de `run()` (`Some(nats.clone())`), donde `nats` sigue
siendo necesario después de la llamada.

**4 call sites** — hay 4 paths de PermanentFailed en el runner, no 3.
Todos los paths 2–4 están dentro del `tokio::spawn` de `recover_stale_promises`:
`nats` es `Option<async_nats::Client>` → usar `nats.as_ref()`.

| Path | Línea aprox. | `failure_reason` | `reason` (métrica) |
|------|-------------|------------------|-------------------|
| `recovery_count >= MAX_RECOVERY_ATTEMPTS` | ~979 | añadir `over_limit.failure_reason = Some("max_recovery_attempts".to_string())` antes del KV write | `"max_recovery_attempts"` |
| Startup: automation disabled/deleted (primera pasada) | ~1446 | `current.failure_reason.as_deref()` — ya se setea con `"automation is disabled"` / `"automation no longer exists"` | `"automation_disabled_or_deleted"` |
| Startup: unknown subject | ~1580 | añadir `current.failure_reason = Some("unknown subject".to_string())` antes del KV write | `"unknown_subject"` |
| Startup: automation disabled/deleted (reintento) | ~1669 | `current.failure_reason.as_deref()` — ya se setea con `reason.to_string()` | `"automation_disabled_or_deleted"` |

```rust
// Path 1 — recovery_count >= MAX_RECOVERY_ATTEMPTS (~979)
// Contexto: dentro de prepare_agent_with_promise.
// `nats` es el parámetro Option<&async_nats::Client> — pasar directamente.
let mut over_limit = existing.clone();
over_limit.status = PromiseStatus::PermanentFailed;
over_limit.failure_reason = Some("max_recovery_attempts".to_string()); // ← añadir
// ... KV write ...
Ok(Ok(_)) => {
    emit_permanent_failed(
        nats,   // ← Option<&async_nats::Client>, el parámetro de la función
        promise_id,
        &existing.automation_id,
        tenant_id,
        over_limit.failure_reason.as_deref(),
        existing.recovery_count,
        "max_recovery_attempts",
    )
    .await;
}

// Path 2 — automation disabled/deleted, primera pasada (~1446)
// failure_reason ya está seteado en current antes del KV write.
Ok(Ok(_)) => {
    emit_permanent_failed(
        nats.as_ref(),
        &promise.id,
        &promise.automation_id,
        tenant_id,
        current.failure_reason.as_deref(),
        current.recovery_count,
        "automation_disabled_or_deleted",
    )
    .await;
}

// Path 3 — unknown subject (~1580)
// failure_reason NO está seteado en el código actual — añadirlo antes del KV write:
//   current.failure_reason = Some("unknown subject".to_string()); // ← añadir
Ok(Ok(_)) => {
    emit_permanent_failed(
        nats.as_ref(),
        &promise.id,
        &promise.automation_id,
        tenant_id,
        current.failure_reason.as_deref(),
        current.recovery_count,
        "unknown_subject",
    )
    .await;
}

// Path 4 — automation disabled/deleted, reintento (~1669)
// El código actual usa `let _ = tokio::time::timeout(...).await` — sin match.
// Cambiar a match para poder llamar emit solo si el KV write fue exitoso:
match tokio::time::timeout(
    crate::agent_loop::NATS_KV_TIMEOUT,
    promise_store.update_promise(&tenant_id, &promise.id, &current, rev),
)
.await
{
    Ok(Ok(_)) => {
        emit_permanent_failed(
            nats.as_ref(),
            &promise.id,
            &promise.automation_id,
            tenant_id,
            current.failure_reason.as_deref(),
            current.recovery_count,
            "automation_disabled_or_deleted",
        )
        .await;
    }
    _ => {}
}
```

---

### `crates/trogon-agent/Cargo.toml`

`opentelemetry` está en el workspace Cargo.toml (versión `=0.31.0`) pero
`trogon-agent` no lo declara como dependencia. Añadir:

```toml
opentelemetry = { workspace = true }
```

---

### Tests para PR 2

**OTel counter**: no es unit-testable directamente — el meter global de
OpenTelemetry requiere inicialización. El patrón del router (`trogon-router`)
tampoco tiene tests para sus counters. Aceptable para v1.

**NATS event**: los tests de `recover_stale_promises` que ya usan NATS real
(líneas ~6174, ~6625, ~6679 en runner.rs) pueden extenderse para verificar
que el evento se publica. Requiere:

1. Pasar `Some(nats.clone())` en lugar de `None` en esos tests específicos.
2. Suscribirse a `trogon.agent.permanent_failed` antes de llamar a la función.
3. Verificar que el subscriber recibe un mensaje tras el `JoinHandle.await`.

```rust
// Ejemplo de extensión para el test de automation deleted (línea ~6652)
let mut sub = nats.subscribe("trogon.agent.permanent_failed").await.unwrap();
let handle = super::recover_stale_promises(
    // ... args existentes ...
    Some(nats.clone()),  // ← en lugar de None
).await.unwrap();
handle.await.unwrap();
let msg = tokio::time::timeout(Duration::from_secs(1), sub.next())
    .await.unwrap().unwrap();
let event: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
assert_eq!(event["reason"], "automation_disabled_or_deleted");
```

Solo los tests con NATS real necesitan el argumento `Some(nats)`. Los ~40
tests sin NATS siguen pasando `None` y no requieren cambios de lógica.

---

## Resumen

| PR | Archivos tocados | Qué resuelve |
|----|-----------------|-------------|
| PR 1 | `trigger.rs`, `store.rs`, `api.rs` | Trigger inválido → `400 BAD_REQUEST` inmediato, no silencio en runtime |
| PR 2 | `metrics.rs` (nuevo), `telemetry/mod.rs` (nuevo), `lib.rs`, `runner.rs` (4 edits), `Cargo.toml` | PermanentFailed visible via OTel + evento NATS consumible sin monitoring externo |

### Nota sobre `serde_json` en PR 2

`trogon-agent/Cargo.toml` ya tiene `serde_json = "1.0.149"` y
`serde = { version = "1.0.228", features = ["derive"] }` — no hace falta añadirlos.
Solo se añade `opentelemetry = { workspace = true }`.

Los dos PRs son independientes y pueden implementarse en cualquier orden.
