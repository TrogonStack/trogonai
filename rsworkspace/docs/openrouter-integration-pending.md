# trogon-openrouter-runner — completado

Todos los ítems pendientes han sido resueltos.

## Resumen de cambios

### 1. Conflicto bollard-stubs — RESUELTO
`testcontainers-modules` alineado a `0.15` en todos los crates del workspace
(workspace root + 33 crates individuales que tenían `0.8` / `0.8.0` / `=0.8`).

### 2. Tests — RESUELTO
Añadido directorio `tests/` con tres archivos de integración:
- `tests/kv_integration.rs` — AgentLoader, SkillLoader, NatsSessionStore (15 tests)
- `tests/agent_nats_integration.rs` — sesiones sobre JetStream real (11 tests)
- `tests/nats_e2e.rs` — ACP request-reply sobre NATS real (3 tests)

Ejecutar con Docker activo:
```
cargo test -p trogon-openrouter-runner
```

### 3. Dockerfile — RESUELTO
- `-p trogon-openrouter-runner` añadido al stage builder
- Stage 16 runtime añadido (`debian:bookworm-slim`)

### 4. docker-compose.yml — RESUELTO
Servicio `trogon-openrouter-runner` añadido con `ACP_PREFIX: acp.openrouter`,
`OPENROUTER_DEFAULT_MODEL` y `OPENROUTER_API_KEY`.
