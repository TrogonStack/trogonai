# Plan de implementación — Compactación con cualquier modelo de cualquier runner

> Objetivo: permitir que el usuario seleccione **cualquier modelo de cualquiera de los 3
> runners (acp / xai / openrouter), del que exista credencial**, como modelo de
> compactación de contexto, en lugar de estar limitado al mismo proveedor de la sesión.

## 1. Requisito y límite irreducible

**Requisito:** el `compactor_model` puede ser un modelo de un proveedor distinto al de la
sesión (p.ej. sesión en xAI, compactar con `claude-haiku-4-5`).

**Default y override (decidido):** por defecto se compacta con **el modelo que se está
usando** (el de la sesión). El `compactor_model` es un **override opcional**, y puede ser
del **mismo o de cualquier proveedor** (de los 3 runners). No hay un modelo de compactación
"barato" impuesto por el sistema; la elección es del usuario.

**Alcance de "cualquier proveedor":** el universo de proveedores son los runners
existentes — `acp-runner` (Anthropic), `xai-runner` (xAI), `openrouter-runner`
(OpenRouter). Es un mapeo **1:1 con los 3 `ProviderConfig` que el compactor ya carga**
(`service.rs:110-113`, `main.rs:69-142`). No se generaliza a openai/gemini/cohere/mistral
como proveedores directos — esos llegan *a través* de `openrouter-runner`.
`codex-runner` queda **fuera de alcance** por ahora.

**Alcance de "cualquier modelo":** la unión de lo que sirven esos 3 runners. El que aporta
amplitud real es `openrouter-runner` (cientos de modelos/marcas vía `/models`); xAI y
Anthropic aportan sus conjuntos.

**Límite irreducible (honesto):** "cualquier modelo de cualquier proveedor" significa
*cualquier modelo de cualquier proveedor del que exista credencial*. No es posible
compactar con un proveedor sin API key — ninguna arquitectura lo permite. El conjunto
seleccionable es:

```
modelos disponibles   ∩   proveedores con credencial
```

El diseño hace que la **credencial sea el único límite**, eliminando los límites
artificiales actuales (lista curada por env var, atada a la liveness de un runner).

## 2. Estado actual (verificado en código)

| Pieza | Ubicación | Limitación actual |
|---|---|---|
| Selección `compactor_model` | `trogon-xai-runner/src/agent.rs:893` (`compactor_model_config_option`) | El dropdown solo ofrece `self.available_models` → **mismo proveedor por construcción** (`agent.rs:892`) |
| Dropdown en `trogon-acp` (embebido) | `trogon-acp/src/agent.rs:170,200,208` (builder), `:818/:855/:916` (`build_config_options`), `:990` (handler) | **Segundo sitio** que construye/maneja `compactor_model`, además de `multi_runner.rs` |
| Listas de modelos | `xai-runner/src/agent.rs:401`, `openrouter-runner/src/agent.rs:353`, `acp-runner/src/agent.rs:363` | Curadas por env var (`XAI_MODELS`, `OPENROUTER_MODELS`) o hardcodeadas (~3 c/u) |
| Ventana de contexto | `xai-runner/src/agent.rs:88`, `openrouter-runner/src/agent.rs:155`, `acp-runner/src/agent.rs:50` | `context_window_tokens()` hardcodeado y **triplicado** |
| Wire de compactación | `trogon-compactor/src/service.rs:48` (`CompactRequest`) | Un solo campo `provider`, usado para sesión **y** compactor. `resolve_model` (`service.rs:120`) solo cambia el string del modelo dentro del mismo proveedor |
| Servicio compactor | `trogon-compactor/src/main.rs:69-142`, `service.rs:172` (`handle`) | **Ya carga `ProviderConfig` para anthropic/xai/openrouter** y rutea por proveedor — el fondo ya soporta multi-proveedor |
| Compactación acp-runner | `agent.rs:465` (pre-turn), `agent.rs:1413` (ext_method `/compact`); payload en `build_compact_payload` (`agent.rs:172`, `provider:"anthropic"` hardcodeado en `:183`) | **Anthropic-only**: 2 call sites (sin post-turn), `CompactReq` sin `compactor_provider` |
| Compactación (resumen) | `trogon-compactor/src/lib.rs` (`compact_if_needed_counted`), `detector.rs` (`find_cut_point`) | `to_summarize` **entero** se manda a `generate_summary` en **una sola llamada**; sin comprobar la ventana del modelo compactador. `CompactionSettings` derivado **solo de la ventana de sesión** |
| Persistencia `compactor_model` | `xai-runner/session_store.rs:26`, `openrouter-runner/session_store.rs:22`, `runner-tools/session_store.rs:83` (usado por acp) | Solo se persiste el **modelo**, no el proveedor |
| Secret-proxy | `trogon-secret-proxy/src/provider.rs:19` (`base_url`), `proxy.rs:38` | Passthrough genérico. **No** introspección runtime de proveedores. **No** proxy de `/models` (pero es trivial) |
| Registry | `trogon-registry/src/registry.rs:141` (`find_by_model`) | Índice runtime vivo de **modelos curados de runners vivos**; ya resuelve `model → runner` |
| CLI `/compact-model` | `trogon-cli/src/repl.rs:1407` (`do_compact_model`) | Acepta **string libre sin validación** |

**Conclusión:** el bloqueo cross-provider no está en el servicio compactor (ya rutea por
proveedor). Está en (a) el wire (un solo `provider`), (b) la fuente de los modelos
seleccionables (listas curadas per-runner), (c) que **no hay fuente de ventanas para el
picker/selección** (necesarias para filtrar por capacidad en cliente), y (d) que acp-runner
no participa.

## 3. Arquitectura objetivo y la pieza keystone

La compactación cross-provider tiene una **causa raíz** compartida detrás de varios gaps:
no existe una fuente de verdad de `model → (runner/proveedor, ventana, max_output)`.

### Keystone (MUST): catálogo de modelos NATS-backed (patrón `registry`)

`model_id → { runner/proveedor, context_window, max_output }`. **Forma decidida** (anclada en
el idioma real de Trogonai: dato transversal en NATS, accedido por crate cliente — como
`trogon-registry` y los stores del `console`):

- **Dato en NATS** (no caché por proceso): el catálogo es un store NATS-backed, **separado del
  `registry`** (que se queda con liveness) y **fuera del `secret-proxy`** (que se queda stateless).
- **Poblado por un servicio pequeño** que fetch-ea `/models` de cada proveedor **vía el
  `secret-proxy`** (egress externo sancionado por la arquitectura) y lo cachea con TTL + tabla
  lateral para ventanas que el API no exponga.
- **Accedido por un crate cliente fino** (patrón `registry`) y/o subject request-reply, para que
  todos los consumidores —runners, compactor, el `trogon-acp` embebido, y futuros
  handlers/automations/memory— vean **una sola fuente consistente**.

**Por qué es MUST y no condicional:** es lo único que entrega "**cualquier** modelo" de verdad
(descubrimiento dinámico `/models`, cientos de OpenRouter), elimina la limitación same-provider
del Claude embebido (Gap D), consolida los `context_window_tokens` triplicados, y da ventanas
exactas al **filtro de capacidad del picker (selección, en cliente)**. Sin él solo hay listas
curadas (~3 por runner) — que **no** cumplen el objetivo. Se descarta **B** (meterlo en el `registry`: mezcla efímero/maestro) y **C** (caché por
proceso: saca el dato de NATS, contra el idioma de la plataforma).

```
                ┌───────────────────────────────┐
   /models  →   │  catálogo (NATS-backed)       │  ← keystone (MUST)
   (OpenRouter) │   model → runner, ventana,    │
                │            max_output         │
                └───────────────┬───────────────┘
                                │  consulta
            ┌───────────────────┼────────────────────┐
            ▼                   ▼                    ▼
   picker (IDE + CLI)      compactor (solo       (futuro: handlers,
   ventana → filtro        ruteo por proveedor)  automations, memory)
   de capacidad
```
*(El compactor **no** consume ventanas: M4 reactivo da la correctitud. Las ventanas las usa el
**picker** en la selección. Ver la mejora diferida en §9 para un auto-guard server-side.)*

### 3.1 Convenciones de arquitectura (ADRs — PR #207)

Este plan **no cambia su diseño** por los ADRs, pero **sí adopta su forma**. Reglas vinculantes que
aplican a todo lo de abajo (wire, catálogo, persistencia, config, telemetría, naming):

- **Contratos = Protocol Buffers (ADR 0009).** Todo contrato máquina-a-máquina y toda persistencia
  schemaless va como **`.proto` versionado** bajo `proto/`, **encoding binario** en paths internos.
  Cubre: el mensaje del compactor (backbone NATS), los **records KV del catálogo** y los **valores
  persistidos de sesión**. Evolución por el modelo de protobuf (**números de campo nuevos**, reservar
  los retirados), **no** `#[serde(default)]`. Los **tipos generados** viven en paquetes dedicados con
  sufijo **`-proto`** (ADR 0002/0009, porque los comparten varios paquetes) — **`trogonai-compactor-proto`**
  (compactor + 3 runners) y **`trogonai-catalog-proto`** (poblador + crate cliente) — y se convierten a
  **value-objects de dominio en el borde** (no primitive obsession).
  - **Excepción ACP (cláusula de 0009 — "boundary owned by a protocol that requires another format"):**
    las superficies **ACP** son **dueñas-de-protocolo → EXENTAS de protobuf**, se quedan en formato ACP:
    el `config_options`/**`SessionConfigSelectOption`** (incluido el value `"provider::model"` que produce
    el codec, que es un **string ACP**, no un mensaje protobuf) y el ext_method **`/compact`**. Solo los
    contratos **internos** (compactor wire, records KV del catálogo, persistencia de sesión) van protobuf.
- **Config = TOML + precedencia (ADR 0007).** Cada valor configurable tiene **un nombre canónico
  tipado** (`margin`, `catalog_ttl`, …), expuesto por **TOML** (primario), **env** `TROGON_*`
  (override), **CLI** `--kebab` (por invocación). Precedencia **defaults < TOML < env < CLI**;
  validación tras el merge; secretos **fuera** de la config (vía secret-proxy/env).
- **Telemetría = OpenTelemetry (ADR 0008).** Métricas/trazas/logs por OTel (instruments + semantic
  conventions + `service.name`), export OTLP, setup compartido en **`trogon-telemetry`**.
- **Naming y boundaries (ADR 0002/0001).** El catálogo es producto-específico → prefijo
  **`trogonai-*`**; su crate cliente = **`trogonai-catalog-client`** (sufijo `-client`). El **poblador
  `trogonai-catalog` es un Service** (ADR 0001: lifecycle propio, identidad OTel `service.name`, va en
  `services/`). Las
  **reusable crates no dependen de services** (el crate cliente lee del **backbone NATS**, no del
  poblador). NATS = **backbone** interno (ADR 0003, no es el transporte público por defecto); el
  secret-proxy = **egress** sancionado (no un "gateway" inbound en el sentido de 0003).

## 4. Los gaps y su resolución (decididas)

| Gap | Descripción | Resolución decidida |
|---|---|---|
| **1** | acp-runner Anthropic-only (`agent.rs:183` hardcodea `provider:"anthropic"`) | **M2/M3:** añadir `compactor_provider` a `CompactReq`/`build_compact_payload` y resolver en los 2 call sites (`:465`, `:1413`) |
| **2** | Desajuste de ventana sesión grande → compactador chico | **M3 (picker estricto)** no ofrece un compactador que no quepa; si uno se cuela → **M4 red reactiva** (fallback al modelo de sesión + aviso ante cualquier error). Chunking **descartado** (degrada en silencio) |
| **3** | `compactor_provider`: ¿derivado o persistido? | **Persistir el par** `(compactor_provider, compactor_model)` (campo hermano en las 3 structs de sesión). Capturado en la selección, nunca re-derivado |
| **4** | CLI sin validación / dropdown puede mostrar modelos no llamables | **S1 (MUST):** el **predicado** filtra proactivamente el dropdown por credencial (consumiendo la introspección `trogon.proxy.providers` como fuente); **S3** validación server-side + error claro como red de seguridad |
| **5** | Ambigüedad de id OpenRouter / dedup | **Resuelto por diseño:** selección **provider-qualified** (la opción lleva proveedor) + CLI resuelve-o-error-ante-ambigüedad. Sin lógica de dedup |
| **6** | Modelo por defecto | **Decidido:** default = modelo de sesión; override opcional, mismo o cualquier proveedor (ver §1) |
| **A** | Estimación de tokens `chars/4` agnóstica → fit no exacto en runtime | **Capacidad decidida en la selección (M3)** con ventanas exactas del catálogo S1 (ventana-vs-ventana, sin estimar contenido); **M4 = red reactiva** (fallback al de sesión ante cualquier error residual). La correctitud no depende del `chars/4` |
| **B** | Cómo se avisa del fallback de M4 | **M4:** `fallback_model` en respuesta (manual) + `SessionNotification` (background) |
| **C** | `agent_type` Claude (`"claude"`) ≠ proveedor compactor (`"anthropic"`) | **M3:** proveedor de sesión desde la **config/identidad del runner** (ya explícito por binario) + `model→proveedor` desde **catálogo/codec (S1)**. El **registry no participa** en compactación → **no se toca `AgentCapability`** (evita duplicar S1 y migrar ese KV a protobuf) |
| **D** | Embebido `trogon-acp/agent.rs` sin registry → no construye dropdown cross-provider | **Resuelto (S1 es MUST):** `build_config_options` lee el catálogo cacheado → los ~9 puntos correctos en el origen; **Claude embebido cross-provider desde el inicio**, sin fork |
| **E** | Switch de modelo cross-runner pierde el `compactor_model` (y **toda** la config de sesión) | **S6:** la causa raíz excede compactación — `session/export`-`import` solo lleva mensajes. Solución correcta = **contrato portable con config (v3)**, trabajo **separado** del que compactación depende; aquí solo su porción (`compactor_model`/`provider` en v3) |

## 5. Tratamiento por prioridad (MUST / SHOULD / COULD)

### MUST — correctitud y objetivo (sin esto no se envía)

**S1 · Catálogo de modelos NATS-backed (keystone)** — *fundación; el dropdown (M3/M3b) y las ventanas (M4) salen de aquí.*
- **Store NATS-backed**, separado del `registry` (liveness) y fuera del `secret-proxy` (stateless).
  Los **records KV se codifican desde un contrato Protobuf versionado** (ADR 0009) —
  `proto/trogonai/catalog/v1/` (`CatalogEntry { provider, context_window, max_output, modality, … }`,
  tipos generados en **`trogonai-catalog-proto`**)— con info de tipo/versión en la envoltura para
  seleccionar el decoder en migraciones.
- **Servicio poblador `trogonai-catalog` (un Service, ADR 0001):** lifecycle propio, identidad OTel
  `service.name = "trogonai-catalog"`, en `services/` (futuro). Fetch-ea `/models` de cada proveedor **vía el `secret-proxy`** (egress externo
  sancionado vía el secret-proxy), cachea con **TTL** + tabla lateral para ventanas que
  el API no exponga (sembrable desde una BD externa tipo LiteLLM); consolida los
  `context_window_tokens` triplicados (`xai-runner:88`, `openrouter:155`, `acp:50`). Config por **TOML
  + precedencia** (ADR 0007): `catalog_ttl`, etc.
- **Crate cliente `trogonai-catalog-client`** (sufijo `-client`, prefijo `trogonai-*` por ser
  producto-específico; patrón `registry`) que resuelve `model → (proveedor, ventana, max_output)`,
  consumido por runners, compactor y el `trogon-acp` embebido (`build_config_options` lee la caché).
  Es **reusable crate → no depende del Service poblador** (lee del backbone NATS), ADR 0001.
- **Dos primitivas compartidas (clave para CLI≡IDE): una sola fuente de verdad del filtro y del
  formato.** Para que el picker ofrezca **lo mismo** en CLI e IDE **por construcción** (no por
  disciplina), el crate cliente expone:
  - **Predicado de filtro** `compactable_models(session_window) -> [modelos]` — **función pura**
    sobre el snapshot cacheado (sirve al path **embebido sync**), dueña de **los tres filtros
    proactivos**: **tipo** (text-output) + **capacidad** (`ventana ≥ session_window × margen`, con el
    **margen como su constante única**) + **credencial** (solo proveedores llamables, leyendo el set
    de `trogon.proxy.providers` cacheado junto al catálogo). Que **posea** los tres —en vez de dejar
    la credencial a cada builder— evita que un builder **se olvide** de aplicarla → garantiza **mismo
    conjunto** en CLI e IDE en **las tres** dimensiones. *(La credencial es dinámica → su red
    **reactiva** sigue siendo S3; aquí solo el filtro proactivo del picker.)*
    **Degradación si falta el snapshot de credencial** (introspección caída/stale, arranque en frío):
    **nunca fail-open** (ofrecer sin filtrar rompería el contrato "lo que ves se puede usar" → S3
    rechazaría la elección). Se cae al **subconjunto probadamente llamable**, piso = **solo el Default
    (modelo de sesión)**, que **siempre** funciona; las opciones cross-provider vuelven al
    restablecerse el snapshot. *(Opcional bajo S5: también los modelos del proveedor de la sesión,
    llamables porque la sesión usa esa credencial ahora.)*
  - **Codec provider-qualified** `qualify(prov, model)` / `parse(value)` / `resolve(id)` — **contrato
    productor↔consumidor** (lo producen los builders del IDE **y** el CLI, lo parsea el handler, se
    persiste). Compartirlo es **funcional**, no cosmético: si el formato deriva, el parser **rompe**.
    **Dueño también del sentinel del default:** el value `""` de la opción "Default (modelo de
    sesión)" lo mapea el codec a **`None`** en el borde UI→dominio (`parse("") → None`; `qualify`
    nunca emite `Some("")`). Como el codec es el **único productor**, `Some("")` **no se filtra** al
    wire ni a `resolve_model` (`compactor_model.or(model)` recibiría `Some("")` como valor real y
    rompería el default). *(Blindaje opcional: tipar la elección —enum/`Option` de value-object— hace
    `Some("")` inrepresentable; idiomático pero no necesario dado el chokepoint del codec.)*
  - **Ambas las consumen CLI e IDE**; cada superficie solo **renderiza** (el IDE como
    `SessionConfigSelectOption` — opcionalmente vía un option-builder común a sus 3 builders; el CLI
    como listado/autocomplete). **Ningún builder reimplementa filtro, margen ni formato.**
- **Introspección de credenciales (la FUENTE):** subject `trogon.proxy.providers` indica qué
  proveedores son llamables. Es el **dato** que cachea el crate cliente y que **consume el predicado**
  para su filtro de credencial (Gap 4) — el filtrado en sí lo hace el predicado, no este subject. El
  mapeo `model → proveedor` lo hace el **codec** (`resolve`), sin inferencia.
- **Filtro por tipo de modelo (compactación):** el catálogo **excluye los modelos hechos para otra
  cosa** —**no-texto** (embeddings, generación de imagen) y **especializados** (moderación,
  clasificación)— de las opciones de compactación. **No aparecen ni en el CLI ni en el IDE.** Es
  derivable de la modalidad/tipo que reporta `/models` (no requiere curación). *(Distinto de los
  modelos de lenguaje flojos/base que sí dan texto: esos no se excluyen aquí.)*
- Descarta **B** (en el `registry`: mezcla efímero/maestro) y **C** (caché por proceso: saca el dato de NATS).

**M1 · Wire: separar proveedor de compactación del de sesión** — *base de todo, retrocompatible (modelo protobuf).*
- **Contrato `.proto` (ADR 0009):** el mensaje del compactor es un **internal backbone message** →
  contrato **Protocol Buffers versionado** bajo `proto/trogonai/compactor/v1/`, **encoding binario**.
  Como tocamos el contrato, se **migra de serde JSON a `.proto`** (ADR 0009 sanciona migrar al tocar).
  Añadir el proveedor del compactador con un **número de campo nuevo**:
  ```proto
  // proto/trogonai/compactor/v1/compactor.proto
  message CompactRequest {
    // … campos existentes (1..N) …
    optional string compactor_provider = 0xN;  // vacío/ausente → usa `provider` (sesión)
  }
  ```
- El tipo generado se convierte a un **value-object de dominio en el borde** (`service.rs`); en
  `handle()`, `compactor_provider` (si presente) elige el `ProviderConfig`, ausente → `provider`
  (sesión). `resolve_model` se mantiene.
- **No se añade la ventana del compactador al contrato.** El fit se decide en la **selección**
  (picker M3, en cliente, leyendo el catálogo) y la garantía es **M4 reactivo** (intentar →
  fallback al de sesión ante cualquier error). El compactor no necesita la ventana del modelo
  elegido para nada portante; añadirla implicaría lógica de fit server-side (la que M4 elimina).
- `CompactResponse` (mismo `.proto`) gana `optional string fallback_model = 0xM;` para señalar el
  fallback (M4) — el runner lo surface al usuario.
- **Compat (modelo protobuf):** campos nuevos con **números nuevos**, los retirados se **reservan**;
  lectores viejos ignoran lo desconocido → ausencia = comportamiento idéntico. (Sustituye al
  `#[serde(default)]` del enfoque JSON. La migración JSON→protobuf del contrato existente es
  coordinada por ser backbone; el encoding binario es interno.)

**M2 · acp-runner participa (Gap 1)** — *requisito directo; es el runner por defecto.*
- `trogon-acp-runner/src/agent.rs`: añadir `compactor_provider: Option<&str>` a `CompactReq`
  (`:145`) y a `build_compact_payload`/`compact_messages` (`:172`, `:198`); quitar el
  hardcode implícito de proveedor.
- Resolver y pasar el proveedor en los **2 call sites**: `:465` (pre-turn) y `:1413`
  (ext_method `/compact`). acp **no** tiene post-turn.

**M3 · Selección cross-provider provider-qualified + persistencia (Gaps 1, 3, 5)** — *el usuario elige; sin ambigüedad.*
- **Builders del dropdown — todos consumen las primitivas compartidas (S1), no reimplementan:**
  el builder de la opción `compactor_model` en cada componente —en los runners
  `compactor_model_config_option` (`xai-runner/agent.rs:893` y equivalente en openrouter); en el
  `trogon-acp` embebido `build_config_options` (`agent.rs:166`)— obtiene la lista llamando al
  **predicado compartido** `compactable_models(session_window)` (S1) y el value vía el **codec
  compartido**, no desde su `registry` handle ni con filtro propio. El **filtro por credencial es
  parte del predicado** (S1), no del builder → ningún builder puede olvidarlo. Unifica los builders
  —el embebido ya no es la excepción (Gap D resuelto) y **multi_runner ya no augmenta** respuestas.
  **El CLI consume las mismas primitivas** (predicado + codec) → CLI e IDE ofrecen **el mismo
  conjunto, por construcción** en tipo, capacidad **y** credencial (Gap 1 del picker).
- **Provider-qualified (Gap 5) — vía el codec compartido:** el value del `SessionConfigSelectOption`
  lleva proveedor + modelo (p.ej. `"anthropic::claude-haiku-4-5-20251001"`), **producido y parseado
  por el codec** (S1) que comparten builders, handler, CLI y persistencia — un solo formato, sin
  deriva. **No** se infiere el proveedor del string del modelo. `find_by_model` se queda solo para el
  switch de `/model`, no para el compactor.
- **CLI — selección + descubrimiento + autocompletado (MUST):** `do_compact_model`
  (`repl.rs:1407`) acepta id desnudo y lo resuelve (duplicado real → error pidiendo cualificar
  `provider:model`); un **listado catalog-backed** de modelos de compactación; y
  **autocompletado/fuzzy-search** de `/compact-model` (Tab-completion + filtro en vivo) desde el
  catálogo. Así "cualquier modelo" es *alcanzable y cómodo* en el CLI (los cientos de OpenRouter
  lo exigen, y el objetivo es seleccionar cualquier modelo **también** en el CLI). Comparte el
  catálogo con el listado de modelos de sesión (`/models`) — misma fuente, eficiencia.
- **Persistencia (Gap 3):** el valor persistido de sesión es un **record KV codificado desde un
  contrato Protobuf versionado** (ADR 0009), no una struct serde — añadir el campo hermano
  `compactor_provider` (número de campo nuevo) junto a `compactor_model` en el `.proto` de sesión, que
  reemplaza las structs serde de `xai-runner/session_store.rs:26`,
  `openrouter-runner/session_store.rs:22`, `runner-tools/session_store.rs:83`. Se persiste el par
  capturado; el resume lee ambos. *(Migración de records viejos: C4.)*
- **Lista de modelos:** desde el **predicado (S1)** — "cualquier modelo" (amplitud OpenRouter),
  ya filtrado por tipo + capacidad + credencial. La opción **`""` = "Default (modelo de sesión)"**
  la mapea el **codec a `None`** en el borde (nunca llega `Some("")` al wire — ver S1).
- **Política del picker (decidido): mostrar solo los usables, con nota en positivo del porqué.**
  El picker (CLI **e** IDE) muestra **solo los modelos que se pueden usar para compactar esta
  sesión** (**text-output** con ventana suficiente; no-texto/especializados ya fuera en S1), con una
  **nota en positivo** que explica el criterio — p.ej. *"Estos son los modelos con ventana
  suficiente para compactar esta sesión."* Los demás **no se muestran** (no se listan las
  ausencias). Así el picker queda **limpio** ("lo que ves se puede usar") y la nota **explica por
  qué son esos**, sin el ruido de enumerar los que faltan.
- **Capacidad suficiente = ventana ≥ ventana del modelo de sesión (× margen).** Un modelo se
  ofrece si su ventana **≥ la ventana del modelo de sesión × margen** (constante conservadora,
  ~1.2). Comparación **exacta ventana-vs-ventana** (números del catálogo), **sin estimar el
  contenido**. Por construcción el modelo ofrecido **cabe con holgura** → el caso normal es que
  **el modelo elegido compacte él mismo**. *(La garantía de "sin error" no la da este filtro, sino
  **M4** — ver abajo.)*
- **El margen absorbe la diferencia entre tokenizers.** La ventana del compactador y la de sesión
  están en tokens de **tokenizers distintos** (el mismo contenido es distinto nº de tokens en cada
  modelo) → sin margen, "≥ sesión" no sería airtight.
- **El margen es un mando, no una garantía:** constante **conservadora y configurable** (no un
  número sagrado). Es un **pre-filtro grueso a propósito** — afina la frecuencia de fallback vs la
  amplitud de opciones, pero **no es portante**: la garantía de no-error la da **M4**. Se
  **autocorrige por ops** (si excluye modelos válidos, se baja; si deja pasar demasiados, se sube),
  sin tocar código. **No** se sobre-ingenieriza: una tabla de densidad por tokenizer es gold-plating
  y queda como **contingencia solo si los datos lo piden** (ver §9).
- **Dos lecturas de "sin error":** (a) *que el usuario nunca vea un error* → **100% por M4** (el
  fallback es éxito con aviso, no fallo); (b) *que el modelo elegido compacte él mismo, nunca el
  fallback* → **garantía práctica** (el filtro la maximiza; el 100% real exigiría conteo exacto por
  modelo, impráctico para los cientos de OpenRouter). El objetivo "los seleccionables compactan sin
  error" se cumple **entero en (a)**; (b) es práctica, con M4 volviendo inofensivo el residuo.
- **Umbral estable:** se mide contra la **ventana del modelo de sesión** (fija), **NO** contra el
  tamaño **actual** → la lista **no baila** mientras chateas (solo cambia si cambias el modelo de
  sesión a uno de ventana distinta — correcto).
- **Trigger:** la compactación dispara al **límite del modelo de sesión** (~85% de su ventana),
  **no** del modelo de compactación — su propósito es que la conversación no desborde el modelo
  que la corre. (El de compactación es la herramienta del resumen, no decide el *cuándo*.)
- **M4 como red residual:** cubre el residuo patológico rarísimo (tokenización extrema que excede
  el margen, credencial perdida en runtime) → fallback al modelo de sesión + aviso. Da el
  **no-error garantizado** encima de la garantía práctica del picker.
- **Orden del picker: no se impone uno por defecto.** Los modelos ofrecidos (todos capaces) pueden
  listarse en **cualquier orden** razonable (alfabético, por proveedor, por catálogo) — **no** se
  fuerza un orden por precio ni ningún otro por defecto. Así el picker **no depende** de que el
  catálogo traiga precio (no todos los `/models` lo exponen). El ahorro lo decide el usuario al
  elegir el modelo, no el orden de la lista.
- **Gap C · Proveedor del compactor sin inferir de `agent_type`.** Verificado: Claude registra
  `agent_type="claude"` (configurable, `acp-runner/main.rs:96`) pero el proveedor del compactor
  es `"anthropic"`; xAI/OpenRouter ya coinciden. Solución correcta (consistente con Gap 5 —
  **eliminar la inferencia**) y **sin tocar el registry** (el `registry`/`AgentCapability` es infra
  compartida; meter `provider` ahí duplicaría el `model→proveedor` que ya da S1 y obligaría a migrar
  ese KV a protobuf, ADR 0009):
  - **Proveedor de sesión:** lo pone el runner en el wire desde su **propia config/identidad** —ya
    es explícito por binario (p.ej. acp-runner hardcodea `"anthropic"` en `build_compact_payload`)—,
    no inferido de `agent_type`.
  - **`compactor_provider`:** lo da el **codec** provider-qualified (ya en el wire).
  - **`model → proveedor`** en cualquier otro punto: el **catálogo (S1)**, fuente única.
  - → En compactación, **ambos proveedores viajan en el wire**; el compactor rutea sin registry ni
    lookup. El `registry`/`find_by_model` se queda **solo** para el switch de `/model`. Sin caveat de
    rollout (no hay campo que registrar). *(Si la plataforma quisiera `runner→proveedor` canónico en
    el registry, es decisión del registry, con su propia migración protobuf — fuera de este plan.)*
- Call sites de compactación que pueblan el wire:
  - xai-runner: `src/compaction.rs` + `agent.rs:1583`, `:2531`, `:2664`.
  - openrouter-runner: `src/compaction.rs` + `agent.rs:1766`, `:2315`, `:1572`.
  - acp-runner: los de M2.

**M3b · Gap D · El embebido sin registry — auditoría de puntos de emisión de `config_options`.**
Verificado que `compactor_model` llega a la IDE por **~9 puntos**, todos vía
`TrogonAcpAgent::build_config_options` (`agent.rs:166`):
- **Respuestas (5):** `new_session` (`agent.rs:817`), `load_session`/`resume_session`/`fork_session`
  (`:855`, `:1238`, `:1275`), `set_session_config_option` (`:1018`). Emitidas por el embebido,
  devueltas por multi_runner (tiene registry → augmentables).
- **Notificaciones `ConfigOptionUpdate` (3):** `agent.rs:917` (mode), `:1012` (config), `:1053`
  (model). Emitidas por el embebido → fluyen por el relay de `main.rs` (`:303-316`), que hoy
  **solo remapea `session_id`, no toca contenido**.
- **Replay (1):** `main.rs:442-449` construye su propia `ConfigOptionUpdate` (tiene registry).

**Resolución (S1 es MUST):** la augmentación dispersa (9 puntos, 2 archivos, + volver el relay
content-aware) es frágil e invasiva → se descarta. Como **S1 es MUST**, `build_config_options`
(`agent.rs:166`) lee el catálogo (cacheado en el estado del agente, refrescado por TTL — **no**
query NATS síncrona, la función es sync) y construye la opción cross-provider **en el origen**,
dejando los 9 puntos correctos de golpe. **El Claude embebido tiene cross-provider desde el inicio**
— ya no hay limitación same-provider ni fork.

**M4 · Red reactiva sobre la garantía del picker (Gap 2)** — *único gap que podría fallar en runtime; con M3 estricto, residual.*

La **decisión de capacidad ya se tomó en la selección** (M3): el picker solo ofrece modelos con
**ventana ≥ sesión × margen** (constante conservadora), comparación **exacta ventana-vs-ventana**
del catálogo. Por eso todo modelo elegible **cabe por construcción** y M4 **no re-evalúa zonas ni
decide si intentar** — es la **red reactiva** para el residuo que la garantía *práctica* no cubre
(tokenización extrema que excede el margen, credencial perdida en runtime, error transitorio).

- **(1) Intentar el modelo elegido (cabe por construcción).** El picker garantizó la ventana; M4
  lanza la compactación con el modelo elegido **sin pre-evaluar zonas**.
- **(2) Catch reactivo = la red.** Ante **cualquier error** de esa llamada → **fallback al modelo
  de sesión** (`provider`/`model`, ya en la request) + `fallback_model` en la respuesta + aviso.
  Correcto **independiente de toda estimación**.
  - *Fundamento (verificado):* `to_summarize ≈ 0.75 × ventana_sesión`, así que el modelo de
    sesión **siempre cabe a una pasada, full-fidelity**. No hay cadena infinita. Si el de sesión
    también fallara → degradación elegante (historia original).
- **Sin zonas proactivas ni guard server-side en M4.** La antigua estrategia "por zonas / sesgada a
  intentar" asumía un picker **permisivo** (modelos ofrecidos que podían no caber). Con M3 estricto
  eso se movió a la **selección**: no hay "mismatch grueso" que saltar (no se ofreció) ni "borderline
  que intentar" (todo lleva margen). M4 no descarta nada — **solo reacciona al error residual**. Si
  alguna vez se quisiera saltar preventivamente una llamada condenada, el sitio correcto es el
  **cliente** (que ya tiene catálogo + picker), no un guard server-side alimentado por el wire.
- **Chunking jerárquico DESCARTADO:** plegar resúmenes degrada la fidelidad **en silencio** y es
  más complejo que el fallback.
- Sitio: `trogon-compactor/src/lib.rs` (`compact_if_needed_counted`) lanza la compactación y aplica
  el catch alrededor de `generate_summary`.

  **Gap A (resuelto):** la imprecisión de `chars/4` no rompe nada — la **capacidad se decide en la
  selección** con ventanas **exactas** del catálogo (S1, MUST), comparación ventana-vs-ventana **sin
  estimar contenido**; y el catch reactivo es la garantía final ante cualquier residuo. La
  correctitud no depende de la precisión del conteo.

  **Gap B · Cómo se avisa del fallback.** Canal según el disparo (mecanismo verificado en
  `agent.rs:917,1012,1053`):
  - *`/compact` manual:* `fallback_model` en `CompactResponse` → `CompactResult` → CLI imprime / IDE muestra.
  - *Auto-compactación (background):* el runner emite una **`SessionNotification`**
    (`ConfigOptionUpdate`/mensaje) — única vía por la que el usuario ve un fallback en background.

### SHOULD — calidad y robustez

> **S1 (catálogo keystone) se promovió a MUST** — ver arriba. El dropdown y las ventanas se
> construyen desde el catálogo desde el inicio; no hay "camino ligero" de listas curadas.

**S2 · Helper compartido de compactación (Gap 4 técnico/DRY)** — *respetando diferencias reales.*
- Extraer en `trogon-runner-tools/src/compaction.rs` un helper fino: construir
  `CompactRequest` + resolver proveedor + llamada NATS.
- **NO** consolidar la conversión de mensajes ni el *cuándo*: xai es **stateful**
  (post-turn, limpia `previous_response_id`), openrouter **stateless** (pre-request, reusa
  cola), acp pre-turn + ext_method. Esas diferencias se quedan per-runner.

**S3 · Validación server-side + degradación (Gap 4)** — *fuente de verdad única.*
- El compactor/runner rechaza `(proveedor, modelo)` desconocido o sin credencial con error
  claro; CLI **y** IDE lo relayan.
- Degradación elegante (devolver historia original) — ya es principio del compactor.
- **Relación con S1:** como S1 (MUST) ya filtra el dropdown proactivamente a proveedores con
  credencial, S3 es la **red de seguridad reactiva** — atrapa cualquier `(proveedor, modelo)`
  que se cuele (credencial perdida en runtime, carrera) con error claro + degradación.

**S4 · Telemetría de compactación (billing)** — *observabilidad, no código de negocio.*
- Emitir métricas **vía OpenTelemetry** (ADR 0008): metric instruments + semantic conventions,
  atributos de recurso (`service.name` del compactor/poblador), export **OTLP**; setup compartido en
  **`trogon-telemetry`**, instruments en el módulo `telemetry/` del paquete que los emite. Etiquetar
  proveedor + modelo + tokens (y `fallback_model` si lo hubo) de cada compactación, para atribución de
  coste. **No** introducir un SDK de vendor como boundary primario.

**S5 · Compactor vía secret-proxy (lazo de credenciales)** — *lo seleccionable = lo llamable.*
- Apuntar `*_BASE_URL` del compactor al proxy (`{proxy}/<provider>/v1`, ya soportado en
  `main.rs:69-142`) y hacerlo el modo por defecto del despliegue, para que "proveedor con
  credencial" = "modelo compactable".

**S6 · Preservar `compactor_model` en el switch cross-runner (Gap E)** — *no perder el override en silencio.*
- **Causa raíz (verificada, excede compactación):** `session/export`/`session/import`
  (`cross_runner.rs:40-135`) transportan **solo mensajes** (`PortableExportV2` =
  `version` + `messages`). El switch cross-runner **pierde toda la config de sesión** —
  `mode`, `mcp_servers`, `system_prompt`, `compactor_model`, `title`
  (`runner-tools/session_store.rs:75-104`) — no solo el compactor.
- **Solución correcta (trabajo SEPARADO, dependencia):** **contrato portable con config (v3)**:
  `export` devuelve `{messages, config}`; cada runner serializa su config portable
  (`mode`, `compactor_model`/`provider`, `mcp_servers`, `system_prompt`); `import` aplica lo
  que entiende e ignora lo desconocido (version bump v2→v3, compat). Solo el **runner destino**
  puede aplicar correctamente (p.ej. **reconectar** los `mcp_servers`), por eso vive en el
  contrato/runner, **no** en el `CrossRunnerSwitcher` (agnóstico por diseño, `cross_runner.rs:84`).
  > ⚠️ Este contrato v3 **excede el alcance de compactación** (afecta MCP, system prompt, mode).
  > Merece su **propio diseño/issue**; compactación **depende** de él pero no lo posee.
- **Porción que sí posee compactación:** asegurar que `compactor_model` + `compactor_provider`
  estén en la config portable v3.
- **Mitigación táctica (solo si v3 se difiere):** que el `CrossRunnerSwitcher` re-aplique
  `compactor_model` tras el import vía `set_session_config_option`, **o** que se avise al
  usuario de que el override se reinició. Nunca perderlo en silencio.

### COULD — polish posterior

- **C1 · Fallback = modelo de sesión (igual que el default).** El fallback de M4 cae al **modelo
  de sesión** — el mismo que el comportamiento por defecto. **No** se intenta un "más barato que
  quepa": el de sesión **siempre cabe** (`to_summarize ≈ 0.75 × su ventana`) full-fidelity, es
  **predecible** (un único destino de fallback, no una heurística de coste) y **consistente** con
  el default. El ahorro vive en la **elección deliberada del usuario** en el picker, no en el
  camino de error.
- **C3 · `max_summary_tokens` por modelo** desde el catálogo (hoy fijo en `main.rs`).
- **C4 · Migración** de sesiones persistidas con `compactor_model` (string desnudo, sin
  proveedor) anteriores a M3: resolver el proveedor al cargar y reescribir el par.

## 6. Cambios de wire (resumen)

Contrato **Protobuf versionado** (ADR 0009), `proto/trogonai/compactor/v1/compactor.proto` (tipos
generados en **`trogonai-compactor-proto`**), encoding **binario** en el path interno. Compat por
números de campo (no `#[serde(default)]`):
```proto
message CompactRequest {
  repeated Message messages          = 1;
  string  provider                   = 2;  // proveedor de la sesión
  string  model                      = 3;  // modelo de la sesión (y fallback de M4)
  uint64  context_window             = 4;  // ventana de la sesión (y fallback de M4)
  optional string compactor_provider = 5;  // NUEVO (M1) — proveedor del modelo compactador
  optional string compactor_model    = 6;  // NUEVO — vacío/ausente → Default (modelo de sesión)
  // NO se añade la ventana del compactador: el fit se decide en la selección (picker M3, en
  // cliente) y la garantía es M4 reactivo. El compactor no la necesita para nada portante.
}
message CompactResponse {
  // … campos existentes …
  optional string fallback_model = 7;      // M4 — el runner lo surface al usuario
}
```
*(Números ilustrativos; se asignan tras los campos ya existentes. Tipos generados → value-objects de
dominio en el borde, ADR 0009.)*

## 7. Pruebas

- **Unit** (`trogon-compactor/src/service.rs`): `compactor_provider` elige el
  `ProviderConfig` correcto; `None` → fallback a `provider` (compat).
- **Unit** (S1 predicado): `compactable_models(session_window)` aplica **los tres** filtros —tipo
  (no-texto fuera), capacidad (ventana ≥ sesión × margen), credencial (proveedor sin credencial
  fuera)—; el mismo input da el **mismo set** (base de la consistencia CLI≡IDE).
- **Unit** (S1 codec): `parse("") → None` y `qualify` nunca emite `Some("")` → el sentinel "Default"
  no se filtra al wire; `parse("prov::model")` → par; id ambiguo → error.
- **Unit** (`lib.rs`/`detector.rs`) M4 red reactiva: **error del compactador** → fallback al modelo
  de sesión + `fallback_model`; el modelo de sesión siempre cabe (no hay cadena infinita).
- **Unit** provider-qualified (M3): value `"prov::model"` se parte y persiste como par; CLI
  con id ambiguo → error.
- **Unit** resolutor (S1): `model → proveedor + ventana`; fallback para desconocidos.
- **Integración** (S1): servicio-catálogo puebla desde `/models` mockeado vía proxy; introspección
  de proveedores; crate cliente resuelve `model→(proveedor,ventana)`; dropdown filtrado a
  proveedores con credencial; TTL de caché.
- **Integración multi-runner** (`trogon-acp`): dropdown con modelos de >1 runner en los
  ~9 puntos de emisión de `config_options` (Gap D); elegir cross-provider produce
  `CompactRequest` con `compactor_provider` correcto (incl. acp-runner y el caso embebido).
- **Unit** (Gap A): las ventanas vienen del catálogo (S1); además, ante ventana errónea, el
  **catch reactivo** sigue dando el resultado correcto (fallback al modelo de sesión) — la
  correctitud no depende de la exactitud del `chars/4`.
- **Unit** (Gap C): el runner pone su proveedor de sesión en el wire desde su config/identidad (no
  de `agent_type`); el compactor rutea con `provider`/`compactor_provider` del wire, sin registry.
- **E2E live** (extender `live_*` de xai-runner): sesión en un proveedor, compactación con
  modelo de otro; y caso fallback (compactador chico → modelo de sesión).
- **Compat (protobuf):** un `CompactRequest` sin los campos nuevos (números 5/6 ausentes) decodifica
  y se comporta idéntico; los números de campo retirados quedan **reservados**. Migración del contrato
  JSON existente → protobuf cubierta por el cambio de wire (§6).

## 8. Orden de entrega

1. **MUST** (S1 → M1 → M2/M3/M3b/M4): el **catálogo NATS-backed (S1) es la fundación** y va
   primero; sobre él, wire + acp-runner + selección provider-qualified + estrategia de ventana.
   Entrega **"cualquier modelo de cualquier proveedor" en CLI e IDE** (incl. Claude embebido),
   sin limitación same-provider. Es el incremento enviable.
2. **SHOULD** (S2–S6): helper compartido, validación/telemetría, compactor vía proxy, y S6
   (preservar config en switch cross-runner). **S6 depende del contrato portable v3** (trabajo
   separado); si v3 se difiere, S6 entra como mitigación táctica (re-aplicar/avisar).
3. **COULD**: fallback = modelo de sesión (C1, igual que el default), `max_summary` por modelo, migración.

> Con **S1 en el MUST**, el objetivo literal ("cualquier modelo de cualquier proveedor") se
> entrega de una vez en CLI e IDE, incluido el Claude embebido. Ya no hay fase de "listas
> curadas" ni fork de keystone — el catálogo dinámico es el núcleo.

## 9. Riesgos y decisiones abiertas

- **Keystone (S1) = MUST, decidido.** El catálogo NATS-backed es la fundación; ya no es un fork
  abierto. Coste asumido: hay que construir el servicio-catálogo + crate cliente **antes** de
  enviar (no hay incremento "ligero" previo). Es el precio de entregar "cualquier modelo" de una.
- **Fallback M4 usa un modelo distinto al elegido** en el caso extremo (el de sesión) — avisado
  vía `fallback_model`. Es el coste honesto de "siempre funciona + full-fidelity". El destino del
  fallback es **siempre el modelo de sesión** (= el default, C1): predecible y consistente, no una
  heurística de coste en el camino de error.
- **Margen del filtro de capacidad = mando conservador, no portante (decidido).** El umbral
  "ventana ≥ sesión × margen" usa una **constante conservadora y configurable** (~1.2 inicial),
  **no** un valor calibrado: es un **pre-filtro grueso**, y la garantía de no-error la da **M4**.
  Config por **TOML + precedencia** (ADR 0007): nombre canónico `margin` (TOML `margin`, env
  `TROGON_MARGIN`, CLI `--margin`; defaults < TOML < env < CLI). Por eso el margen solo afina
  fallback-vs-amplitud de opciones y **se autocorrige por ops** (subir/bajar config, sin código). La **tabla de densidad empírica por familia de tokenizer** (fit-check
  en caracteres) queda como **contingencia: solo si producción muestra que la constante excluye un
  nº significativo de modelos válidos**. Hasta entonces, calibrar es resolver un problema que no se
  tiene (falsa precisión sobre entradas ya aproximadas: `to_summarize ≈ 0.75×`, trigger ~85%).
- **Auto-guard server-side del compactor (mejora diferida, YAGNI).** Hoy el compactor es **mínimo**:
  rutea por proveedor + **M4 reactivo** (la correctitud **no** necesita que conozca ventanas; el fit
  se decide en la selección/picker). Si algún día aparece un caller de **alto volumen sin picker**
  (p.ej. una automation), el compactor puede autoprotegerse leyendo la ventana del modelo compactador
  **del catálogo** (no del wire) para saltar preventivamente llamadas condenadas — **añadible sin
  cambio de contrato** (ya es consumidor del catálogo). Hasta que ese caller exista, **no se
  construye** (beneficio especulativo, coste de diferir ≈ cero).
- **Latencia/coste de `/models`:** mitigado con cache TTL; decidir el TTL (`catalog_ttl`, vía TOML +
  precedencia, ADR 0007).
- **Ventanas incompletas en `/models`:** tabla lateral estática como respaldo.
- **Despliegue:** el compactor debe tener credencial de los mismos proveedores que los
  runners (S5 lo formaliza vía proxy).
- **Contrato portable v3 (Gap E / S6) es cross-cutting:** el switch cross-runner pierde
  **toda** la config de sesión (MCP, system prompt, mode, compactor_model), no solo la
  compactación. La solución correcta excede este plan y merece diseño propio; compactación
  solo aporta su porción (`compactor_model`/`provider` en v3) y depende del resto.

## 10. Lo que se descarta y por qué

- **Chunking / resumen jerárquico (Gap 2):** degrada la fidelidad **en silencio** (lo peor
  para calidad de producto) y es más complejo que el fallback al modelo de sesión (M4).
- **Derivar el proveedor del string del modelo:** frágil (depende de convenciones de id no
  impuestas). Se captura en la selección y se persiste (Gaps 3/5).
- **Lógica de dedup de modelos:** rutas distintas (Claude directo vs vía OpenRouter) son
  intencionales; el value provider-qualified evita la ambigüedad sin deduplicar.
- **Registry como catálogo de "cualquier modelo":** solo expone ~3 modelos curados por
  runner. Sirve para `model → runner`, no para amplitud OpenRouter (eso lo da el catálogo S1).
- **Catálogo estático compilado:** se pudre y exige redeploy por cada modelo nuevo.
- **Augmentar `config_options` en los ~9 puntos de emisión (Gap D):** frágil (cualquier punto
  olvidado muestra same-provider) e invasivo (volver content-aware el relay de notificaciones
  de `main.rs`). Se prefiere S1 (corregir en el origen, `build_config_options`).
- **Inferir el proveedor del `agent_type` (Gap C):** frágil (`AGENT_TYPE` es configurable). El
  proveedor de sesión viene de la **config/identidad del runner** y `model→proveedor` del **catálogo
  (S1)** — **sin tocar el registry** (evita duplicar S1 y migrar `AgentCapability` a protobuf).
- **Chunking para distinguir el error de overflow (Gap A):** innecesario parsear errores
  provider-specific; basta el fallback al modelo de sesión ante cualquier error.
- **Consolidar los 3 runners en un solo path:** sus diferencias stateful/stateless son
  reales; solo se comparte el helper fino (S2).
- **Capa de validación previa nueva:** innecesaria; basta la degradación elegante que el
  compactor ya tiene + error claro (S3).
