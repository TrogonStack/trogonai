# CLI branch — estado actual (2026-05-06)

## Dev B completo

Todos los PRs del plan están implementados y testeados en la rama `cli`:

| PR | Contenido | Estado |
|---|---|---|
| PR 3 | trogon-cli core (REPL, sesión NATS, historial) | ✅ |
| PR 5 | @mentions de archivos + herramientas paralelas | ✅ |
| PR 8 | TUI: diffs, Ctrl+C, usage, multilinea | ✅ |
| PR 9 | Slash commands: /help /cost /clear /compact /config /model /init | ✅ |
| PR 10 | Modo no-interactivo --print + --output-format json | ✅ |
| PR 12 | Plugin JetBrains | ✅ |

## Tests

- 77 unit tests — todos pasan
- 17 integration tests (testcontainers NATS) — todos pasan
- 5 e2e tests (scripts/e2e-test.sh, sin credenciales) — todos pasan

## Stubs parciales — esperando Dev A

| Comando | Qué hay ahora | Qué falta (requiere runner) |
|---|---|---|
| `/compact` | Muestra stats de contexto + threshold 85% | Trigger real vía `trogon.compactor.compact` |
| `/model <id>` | Guarda en config, actualiza estimados de costo | Cambiar modelo en sesión viva sin reiniciar |
| `/init` | Genera template TROGON.md con detección de lenguajes | Análisis LLM del proyecto vía ACP |

## Pendiente de Dev A

PRs 1, 2, 4, 6, 7, 10-permisos, 11 (VS Code).
Una vez que Dev A termine, se puede hacer merge de `cli` a `main` y completar los stubs.

## Próximos pasos posibles en cli

1. Mejorar `/init`: leer `Cargo.toml` / `package.json` para extraer nombre del proyecto y dependencias reales
2. Más tests e2e del REPL (entrada simulada)
3. Preparar merge a `main` cuando Dev A esté listo
