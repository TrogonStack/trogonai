# Trazabilidad: Criterios de aceptación de `cambio-modelo.md` ↔ tests

Mapeo de cada requisito de la sección **§Criterios de aceptación** (`cambio-modelo.md` líneas 2378-2404)
y de los 13 elementos / 7 criterios del **Complex Session Fixture** al test que lo cubre.

- **COVERED** — existe un test que lo asercióna (con cita).
- **EMITTED-NOT-ASSERTED** — el código lo implementa/emite (verificado), pero ningún test lo asercióna explícitamente (hueco de *cobertura de test*, no de implementación).

Rutas relativas a `rsworkspace/`. No modifica ningún código; es solo trazabilidad.

## Criterios de aceptación (AC-1 … AC-24)

| ID | Criterio | Estado | Test / evidencia |
|----|----------|--------|------------------|
| AC-1 | Usuario permanece en la misma sesión visible | COVERED | `crates/trogonai-switching/src/visible_result.rs` golden tests (`session_id` estable en todos los resultados) |
| AC-2 | Cada evento tiene event_id, session_id, seq, operation_id, correlation_id, causation_id, idempotency_key, created_at, actor | COVERED | `crates/trogonai-session-contracts/tests/contract_compat_v1.rs::n_minus_one_reader_accepts_minimal_event_without_optional_fields`; validación en `src/lib.rs::ValidatedSessionEvent::try_from_event` |
| AC-3 | Snapshots guardan last_applied_seq | COVERED | `crates/trogonai-session-kernel/tests/kernel_mock_nats.rs::recovery_rebuilds_state_from_event_log_and_snapshot`; fixture CRITERION 1 (`complex_session_fixture.rs:686`) |
| AC-4 | Retries no duplican eventos ni tool calls | COVERED | fixture CRITERION 6 (`complex_session_fixture.rs:549`); `kernel_mock_nats.rs::append_event_idempotent_deduplicates_retries_after_crash`; dedup en `materialize.rs:26-31` |
| AC-5 | Tools no idempotentes usan receipts y requires_reconciliation | COVERED | fixture ELEMENT 3 receipt (`complex_session_fixture.rs:735`); `safety.rs::evaluate_tool_calls` (RequiresReconciliation→confirm) |
| AC-6 | No se pierde historial canónico | COVERED | fixture CRITERION 1 replay byte-idéntico (`:692`) + CRITERION 3 (`:703`) |
| AC-7 | No dos operaciones mutadoras simultáneas por session_id | COVERED | `kernel_mock_nats.rs::concurrent_mutating_operation_returns_session_busy:138` |
| AC-8 | Session Lease se renueva en ops largas y expira si el proceso cae | COVERED | renovación: `lease/mod.rs::start_renewal` + tests; expiración: `lease/mod.rs:434 mod expiry_tests` |
| AC-9 | No se truncan tool inputs/outputs como verdad almacenada | COVERED | fixture CRITERION 3 (`:720-731` verbatim); `kernel_mock_nats.rs::tool_io_divergence_is_measured...` |
| AC-10 | Runner destino puede construir contexto desde la sesión Trogonai | COVERED | `switching/src/hydration.rs::messages_json_for_runner_hydration`; flujo en `orchestrator.rs` (projection) ejercitado por fixture |
| AC-11 | Capabilities del modelo destino se respetan | COVERED | `capabilities/tests/adaptation_tests.rs`; `safety.rs::evaluate_indispensable_capabilities`; fixture ELEMENT 13 (`:637`) |
| AC-12 | compactor_model se preserva salvo fallback/degradación auditada | COVERED | fixture CRITERION 5 (`:798`); `safety.rs::evaluate_compactor_degradation` |
| AC-13 | Existe un Context Twin actualizado antes del switch | COVERED | Emitido: `orchestrator.rs:279 update_context_twin`; asercionado en `complex_session_fixture.rs` (`assert! has_kind ContextTwinUpdated`, añadido). GAP-1 cerrado. |
| AC-14 | Existe switch adaptation plan antes de adjuntar el runner | COVERED | Emitido: `orchestrator.rs:314`; asercionado en `complex_session_fixture.rs:639-642` (`Kind::SwitchAdaptationPlanCreated`). |
| AC-15 | Evaluación de Switch Safety antes de model_switched | COVERED | fixture `complex_session_fixture.rs:632` (`SwitchSafetyEvaluated`); orden en `orchestrator.rs` (safety en :348, model_switched en :415) |
| AC-16 | Switches inseguros se bloquean o piden confirmación | COVERED | `switch_flow.rs::safety_gate_blocks_tool_in_progress_before_orchestrator:343`; `switch_flow.rs::switch_blocked_when_session_busy:290` |
| AC-17 | Alto riesgo: checkpoint real pasado/reparado/bloqueo/rollback/confirmación | COVERED | fixture CRITERION 7 (`:627`); `switch_flow.rs::switch_rolls_back_to_previous_binding_when_checkpoint_fails:516`; `switch_flow.rs::switch_records_runner_failed...:418` |
| AC-18 | Estado no portable se invalida explícitamente | COVERED (transitivo) | `runner.rs::invalidate_nonportable_runner_state` se ejecuta en `detach_runner`; el `RunnerDetached` (que lo dispara) sí se asercióna (`complex_session_fixture.rs:635`). |
| AC-19 | Artefactos grandes siguen disponibles | COVERED | fixture CRITERION 4 (`:753` artifact ref recuperable) + ELEMENT 5 (`:776` artifact_unavailable) |
| AC-20 | Permisos y policies portables se preservan | COVERED (transitivo) | `permission_rules_text`/`tool_policies_json` en `SessionConfig`; fixture CRITERION 1 (snapshot byte-idéntico) preserva la config completa |
| AC-21 | El cambio queda auditado | COVERED | event log + `SessionSnapshotState.audit_log`; eventos durables verificados por replay (CRITERION 1) |
| AC-22 | Existe model_switched en el event log | COVERED | fixture `:633` + `:897` (`ModelSwitched`); `contract_golden_v1.rs::golden_session_event_model_switched_roundtrip` |
| AC-23 | Existen context_twin_updated, switch_adaptation_plan_created, switch_safety_evaluated y eventos de checkpoint cuando aplica | COVERED | Todos asercionados en `complex_session_fixture.rs`: context_twin_updated (añadido), switch_adaptation_plan_created (`:639-642`), switch_safety_evaluated (`:632`), checkpoint (CRITERION 7). |
| AC-24 | Snapshots de KV pueden regenerarse desde eventos | COVERED | fixture CRITERION 1 (`:686`); `kernel_mock_nats.rs::recovery_rebuilds_state...`; `materialize.rs::materialize_from_events` tests |
| AC-25 | Hay tests reales entre proveedores | COVERED | `crates/trogon-e2e/tests/cross_runner.rs` (8 tests xai/openrouter/acp/codex); `cross_runner_switcher.rs::canonical_switch_records_kernel_state_over_real_nats` |

## Complex Session Fixture — 13 elementos / 7 criterios

Todos cubiertos en `crates/trogonai-switching/tests/complex_session_fixture.rs` (un solo test `complex_session_fixture_passes_all_criteria`, etiquetado inline):
ELEMENT 1 (`:353`), 2 (`:365`), 3 (`:386`), 4 (`:420`), 5 (`:765`), 6 (`:582`), 7 (`:93`/`:644`), 8 (`:460`), 9 (`:522`), 10 (`:552`), 11 (`:656`), 12 (`:634`), 13 (`:152`/`:637`).
CRITERION 1 (`:686`), 2 (proyección determinista), 3 (`:703`), 4 (`:757`), 5 (`:782`), 6 (`:522`), 7 (`:603`).

## Resumen de huecos (solo cobertura de test; NO de implementación)

| Gap | Qué | Estado |
|-----|-----|--------|
| GAP-1 | El fixture no asercionaba el evento `ContextTwinUpdated` (AC-13/AC-23) | **CERRADO** — assert añadido a `complex_session_fixture.rs`; el fixture pasa (confirma que el evento se emite). |
| GAP-2 | (supuesto) `SwitchAdaptationPlanCreated` no asercionado | **FALSO HALLAZGO** — el fixture **sí** lo asercióna (`:639-642`); fue un error de lectura mío en el grep. |
| GAP-3 | Invalidación de `NonPortableState` tras detach (AC-18) | **CUBIERTO (transitivo)** — el `RunnerDetached` que dispara la invalidación sí se asercióna (`:635`). |

**Ningún gap era de implementación.** Tras esta revisión: GAP-1 cerrado con un assert (en archivo de test, sin tocar `src/`); GAP-2 no existía; GAP-3 cubierto. El código ya producía los tres eventos/efectos — solo faltaba 1 assert.

## Desviaciones contrato vs doc (no de cobertura)

| # | Doc dice | Código | Naturaleza |
|---|----------|--------|-----------|
| D-1 | `"type": "..."` campo string en el evento (JSON ilustrativo) | discriminante `oneof` en `SessionEventPayload` | Encoding mandado por ADR-0009; el JSON del doc es ilustrativo |
| D-2 | `created_by_event_id` (§426) | `event_id` (`artifact_metadata.proto:13`) | Contradicción interna del doc: §937 dice `event_id` |
| D-3 | `modo` portable (§796) | ausente de `SessionConfig`; preservado en runtime vía `PortableRunnerConfig.mode` | Contradicción interna del doc: el envelope (§741-752) tampoco lista `mode` |
