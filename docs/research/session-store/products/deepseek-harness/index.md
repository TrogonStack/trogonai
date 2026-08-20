# DeepSeek Harness: how session transcripts are stored and resumed

Part of [Session Store Research](../../index.md).

Produced by running [RESEARCH_PROMPT](../../RESEARCH_PROMPT.md).

- Repository: [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)
- Snapshot: [`141eb6fef83422698aef7a981029e843e8161534`](https://github.com/deepseek-ai/deepseek-harness/commit/141eb6fef83422698aef7a981029e843e8161534)
- Retrieved: 2026-08-20
- Evidence boundary: repository source and generated repository documentation at the pinned commit
- Verification note: this is static source research. Upstream tests were not run.

DeepSeek Harness separates three concerns that other products often call one session store:

1. `Session` and `SessionStore` own the live, process-local source of truth. [`packages/core/session/src/index.ts:417-472`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L417-L472)
2. `SessionPersistence` is an optional durability seam that subscribes to live events and persists the same event log. [`packages/core/session/src/index.ts:786-794`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L786-L794)
3. Projection and query packages derive read models from live or persisted logs. Their caches and indexes are explicitly non-authoritative. [`packages/session/session-projection-cache/src/index.ts:1-12`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L1-L12)

This boundary is stated directly in the runtime: `SessionStore` is an in-memory map and persistence is intentionally implemented by subscribing plugins. [`packages/core/session/src/index.ts:786-794`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L786-L794) The persistence package likewise identifies the existing `SessionEvent` as the persisted unit and the event log as the single source of truth. [`packages/session/session-persistence/README.md:5-7`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L5-L7)

## The storage model

### Authoritative runtime state

`Session` owns an append-only in-memory `SessionEvent[]`, an immutable `SessionHeader`, and an incrementally maintained model-visible surface. A live append is synchronous and I/O-free: it allocates `seq` from the current log length, stamps `Date.now()`, validates the payload and surface operation, freezes the event, pushes it to the log, updates the surface, and then notifies observers. Observer failures are contained after the event is already committed in memory. [`packages/core/session/src/index.ts:417-472`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L417-L472) [`packages/core/session/src/index.ts:569-655`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L569-L655)

The exact source description is:

> “An event-sourced session: an append-only log of {@link SessionEvent}s.”

[`packages/core/session/src/index.ts:417-425`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L417-L425)

`SessionStore.get()` and `SessionStore.list()` expose only currently live sessions. Detaching a `Session` removes it from the map and emits the disposal lifecycle, but does not delete any durable artifact. [`packages/core/session/src/index.ts:949-958`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L949-L958) [`packages/core/session/src/index.ts:1050-1065`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1050-L1065)

### Optional durability backends

The abstract `SessionPersistence` service stores immutable header metadata separately from the canonical event stream. The first-party coordinator supplies buffering, contiguous cursors, per-session operation serialization, cold preparation, repair sequencing, and quiescent disposal. A backend supplies physical read, append, repair, list, and optional suffix-read primitives. [`packages/session/session-persistence/src/index.ts:1-15`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L1-L15) [`packages/session/session-persistence/src/coordinator.ts:117-215`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L117-L215)

The repository ships two implementations:

| Backend | Physical source of truth | Access shape |
|---|---|---|
| JSONL | One `session.jsonl.zstd` by default, or raw `session.jsonl`, per materialized session. The first logical line is the header and later records encode every logical event. | Sequential full-file scan. `readFrom()` still parses the artifact and skips earlier events. [`packages/session/session-persistence-jsonl/README.md:5-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L5-L20) [`packages/session/session-persistence/src/index.ts:202-221`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L202-L221) |
| SQLite | A strict `sessions` metadata table plus an `events` table keyed by `(session_id, seq)`. Some physical event rows pack several logical chunk events. | Full read or seekable suffix read. [`packages/session/session-persistence-sqlite/resources/sql/schema.sql:1-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/resources/sql/schema.sql#L1-L30) [`packages/session/session-persistence-sqlite/README.md:5-19`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L5-L19) |

The best-fit conceptual model is session-as-event-log with a separate immutable header. JSONL expresses that log as a per-session file, SQLite as a keyed row set, and both decode to the same ordered logical events. [`packages/session/session-persistence/README.md:5-7`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L5-L7) [`packages/session/session-persistence-sqlite/README.md:5-19`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L5-L19)

`create()` is deliberately lazy in both first-party backends. A session with no appended event has no durable artifact or row and is absent from persistent listing. The first append atomically materializes the header and the first batch. [`packages/session/session-persistence/src/index.ts:126-143`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L126-L143) [`packages/session/session-persistence/README.md:16-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L16-L23)

### Derived projections and indexes

The model transcript is a projection, not another stored message list. `deriveMessages()` reads the session surface and caches a frozen array until the surface generation changes. Surface replacement changes this projection without mutating the underlying event log. [`packages/core/session/src/index.ts:701-747`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L701-L747)

The generic projection registry folds pure projection units over `SessionEvent`s. Each unit declares `init`, `apply`, `view`, and a `stateVersion`; the registry can snapshot all registered views and serialize per-unit checkpoints. [`packages/session/session-projection/src/index.ts:34-74`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection/src/index.ts#L34-L74) [`packages/session/session-projection/src/index.ts:88-118`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection/src/index.ts#L88-L118)

The optional projection cache stores one record per session in the `session_projcache` domain:

```ts
{
  identity: { createdAt: number; cwd?: string }
  rows: Record<string, { ver: number; seq: number; val: JsonValue }>
}
```

It is explicitly a fold shortcut rather than authority. The shipped domain version is `3`; domain-version mismatch discards the medium, and unit `stateVersion` mismatch discards that unit row. [`packages/session/session-projection-cache/src/spec.ts:16-69`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/spec.ts#L16-L69) [`packages/session/session-projection-cache/src/index.ts:1-12`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L1-L12)

## Keying and identity

`SessionId` is only a branded string. `SessionId(id)` performs a compile-time cast and no runtime validation or generation. IDs contain no required timestamp, project, tenant, or host component. [`packages/core/session/src/types.ts:21-31`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L21-L31)

If a caller omits the ID at the low-level `SessionStore`, it mints `session-<n>` from a process-local counter. Callers may instead supply any branded string. Host and SDK entry points often supply UUID-based values, but that is caller policy rather than a storage invariant. [`packages/core/session/src/index.ts:809-888`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L809-L888) [`packages/host/apiproxy/src/api-proxy.ts:2078-2087`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/host/apiproxy/src/api-proxy.ts#L2078-L2087) [`packages/sdk/client/src/api.ts:84-93`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/sdk/client/src/api.ts#L84-L93)

The immutable `SessionHeader` is the storage identity record:

```ts
export interface SessionHeader {
  readonly version: number
  readonly id: SessionId
  readonly createdAt: number
  readonly cwd?: string
  readonly parentSession?: SessionId
  readonly seedLength?: number
  readonly origin?: 'subagent'
  readonly delegationDepth?: number
  readonly agentPreset?: string
}
```

This declaration is quoted verbatim. [`packages/core/session/src/types.ts:58-99`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L58-L99)

For JSONL, `cwd` selects a readable project directory and the ID is injectively escaped to one safe path component. CWD normalization is intentionally lossy, so multiple CWDs may share a project directory; lookup scans readable project directories for the encoded ID, validates the artifact header against the selected path, and rejects duplicate IDs. Identity is therefore session-ID-wide inside one configured root, not `(cwd, id)`, even though `cwd` participates in placement. [`packages/session/session-persistence-jsonl/src/format.ts:110-179`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/format.ts#L110-L179) [`packages/session/session-persistence-jsonl/src/index.ts:773-795`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L773-L795)

Persistent `list()` is global to the configured backend, not scoped to a CWD. JSONL discovers all readable project directories under its root and SQLite selects every session row. Callers that need a project-scoped view must filter the returned headers. [`packages/session/session-persistence/src/index.ts:223-228`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L223-L228) [`packages/session/session-persistence-jsonl/src/index.ts:472-509`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L472-L509) [`packages/session/session-persistence-sqlite/src/store.ts:241-260`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L241-L260)

For SQLite, `id` is the primary key in `sessions`; `cwd` is metadata. Neither persistence service exposes rename, relocate, or header-update operations. Relocating a worktree does not rewrite a stored header, and JSONL additionally binds the header's ID and CWD to its derived artifact path. Any relocation is therefore a new session or out-of-band backend maintenance, not a supported session-store operation. [`packages/session/session-persistence-sqlite/resources/sql/schema.sql:6-18`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/resources/sql/schema.sql#L6-L18) [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240) [`packages/session/session-persistence-jsonl/README.md:40-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L40-L48)

## The store interface

These supporting value declarations are quoted verbatim:

```ts
export interface SessionPersistenceSnapshot {
  header: SessionHeader
  revision: SessionPersistenceRevision
}

export interface SessionInspection {
  readonly meta: SessionHeader
  readonly events: readonly SessionEvent[]
}

export interface SessionRawArtifact {
  readonly meta: SessionHeader
  readonly filename: string
  readonly content: string
}

export interface SessionLocation {
  readonly kind: string
  readonly path: string
}
```

[`packages/session/session-persistence/src/index.ts:17-41`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L17-L41) [`packages/session/session-persistence/src/index.ts:66-76`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L66-L76)

The complete public `SessionPersistence` signature set is shown below. Source JSDoc and the two concrete method bodies are omitted; member names, modifiers, parameters, and return types are preserved:

```ts
constructor(ctx: Context)
abstract locate(meta: SessionHeader): SessionLocation | undefined
abstract readonly supportsRawArtifacts: boolean
readRaw(_id: SessionId, signal?: AbortSignal): Promise<SessionRawArtifact | undefined>
abstract create(meta: SessionHeader): Promise<void>
abstract append(id: SessionId, events: readonly SessionEvent[]): Promise<void>
async prepare(id: SessionId, signal?: AbortSignal): Promise<SessionPreparation>
abstract load(id: SessionId): Promise<SessionInspection>
abstract inspect(id: SessionId, signal?: AbortSignal): Promise<SessionInspection>
abstract readFrom(id: SessionId, fromSeq: number, signal?: AbortSignal): Promise<{ meta: SessionHeader; events: SessionEvent[] }>
abstract list(signal?: AbortSignal): Promise<SessionHeader[]>
abstract listSnapshots(signal?: AbortSignal): Promise<SessionPersistenceSnapshot[]>
```

The constructor delegates service registration to Cordis. `readRaw()` and `prepare()` have concrete default implementations; every other callable method is abstract, as is `supportsRawArtifacts`. [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240) [`docs/subsystems/persistence.md:246-380`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/subsystems/persistence.md#L246-L380)

| Operation | Semantics |
|---|---|
| `locate` | Side-effect-free local artifact hint. SQLite returns `undefined`; a path is not an authorization token and may not exist yet. [`packages/session/session-persistence/src/index.ts:66-102`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L66-L102) |
| `readRaw` | Optional exact backend artifact text, decoded from physical compression but not reconstructed from events. JSONL supports it; SQLite does not. [`packages/session/session-persistence/src/index.ts:98-124`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L98-L124) |
| `create` | Registers immutable metadata and may remain entirely lazy. [`packages/session/session-persistence/src/index.ts:126-133`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L126-L133) |
| `append` | Requires a contiguous batch and resolves only after durability. [`packages/session/session-persistence/src/index.ts:135-143`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L135-L143) |
| `prepare` | Builds or reuses an unpublished `Session` for resume, checking that the persisted revision still matches before reservation. [`packages/session/session-persistence/src/index.ts:145-168`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L145-L168) |
| `load` | Returns a balanced immutable logical log and commits cold crash repair. It rejects a live session whose turn remains open. [`packages/session/session-persistence/src/index.ts:170-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L170-L183) |
| `inspect` | Returns a logical view without committing repair or publishing a live session. Cold repair closers exist only in the returned view. [`packages/session/session-persistence/src/index.ts:185-200`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L185-L200) |
| `readFrom` | Detached valid-prefix suffix read. It does not truncate, synthesize closers, or use preparation state. [`packages/session/session-persistence/src/index.ts:202-221`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L202-L221) |
| `list` | All materialized headers, without full-log parsing, pagination, or filtering. [`packages/session/session-persistence/src/index.ts:223-228`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L223-L228) [`packages/session/session-persistence/README.md:81-85`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L81-L85) |
| `listSnapshots` | All materialized headers plus opaque source-qualified revisions that stay equal while a stored log is unchanged. [`packages/session/session-persistence/src/index.ts:230-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L230-L240) |

The complete exported lower-level backend signature set is quoted verbatim, with source JSDoc omitted:

```ts
export interface PersistenceBackend<TornMarker = unknown> {
  readonly name: string
  loadStored(id: SessionId, signal?: AbortSignal): Promise<StoredPrefix<TornMarker> | undefined>
  readStoredRevision(id: SessionId, signal?: AbortSignal): Promise<SessionPersistenceRevision | undefined>
  loadStoredFrom?(id: SessionId, fromSeq: number, signal?: AbortSignal): Promise<StoredSuffix | undefined>
  appendBatch(meta: SessionHeader, events: readonly SessionEvent[], isMaterialized: boolean): Promise<void>
  commitRepair(meta: SessionHeader, tornMarker: TornMarker | undefined, closers: readonly SessionEvent[]): Promise<void>
  list(signal?: AbortSignal): Promise<SessionHeader[]>
  locate?(meta: SessionHeader): SessionLocation | undefined
  close?(): Promise<void>
}
```

`name`, `loadStored`, `readStoredRevision`, `appendBatch`, `commitRepair`, and `list` are required. `loadStoredFrom`, `locate`, and `close` are optional, as shown by `?`. `appendBatch` must atomically combine first materialization with the first event batch. `commitRepair` is not required to be atomic. [`packages/session/session-persistence/src/coordinator.ts:117-215`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L117-L215)

There is no persistent update, delete, retention, rename, pagination, text-search, arbitrary metadata query, transaction callback, or compare-and-swap method in this service. Search and projections are separate consumers. [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240)

## Write and append path (ordering, durability, concurrency, delivery)

### Live commit and delivery

`Session.append()` is the first commit point. It computes a contiguous sequence number in memory, validates lossless JSON and surface invariants, deep-freezes the event, and appends it before invoking observers. The event exists in the authoritative live session even if a persistence listener later fails. [`packages/core/session/src/index.ts:569-655`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L569-L655)

Persistence is a subscriber. For each committed event, the coordinator copies the frozen event with `structuredClone()` into a per-session write-behind queue. The first pending event arms a fixed deadline; later events do not extend it. An event arriving during a write forms a later batch. [`packages/session/session-persistence/src/write-behind.ts:18-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/write-behind.ts#L18-L56) [`packages/session/session-persistence/README.md:32-38`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L32-L38)

An explicit `SessionStore.flush(session)` dispatches `session/flush` and waits for every registered persistence listener. Concurrent flush callers on one controller share a barrier; that barrier waits for an overlapping write and then drains all work admitted before quiescence. [`packages/core/session/src/index.ts:1009-1039`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1009-L1039) [`packages/session/session-persistence/src/write-behind.ts:58-71`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/write-behind.ts#L58-L71) [`packages/session/session-persistence/src/write-behind.ts:117-136`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/write-behind.ts#L117-L136)

The shipped `session-checkpoint-policy` chooses semantic durability barriers on top of background batching. It flushes before constructing and dispatching a model request, before a top-level tool body may perform an external side effect, and at every `agent/pre-step` boundary so the prior response and ordered tool results are durable before the next request. Model and top-level tool dispatch fail closed when their flush fails, and a pre-step failure ends the turn before another request begins. Nested tool dispatches reuse the outer call checkpoint. [`packages/session/session-checkpoint-policy/README.md:5-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-checkpoint-policy/README.md#L5-L23)

A background write failure is reported, its batch is restored at the front of the queue, and automatic retry pauses. A later event starts a new deadline; an explicit flush or teardown retries immediately and exposes repeated failure. This is at-least-retained in memory until retry, not an at-least-once distributed-delivery promise. Sequence contiguity is the duplicate and gap guard; the event envelope has no independent write ID. [`packages/session/session-persistence/src/write-behind.ts:138-158`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/write-behind.ts#L138-L158) [`packages/session/session-persistence/README.md:25-36`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L25-L36)

The coordinator serializes all operations for a session ID through a promise chain. This prevents interleaving inside one backend instance, and errors do not poison later operations. It is not a cross-process lease. [`packages/session/session-persistence/src/coordinator.ts:1004-1033`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L1004-L1033)

### JSONL write shape

The exact header record type is:

```ts
export interface HeaderLine {
  type: 'session'
  version: number
  id: SessionId
  createdAt: number
  cwd?: string
  parentSession?: SessionId
  seedLength?: number
  origin?: 'subagent'
  delegationDepth: number
  agentPreset?: string
}
```

This declaration is quoted verbatim. [`packages/session/session-persistence-jsonl/src/format.ts:28-44`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/format.ts#L28-L44)

Each later logical record is either a verbatim `SessionEvent` or a lossless packed run of at least three compatible `assistant/chunk` deltas. Packed rows preserve every member's sequence and timestamp and decode back into the same logical event sequence. [`packages/session/session-persistence-jsonl/README.md:17-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L17-L20) [`packages/session/session-persistence-jsonl/src/format.ts:210-224`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/format.ts#L210-L224)

With Zstandard, the header is one checksummed frame and every durable append batch is another checksummed frame. Raw mode appends newline-delimited UTF-8. The backend fsyncs successful appends and rolls a failed append or sync back to the prior byte length. [`packages/session/session-persistence-jsonl/README.md:34-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L34-L48) [`packages/session/session-persistence-jsonl/src/index.ts:646-700`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L646-L700)

First materialization writes and syncs a temporary file containing header plus first batch. POSIX publishes it without overwrite via a hard link and fsyncs the parent directory. Windows uses write-through `MoveFileExW` without replacement. This makes first same-ID publication collision-safe. Later concurrent writers are unsupported. [`packages/session/session-persistence-jsonl/src/index.ts:513-625`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L513-L625) [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77)

### SQLite write shape

The complete schema is:

```sql
CREATE TABLE persistence_state (
  singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
  store_id  TEXT NOT NULL
) STRICT;

CREATE TABLE sessions (
  id               TEXT PRIMARY KEY,
  version          INTEGER NOT NULL,
  created_at       INTEGER NOT NULL,
  cwd              TEXT,
  parent_session   TEXT,
  seed_length      INTEGER,
  origin           TEXT,
  delegation_depth INTEGER,
  agent_preset     TEXT,
  incarnation      TEXT NOT NULL,
  revision         INTEGER NOT NULL
) STRICT;

CREATE TABLE events (
  session_id        TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  seq               INTEGER NOT NULL,
  type              TEXT NOT NULL,
  time              INTEGER NOT NULL,
  data              ANY NOT NULL,
  source_event_seqs ANY,
  surface_op        TEXT,
  ignorable         INTEGER CHECK (ignorable IS NULL OR ignorable IN (0, 1)),
  PRIMARY KEY (session_id, seq)
) STRICT;
```

This schema is quoted verbatim. [`packages/session/session-persistence-sqlite/resources/sql/schema.sql:1-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/resources/sql/schema.sql#L1-L30)

An append uses `BEGIN IMMEDIATE`, validates the current tail and expected first sequence, writes the session row during first materialization, inserts physical event rows, increments the session revision, and commits. Any failure rolls back the transaction. `synchronous=FULL` is set and read back. [`packages/session/session-persistence-sqlite/src/store.ts:173-199`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L173-L199) [`packages/session/session-persistence-sqlite/src/schema.ts:177-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/schema.ts#L177-L183)

Compatible chunk runs may be packed into a physical row with limits of 1,024 logical events or 1 MiB. Payload data at least 4 KiB is Zstandard-compressed only when compression makes it smaller, and source sequences use a compact binary representation. These encodings are invisible after logical decode. [`packages/session/session-persistence-sqlite/README.md:5-19`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L5-L19)

### Crash repair

The recovery contract preserves every complete event. It removes only a physically torn final fragment, then closes a complete interrupted turn by appending synthetic error `tool/result` events for unanswered calls, an open `step/end` when needed, and `turn/end` with `{ kind: 'interrupted' }`. A gap, middle parse failure, committed corruption, or malformed prefix rejects instead of being repaired. [`packages/session/session-persistence/README.md:25-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L25-L30) [`packages/session/session-persistence/src/index.ts:170-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L170-L183)

Recovery distinguishes two unanswered-call states. An assistant message that requested a call but has no durable `tool/call` receives `ToolNotStartedError` with code `TOOL_NOT_STARTED`, no source-event sequence, and text saying the Harness never recorded the call as started. A durable `tool/call` with no result receives `ToolOutcomeUnknownError` with code `TOOL_OUTCOME_UNKNOWN`, cites the call event, and warns that external side effects may already have happened. [`packages/core/session/src/repair.ts:89-123`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/repair.ts#L89-L123)

Cold `inspect()` computes the same balanced logical view but leaves the physical tail untouched. Cold `load()` and `prepare()` commit repair. A live `load()` first flushes the exact authoritative in-memory snapshot and returns it only if balanced; it never manufactures interruption inside a still-live open turn. [`packages/session/session-persistence/src/index.ts:170-200`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L170-L200) [`packages/session/session-persistence/src/coordinator.ts:973-989`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L973-L989)

JSONL repair truncates to a byte offset and syncs, then appends and syncs the recovered complete records and closers. SQLite repair performs stale-tail checks, deletes rows from the torn sequence onward, inserts closers, increments revision, and commits in one transaction. The abstract backend contract intentionally permits non-atomic repair, so backend-specific idempotence matters after a second crash. [`packages/session/session-persistence-jsonl/src/index.ts:646-700`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L646-L700) [`packages/session/session-persistence-sqlite/src/store.ts:201-239`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L201-L239) [`packages/session/session-persistence/src/coordinator.ts:186-193`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L186-L193)

## Read and resume path

The cold resume path is `prepare(id)`, not a direct mutable restore. The coordinator reads and validates the stored prefix, computes logical repair, constructs one unpublished `Session`, checks that the source revision is still current, commits pending repair, reserves that exact object, and returns a `SessionPreparation` for publication. Stale prepared objects are discarded and loaded again. [`packages/session/session-persistence/src/index.ts:145-168`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L145-L168) [`packages/session/session-persistence/src/coordinator.ts:720-775`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L720-L775) [`packages/session/session-persistence/src/coordinator.ts:891-970`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L891-L970)

`Session.fromRestore()` validates a contiguous seed beginning at sequence zero, validates every JSON payload and surface transition, adopts the immutable header, and appends `session/end-seed` unless the seed already ends with one. The result preserves prior events exactly and marks where the new lifecycle begins. [`packages/core/session/src/index.ts:474-547`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L474-L547) [`packages/core/session/src/types.ts:315-336`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L315-L336)

Reading has three distinct semantics:

| Path | Repair side effect | Live-session behavior | Intended use |
|---|---|---|---|
| `load` | Commits cold tail repair. | Flushes and returns a balanced live snapshot; rejects an open live turn. | Balanced replay or compatibility callers. [`packages/session/session-persistence/src/index.ts:170-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L170-L183) |
| `inspect` | None. Synthetic closers are returned only in memory. | Returns the current immutable live snapshot, which may be open. | History inspection and preparing reusable cold state. [`packages/session/session-persistence/src/index.ts:185-200`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L185-L200) |
| `readFrom` | None. No closers and no preparation cache. | Detached stored-prefix read. | Projection tails and watermarks. [`packages/session/session-persistence/src/index.ts:202-221`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L202-L221) |

SQLite implements a true suffix read and scans backward only far enough to include a packed row that may begin below `fromSeq`. JSONL has no seek primitive and scans the whole artifact before skipping. [`packages/session/session-persistence-sqlite/src/store.ts:317-350`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L317-L350) [`packages/session/session-persistence/src/index.ts:202-221`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L202-L221)

There is no entry pagination, page size, or transcript-length bound in the persistence API. `prepare()`, `load()`, and `inspect()` eagerly reconstruct a complete logical log; only `readFrom()` lets a projection consumer request an unbounded suffix. Projection-cache cold reads are the lazy exception: they load a cached fold plus the durable tail, falling back to sequence zero when the cache overreaches the log. [`packages/session/session-persistence/src/index.ts:145-221`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L145-L221) [`packages/session/session-projection-cache/src/index.ts:154-197`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L154-L197)

The model sees only the surface projection after resume. Raw chunks, request metadata, audits, lifecycle facts, and compaction records remain in the log but are not all projected into model messages. [`packages/core/session/src/types.ts:230-350`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L230-L350) [`packages/core/session/src/index.ts:701-747`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L701-L747)

## Listing, summaries, and search

The persistence seam lists lightweight immutable headers or header-plus-revision snapshots. Both first-party backends avoid decoding full logs for listing. JSONL validates only the header frame or line; SQLite selects session rows. There is no paging, filtering, sorting, or text query in the seam. [`packages/session/session-persistence/src/index.ts:223-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L223-L240) [`packages/session/session-persistence-jsonl/src/format.ts:396-413`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/format.ts#L396-L413) [`packages/session/session-persistence-sqlite/src/store.ts:241-260`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L241-L260)

JSONL listing is a directory and header scan across the configured root; SQLite listing is a table query. The sources state no benchmark or scale limit, but the seam itself warns that all-session listing is unindexed at scale because it is unpaginated and unfiltered. [`packages/session/session-persistence-jsonl/src/index.ts:472-509`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L472-L509) [`packages/session/session-persistence/README.md:81-85`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L81-L85)

The host constructs a user-facing list by starting with live `SessionStore.list()`, merging persisted `list()` rows not already live, attaching projection-cache baselines when available, and sorting the combined view by update time. Live state wins identity races. [`packages/host/apiproxy/src/api-proxy.ts:1665-1729`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/host/apiproxy/src/api-proxy.ts#L1665-L1729)

`SessionCorpus` provides an exact logical corpus over live and persisted sessions. It merges headers with live precedence, sorts newest first, reads live logs directly, and otherwise calls persistence inspection. It rechecks live state after a persisted read to close the race where a session becomes live during I/O. [`packages/session-query/session-query/src/corpus.ts:31-116`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query/src/corpus.ts#L31-L116)

The optional SQLite query provider is a separate FTS5 derived index. It reconciles `listSnapshots()` revisions, calls `inspect()` only for changed logs, combines persisted FTS rows with a temporary live-session overlay, and can rebuild from the authoritative corpus. The query database has one owner process and is not the session store. [`packages/session-query/session-query-sqlite/README.md:5-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query-sqlite/README.md#L5-L23) [`packages/session-query/session-query-sqlite/README.md:52-57`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query-sqlite/README.md#L52-L57)

Projection-cache listing is intentionally stale but watermark-qualified. `cachedSnapshot()` returns only identity-matching, version-matching rows and chooses the lowest served watermark. A later cold read obtains `readFrom(floor)`, folds the tail, falls back to a full read if the stored log is shorter than the cache claims, and writes the repaired cache back fail-soft. [`packages/session/session-projection-cache/src/index.ts:91-130`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L91-L130) [`packages/session/session-projection-cache/src/index.ts:154-197`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L154-L197)

## Entry/message structure and versioning

### Event envelope

The canonical event is an envelope over a declaration-merged payload map. This declaration is quoted verbatim:

```ts
export type SessionEvent<T extends SessionEventType = SessionEventType> = {
  [K in SessionEventType]: {
    type: K
    seq: number
    time: number
    data: SessionEventMap[K]
    ignorable?: true
  } & (K extends SurfaceEventType ? {
    sourceEventSeqs?: number[]
    surfaceOp?: SurfaceOp
  } : object)
}[T]
```

[`packages/core/session/src/types.ts:395-440`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L395-L440)

Only `user/message`, `assistant/message`, and `tool/result` are surface-eligible. Their `surfaceOp` is either `'append'` or `{ op: 'replace', start, end }`; `sourceEventSeqs` cites earlier events that produced the surface node. All other payloads are log-only and cannot carry those fields. [`packages/core/session/src/types.ts:339-379`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L339-L379)

### Complete payload catalog at the snapshot

The repository generates and verifies a full persistence catalog from the merge-extended `SessionEventMap`. The following table transcribes every event declaration at the pinned snapshot. [`docs/persistence-catalog.md:1-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1-L20)

| Event type | `data` payload |
|---|---|
| `agent/inbox/spliced` | `{ target: InboxTarget; start: number; removedCount?: number; inserted: UserMessage[]; outcome?: 'canceled' }` [`docs/persistence-catalog.md:101-122`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L101-L122) |
| `agent-preset/selected` | `{ agentPreset: string }` [`docs/persistence-catalog.md:124-140`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L124-L140) |
| `approval/asked` | `{ id: ApprovalRequestId; toolName: string; callId?: CallId; reason?: string }` [`docs/persistence-catalog.md:142-165`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L142-L165) |
| `approval/decided` | `{ id: ApprovalRequestId; outcome: ApprovalOutcome }` [`docs/persistence-catalog.md:167-183`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L167-L183) |
| `approval/policy` | `{ policy: ApprovalPolicy; source?: 'delegation' }` [`docs/persistence-catalog.md:185-207`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L185-L207) |
| `assistant/chunk` | `{ turn: number; step: number; chunk: StreamChunk }` [`docs/persistence-catalog.md:209-220`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L209-L220) |
| `assistant/message` | `{ turn: number; step: number; message: AssistantMessage; usage?: TokenUsage; interrupted?: true }` [`docs/persistence-catalog.md:222-244`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L222-L244) |
| `command/done` | `{ commandId: CommandId; kind: 'success' \| 'error'; text?: string; sourceEventSeq?: number }` [`docs/persistence-catalog.md:246-265`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L246-L265) |
| `command/run` | `{ commandId: CommandId; name: string; args?: string; source: CommandSource }` [`docs/persistence-catalog.md:267-287`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L267-L287) |
| `compaction/end` | `{ compactionId: CompactionId; sourceCommandId?: CommandId; turn: number \| null; error?: string }` [`docs/persistence-catalog.md:289-301`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L289-L301) |
| `compaction/prune` | `{ shadowedRange: { start: number; end: number }; shadowedSeqs: number[]; shadowedTokenCount: number }` [`docs/persistence-catalog.md:303-327`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L303-L327) |
| `compaction/start` | `{ compactionId: CompactionId; sourceCommandId?: CommandId; turn: number \| null }` [`docs/persistence-catalog.md:329-342`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L329-L342) |
| `compaction/summary` | `{ compactionId; sourceCommandId?; summary: ContentBlock[]; shadowedRange; shadowedSeqs; shadowedTokenCount; provider; model; maxTokens?; usage? }` plus either `{ rawOutput; llmStreamCall: true }` or `{ rawOutput?; llmStreamCall?: never }` [`docs/persistence-catalog.md:344-398`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L344-L398) |
| `feedback/record` | `{ text: string }` [`docs/persistence-catalog.md:400-414`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L400-L414) |
| `goal/change` | `GoalChangeMeta` [`docs/persistence-catalog.md:416-429`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L416-L429) |
| `hook/invoked` | `{ turn: number; point: string; dialect: HookDialect; matcher?: string; handlerId: string }` [`docs/persistence-catalog.md:431-454`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L431-L454) |
| `hook/result` | `{ turn: number; point: string; handlerId: string; decision: string; exitCode?: number; stderrSummary?: string; durationMs: number }` [`docs/persistence-catalog.md:456-479`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L456-L479) |
| `llm/retry` | `LlmRetryEventData` [`docs/persistence-catalog.md:481-490`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L481-L490) |
| `llm/retry-started` | `LlmRetryStartedEventData` [`docs/persistence-catalog.md:492-503`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L492-L503) |
| `permission/preset` | `{ preset: string }` [`docs/persistence-catalog.md:505-521`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L505-L521) |
| `plan/mode` | `{ active: boolean }` [`docs/persistence-catalog.md:523-538`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L523-L538) |
| `request/context` | `{ provider: string; model: string; contextWindow?: number }` [`docs/persistence-catalog.md:540-552`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L540-L552) |
| `request/header` | `{ header: EpochHeader; reason: 'initial' \| 'resume' \| 'change' }` [`docs/persistence-catalog.md:554-568`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L554-L568) |
| `sandbox/mode` | `{ mode: SandboxMode; source?: 'delegation' }` [`docs/persistence-catalog.md:570-591`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L570-L591) |
| `schedule/change` | `ScheduleChange` [`docs/persistence-catalog.md:593-609`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L593-L609) |
| `session/end-seed` | `Record<string, never>` [`docs/persistence-catalog.md:611-641`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L611-L641) |
| `session/title` | `{ title: string; messageSeqs: number[]; source: SessionTitleSource }` [`docs/persistence-catalog.md:643-657`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L643-L657) [`packages/session/session-title/src/index.ts:47-68`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-title/src/index.ts#L47-L68) |
| `session/title-llm-request` | `{ titleProvider; messageSeqs; route; system; messages; maxTokens }` [`docs/persistence-catalog.md:659-672`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L659-L672) [`packages/session/session-title-llm/src/index.ts:24-38`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-title-llm/src/index.ts#L24-L38) |
| `step/end` | `{ turn: number; step: number }` [`docs/persistence-catalog.md:674-683`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L674-L683) |
| `step/start` | `{ turn: number; step: number }` [`docs/persistence-catalog.md:685-696`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L685-L696) |
| `subagent/descriptor` | `SubagentDescriptorData` [`docs/persistence-catalog.md:698-715`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L698-L715) |
| `team/member` | `{ version: 1; teamId: TeamId; member: TeamMemberSnapshot }` [`docs/persistence-catalog.md:717-728`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L717-L728) |
| `team/message/delivered` | `{ version: 1; teamId: TeamId; messageId: TeamMessageId; targetId: SessionId }` [`docs/persistence-catalog.md:730-746`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L730-L746) |
| `team/message/queued` | `{ version: 1; teamId: TeamId; message: TeamMessageSnapshot }` [`docs/persistence-catalog.md:748-759`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L748-L759) |
| `team/task` | `{ version: 1; teamId: TeamId; task: TeamTaskSnapshot }` [`docs/persistence-catalog.md:761-774`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L761-L774) |
| `todo/write` | `{ todos: { content: string; status: 'pending' \| 'in_progress' \| 'completed' }[] }` [`docs/persistence-catalog.md:776-789`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L776-L789) [`packages/core/session/src/types.ts:179-194`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L179-L194) |
| `tool/call` | `{ turn: number; step: number; callId: CallId; name: string; arguments: string }` [`docs/persistence-catalog.md:791-806`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L791-L806) |
| `tool/code-dispatch` | `{ rootCallId; parentCallId; subCallId; name; arguments; isError; content }` [`docs/persistence-catalog.md:808-831`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L808-L831) [`packages/core/tools/src/types.ts:10-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/tools/src/types.ts#L10-L23) |
| `tool/code-dispatch-start` | `{ rootCallId; parentCallId; subCallId; name; arguments }` [`docs/persistence-catalog.md:833-854`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L833-L854) [`packages/core/tools/src/types.ts:10-17`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/tools/src/types.ts#L10-L17) |
| `tool/result` | `{ turn; step; message: ToolResultMessage; error?: { name; code }; meta?: JsonValue }` [`docs/persistence-catalog.md:856-883`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L856-L883) |
| `tool-workflow/agent-end` | `{ runId: WorkflowRunId; seq: number; outcome: WorkflowAgentOutcome }` [`docs/persistence-catalog.md:885-897`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L885-L897) [`packages/workflow/tool-workflow/src/types.ts:28-33`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/tool-workflow/src/types.ts#L28-L33) |
| `tool-workflow/agent-start` | `{ runId; seq; label; phase?; childId }` [`docs/persistence-catalog.md:899-911`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L899-L911) [`packages/workflow/tool-workflow/src/types.ts:19-26`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/tool-workflow/src/types.ts#L19-L26) |
| `tool-workflow/run-end` | `{ runId: WorkflowRunId; stopReason: WorkflowStopReason }` [`docs/persistence-catalog.md:913-925`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L913-L925) [`packages/workflow/tool-workflow/src/types.ts:35-39`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/tool-workflow/src/types.ts#L35-L39) |
| `tool-workflow/run-start` | `{ runId: WorkflowRunId; name: string }` [`docs/persistence-catalog.md:927-941`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L927-L941) [`packages/workflow/tool-workflow/src/types.ts:13-17`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/tool-workflow/src/types.ts#L13-L17) |
| `turn/end` | `{ turn: number; reason: TurnEndReason }` [`docs/persistence-catalog.md:943-961`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L943-L961) |
| `turn/start` | `{ turn: number }` [`docs/persistence-catalog.md:963-979`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L963-L979) |
| `user/message` | `UserMessage` [`docs/persistence-catalog.md:981-998`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L981-L998) |
| `web/deepseek-search-llm-request` | `{ endpoint; apiVersion; body: { model; max_tokens; messages; tools } }` [`docs/persistence-catalog.md:1000-1007`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1000-L1007) [`packages/web/web-search-deepseek/src/provider.ts:52-78`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/web/web-search-deepseek/src/provider.ts#L52-L78) |

The important nested payloads are also durable values:

The message payload declarations are quoted verbatim:

```ts
export interface Message {
  readonly id: MessageId
  readonly role: 'system' | 'user' | 'assistant'
  readonly content: ContentBlock[]
  readonly source: MessageSource
}

export interface UserMessage extends Message {
  readonly role: 'user'
}

export interface AssistantMessage extends Message {
  readonly role: 'assistant'
  readonly source: ModelMessageSource
}

export interface ToolResultMessage extends Message {
  readonly role: 'user'
  readonly content: [ToolResultBlock]
  readonly source: ToolMessageSource
}
```

[`packages/llm/llm/src/message.ts:96-156`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/message.ts#L96-L156)

The content and streamed assistant payload declarations are also quoted verbatim:

```ts
export interface TextBlock {
  type: 'text'
  text: string
}

export interface ReasoningBlock {
  type: 'reasoning'
  text: string
}

export interface ImageBlock {
  type: 'image'
  attachment: ImageAttachmentRef
}

export interface ToolCallBlock {
  type: 'tool-call'
  id: CallId
  name: string
  arguments: string
}

export interface ToolResultBlock {
  type: 'tool-result'
  toolCallId: CallId
  content: ContentBlock[]
  isError?: boolean
}

export interface ContentBlockMap {
  'text': TextBlock
  'reasoning': ReasoningBlock
  'image': ImageBlock
  'tool-call': ToolCallBlock
  'tool-result': ToolResultBlock
}

export type ContentBlock = ContentBlockMap[ContentBlockType]

export type StreamChunk =
  | { type: 'block-start'; index: number; blockType: ContentBlockType }
  | { type: 'text-delta'; index: number; text: string }
  | { type: 'reasoning-delta'; index: number; text: string }
  | { type: 'tool-call-delta'; index: number; id: CallId; name?: string; argumentsDelta: string }
  | { type: 'block-end'; index: number; block: ContentBlock }
  | { type: 'usage'; usage: TokenUsage }
  | {
    type: 'finish'
    reason: FinishReason
    replayState?: ReplayEnvelope
  }

export interface FinishReasonMap {
  'stop': { kind: 'stop' }
  'tool-calls': { kind: 'tool-calls' }
  'max-tokens': { kind: 'max-tokens' }
  'aborted': { kind: 'aborted'; failure: LlmFailure }
  'error': { kind: 'error'; failure: LlmFailure }
}

export interface TokenUsage {
  inputTokens: number
  outputTokens: number
  cacheReadTokens?: number
  cacheWriteTokens?: number
  reasoningTokens?: number
}

export interface ReplayEnvelope {
  response: unknown
  blocks?: readonly unknown[]
}

export type ImageMediaType = 'image/png' | 'image/jpeg' | 'image/webp' | 'image/gif'

export interface ImageAttachmentRef {
  attachmentId: AttachmentId
  mediaType: ImageMediaType
  bytes: number
  width: number
  height: number
  name?: string
}
```

[`packages/llm/llm/src/types.ts:53-141`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/types.ts#L53-L141) [`packages/llm/llm/src/types.ts:290-324`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/types.ts#L290-L324) [`packages/attachment/attachment/src/types.ts:7-24`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/attachment/attachment/src/types.ts#L7-L24)

The request-header, approval, and retry aliases expand to these durable fields. Source comments are omitted, while declarations and union values are reproduced exactly:

```ts
export type ApprovalOutcome = 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'
export type ApprovalPolicy = 'ask' | 'never'

export interface LlmCallConfig {
  provider: string
  model: string
  reasoningEffort?: ReasoningEffortId
  temperature?: number
  maxTokens?: number
  stop?: string[]
}

export interface LlmCallConfigAdapterDefaults {
  reasoningEffort?: true
  maxTokens?: true
}

export interface ToolSchema {
  name: string
  description: string
  parameters: Record<string, unknown>
}

export interface EpochHeader {
  config: LlmCallConfig
  adapterDefaults?: LlmCallConfigAdapterDefaults
  system?: string
  tools?: ToolSchema[]
}

export interface LlmFailure {
  readonly message: string
  readonly code: string
  readonly status?: number
  readonly providerRetryAfterMs?: number
  readonly requestId?: ProviderRequestId
}

export type LlmRetryEventData =
  | {
    retryId: RetryId
    turn: number
    step: number
    provider: string
    mode: 'normal'
    policyKey: string
    retry: number
    maxRetries: number
    delayMs: number
    failure: LlmFailure
  }
  | {
    retryId: RetryId
    turn: number
    step: number
    provider: string
    mode: 'always'
    policyKey: string
    retry: number
    delayMs: number
    failure: LlmFailure
  }

export interface LlmRetryStartedEventData {
  retryId: RetryId
  turn: number
  step: number
  retry: number
}
```

[`packages/interaction/user-approval/src/types.ts:25-29`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/interaction/user-approval/src/types.ts#L25-L29) [`packages/interaction/user-approval/src/index.ts:84-94`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/interaction/user-approval/src/index.ts#L84-L94) [`packages/llm/llm/src/call-config.ts:17-39`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/call-config.ts#L17-L39) [`packages/llm/llm/src/types.ts:39-51`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/types.ts#L39-L51) [`packages/llm/llm/src/types.ts:326-338`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/types.ts#L326-L338) [`packages/core/session/src/types.ts:196-210`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L196-L210) [`packages/llm/llm-retry/src/types.ts:15-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm-retry/src/types.ts#L15-L48)

`TurnEndReason` includes completed, aborted with cause, blocked, structured error, max-tokens, and persistence-repaired interrupted. [`packages/core/session/src/types.ts:142-177`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L142-L177)

The goal alias includes its nested stable reference, phase, block reason, full snapshot, mutation counters, timestamps, and clear tombstone:

```ts
export interface GoalRef {
  readonly id: GoalId
  readonly revision: number
}

export type GoalPhase =
  | 'active'
  | 'paused'
  | 'blocked'
  | 'complete'

export interface GoalBlockReason {
  readonly code: string
  readonly message: string
}

export interface GoalSnapshot extends GoalRef {
  readonly objective: string
  readonly phase: GoalPhase
  readonly blockedReason?: GoalBlockReason
  readonly maxGoalRounds: number
}

export type GoalOperation =
  | 'create'
  | 'edit'
  | 'pause'
  | 'resume'
  | 'complete'
  | 'block'
  | 'clear'

export interface GoalSnapshotChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: Exclude<GoalOperation, 'clear'>
  readonly goal: GoalSnapshot
  readonly roundsStarted: number
  readonly createdAt: number
  readonly updatedAt: number
}

export interface GoalClearChangeMeta {
  readonly kind: 'goal/change'
  readonly version: 1
  readonly operation: 'clear'
  readonly cleared: GoalRef
  readonly clearedAt: number
}

export type GoalChangeMeta = GoalSnapshotChangeMeta | GoalClearChangeMeta
```

[`packages/goal/goal/src/types.ts:15-68`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/goal/goal/src/types.ts#L15-L68) [`packages/goal/goal/src/domain.ts:13-44`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/goal/goal/src/domain.ts#L13-L44)

The schedule alias is the following version-1 union. All creation record variants and dispatch fields are included:

```ts
export interface AfterScheduleRecord {
  readonly id: ScheduleId
  readonly kind: 'after'
  readonly prompt: string
  readonly afterSeconds: number
  readonly scheduledAt: string
}

export interface AtScheduleRecord {
  readonly id: ScheduleId
  readonly kind: 'at'
  readonly prompt: string
  readonly scheduledAt: string
}

export interface EveryScheduleRecord {
  readonly id: ScheduleId
  readonly kind: 'every'
  readonly prompt: string
  readonly everySeconds: number
  readonly scheduledAt: string
}

export type ScheduleRecord = AfterScheduleRecord | AtScheduleRecord | EveryScheduleRecord

export interface ScheduleCreateChange {
  readonly version: 1
  readonly operation: 'create'
  readonly schedule: ScheduleRecord
}

export interface ScheduleDeleteChange {
  readonly version: 1
  readonly operation: 'delete'
  readonly id: ScheduleId
}

export interface OneShotScheduleDispatchChange {
  readonly version: 1
  readonly operation: 'dispatch'
  readonly id: ScheduleId
}

export interface EveryScheduleDispatchChange {
  readonly version: 1
  readonly operation: 'dispatch'
  readonly id: ScheduleId
  readonly acceptedAt: string
}

export type ScheduleChange =
  | ScheduleCreateChange
  | ScheduleDeleteChange
  | OneShotScheduleDispatchChange
  | EveryScheduleDispatchChange
```

[`packages/schedule/schedule/src/types.ts:9-105`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/schedule/schedule/src/types.ts#L9-L105)

The version-2 subagent descriptor carries every durable child-composition field. `ToolRestriction` is expanded because it is nested in the continuable variant:

```ts
interface SubagentDescriptorBase {
  readonly version: number
  readonly mode: 'one-shot' | 'continuable'
  readonly provider: string
}

export interface OneShotSubagentDescriptorData extends SubagentDescriptorBase {
  readonly mode: 'one-shot'
  readonly label?: string
}

export interface ContinuableSubagentDescriptorData extends SubagentDescriptorBase {
  readonly mode: 'continuable'
  readonly label: string
  readonly agentProvider?: string
  readonly agentModel?: string
  readonly persona?: string
  readonly toolFilter?: ToolRestriction
}

export interface ToolRestriction {
  readonly allow?: readonly string[]
  readonly deny?: readonly string[]
}

export type SubagentDescriptorData =
  | OneShotSubagentDescriptorData
  | ContinuableSubagentDescriptorData
```

[`packages/subagent/subagent/src/descriptor.ts:47-88`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/descriptor.ts#L47-L88) [`packages/core/tools/src/index.ts:676-685`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/tools/src/index.ts#L676-L685)

The session-title request records the exact auxiliary route, system prompt, message list, and output cap. Its companion title event records the accepted title and provenance:

```ts
export interface SessionTitleModelProvenance {
  readonly provider: string
  readonly model: string
}

export type SessionTitleSource =
  | { readonly kind: 'fallback' }
  | {
    readonly kind: 'provider'
    readonly provider: SessionTitleProviderId
    readonly model?: SessionTitleModelProvenance
  }
  | { readonly kind: 'user' }

export interface SessionTitleEventData {
  readonly title: string
  readonly messageSeqs: number[]
  readonly source: SessionTitleSource
}

export interface SessionTitleLlmRequestEventData {
  readonly titleProvider: SessionTitleProviderId
  readonly messageSeqs: number[]
  readonly route: SessionTitleModelProvenance
  readonly system: string
  readonly messages: Message[]
  readonly maxTokens: number
}
```

[`packages/session/session-title/src/index.ts:39-68`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-title/src/index.ts#L39-L68) [`packages/session/session-title-llm/src/index.ts:24-38`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-title-llm/src/index.ts#L24-L38)

The durable Team event wrappers add `version: 1` and `teamId`; their nested snapshots are:

```ts
export type TeamMemberPhase = 'provisioning' | 'active' | 'failed'

export interface TeamMemberSnapshot {
  readonly id: SessionId
  readonly name: string
  readonly description: string
  readonly provider: string
  readonly context: 'fresh' | 'fork'
  readonly phase: TeamMemberPhase
  readonly error?: string
}

export type TeamTaskStatus = 'pending' | 'in_progress' | 'completed' | 'deleted'

export interface TeamTaskSnapshot {
  readonly id: TeamTaskId
  readonly revision: number
  readonly subject: string
  readonly description: string
  readonly status: TeamTaskStatus
  readonly ownerId?: SessionId
  readonly blockedBy: TeamTaskId[]
  readonly writeScopes: string[]
}

export interface TeamMessageSnapshot {
  readonly id: TeamMessageId
  readonly senderId: SessionId
  readonly senderName: string
  readonly targetId: SessionId
  readonly delivery: 'quiet' | 'wakeup'
  readonly content: ContentBlock[]
}
```

[`packages/experimental/agent-team/src/types.ts:43-107`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/experimental/agent-team/src/types.ts#L43-L107) The four event wrappers, including delivered-message fields, are at [`packages/experimental/agent-team/src/types.ts:203-218`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/experimental/agent-team/src/types.ts#L203-L218).

Code dispatch and workflow event aliases expand as follows. Workflow IDs are branded strings; the terminal unions below are their complete durable value sets:

```ts
export interface CodeDispatchStartEventData {
  rootCallId: CallId
  parentCallId: CallId
  subCallId: CallId
  name: string
  arguments: unknown
}

export interface CodeDispatchEventData extends CodeDispatchStartEventData {
  isError: boolean
  content: ContentBlock[]
}

export interface ToolWorkflowRunStartData {
  readonly runId: WorkflowRunId
  readonly name: string
}

export interface ToolWorkflowAgentStartData {
  readonly runId: WorkflowRunId
  readonly seq: number
  readonly label: string
  readonly phase?: string
  readonly childId: SessionId
}

export type WorkflowAgentOutcome = 'completed' | 'failed' | 'cancelled'

export interface ToolWorkflowAgentEndData {
  readonly runId: WorkflowRunId
  readonly seq: number
  readonly outcome: WorkflowAgentOutcome
}

export type WorkflowStopReason = 'completed' | 'cancelled' | 'error'

export interface ToolWorkflowRunEndData {
  readonly runId: WorkflowRunId
  readonly stopReason: WorkflowStopReason
}
```

[`packages/core/tools/src/types.ts:10-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/tools/src/types.ts#L10-L23) [`packages/workflow/tool-workflow/src/types.ts:13-39`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/tool-workflow/src/types.ts#L13-L39) [`packages/workflow/workflow/src/types.ts:57-63`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/workflow/src/types.ts#L57-L63) [`packages/workflow/workflow/src/types.ts:97-110`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/workflow/src/types.ts#L97-L110)

The DeepSeek search event contains this exact secret-free provider request:

```ts
export interface DeepSeekSearchLlmRequest {
  readonly endpoint: string
  readonly apiVersion: string
  readonly body: {
    readonly model: string
    readonly max_tokens: number
    readonly messages: readonly [{
      readonly role: 'user'
      readonly content: readonly [{
        readonly type: 'text'
        readonly text: string
      }]
    }]
    readonly tools: readonly [{
      readonly type: 'web_search_20250305'
      readonly name: 'web_search'
      readonly max_uses: number
    }]
  }
}
```

[`packages/web/web-search-deepseek/src/provider.ts:52-78`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/web/web-search-deepseek/src/provider.ts#L52-L78)

The remaining named scalar and terminal aliases in the catalog are:

```ts
export type InboxTarget = 'next-turn' | 'next-step'
export type CommandSource = { kind: 'user' }
export type HookDialect = 'claude-code' | 'codex'
export type SandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access'
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }

export type AgentCancelCause =
  | { readonly kind: 'user' }
  | { readonly kind: 'parent' }
  | { readonly kind: 'hook'; readonly reason: string }
  | { readonly kind: 'disposed' }

export type TurnEndCancelCause = AgentCancelCause | { readonly kind: 'legacy' }

export interface TurnEndReasonMap {
  completed: { kind: 'completed' }
  aborted: { kind: 'aborted'; reason: TurnEndCancelCause }
  blocked: { kind: 'blocked' }
  error: { kind: 'error'; error: LlmFailure }
  'max-tokens': { kind: 'max-tokens' }
  interrupted: { kind: 'interrupted' }
}
```

`CommandSource` is written as its current single merged variant; the source declares it through `CommandSourceMap`. All catalog IDs such as `SessionId`, `MessageId`, `CallId`, `CommandId`, `CompactionId`, `RetryId`, `TeamId`, `TeamTaskId`, `TeamMessageId`, and `WorkflowRunId` are opaque branded strings rather than nested records. [`packages/core/agent/src/types.ts:7-10`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/types.ts#L7-L10) [`packages/interaction/commands/src/types.ts:59-70`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/interaction/commands/src/types.ts#L59-L70) [`packages/hooks/hook-protocol/src/types.ts:43-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/hooks/hook-protocol/src/types.ts#L43-L48) [`packages/sandbox/sandbox/src/index.ts:23-29`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/sandbox/sandbox/src/index.ts#L23-L29) [`packages/core/session/src/json.ts:1-13`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/json.ts#L1-L13) [`packages/core/session/src/types.ts:21-31`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L21-L31) [`packages/core/session/src/types.ts:142-177`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L142-L177) [`packages/llm/llm/src/brand.ts:13-39`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm/src/brand.ts#L13-L39) [`packages/interaction/commands/src/brand.ts:13-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/interaction/commands/src/brand.ts#L13-L20) [`packages/compaction/compaction/src/brand.ts:1-4`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction/src/brand.ts#L1-L4) [`packages/llm/llm-retry/src/brand.ts:1-4`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/llm/llm-retry/src/brand.ts#L1-L4) [`packages/experimental/agent-team/src/types.ts:7-40`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/experimental/agent-team/src/types.ts#L7-L40) [`packages/workflow/workflow/src/types.ts:9-21`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/workflow/workflow/src/types.ts#L9-L21)

The store does not treat events as opaque blobs. It validates contiguous `seq`, JSON payloads, known or ignorable event types, surface transitions, turn balance, and repairable tails; the physical backends also parse or pack known record shapes. Within those rules, payload data is preserved losslessly. There is no independent event ID for deduplication, so sequence position and the expected next sequence are the append identity. [`packages/session/session-persistence/README.md:25-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L25-L30) [`packages/core/session/src/types.ts:395-440`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L395-L440) [`packages/session/session-persistence-jsonl/src/format.ts:210-224`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/format.ts#L210-L224)

### Format and schema evolution

`SESSION_FORMAT_VERSION` is `0`. The source explicitly states that the prerelease format provides no compatibility or migration promise. Readers reject another header version. Structural changes to the header, envelope, core event semantics, or surface mechanism require a monotonic format bump; adding an ordinary event type relies on `ignorable`. An unknown event without `ignorable: true` refuses reconstruction. [`packages/core/session/src/types.ts:33-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L33-L56) [`packages/core/session/src/types.ts:408-440`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L408-L440)

The coordinator contains narrow in-version import normalization for older prerelease records, including deterministic legacy message IDs, removed steering records, and older turn shapes. Reads expose the normalized view, but storage stays append-only and old rows are not rewritten. This is explicitly not a general version migration system. [`packages/session/session-persistence/README.md:38-40`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L38-L40) [`packages/session/session-persistence/src/coordinator.ts:273-572`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/coordinator.ts#L273-L572)

JSONL rejects another compression suffix, legacy flat layouts, and mixed roots; it provides no migration or dual write. SQLite has application ID `0x44534850` and physical schema version `17`; it opens schema ownership under `BEGIN IMMEDIATE` and rejects incompatible, foreign, or unversioned databases instead of migrating them. [`packages/session/session-persistence-jsonl/README.md:34-38`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L34-L38) [`packages/session/session-persistence-sqlite/src/schema.ts:17-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/schema.ts#L17-L20) [`packages/session/session-persistence-sqlite/src/schema.ts:107-149`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/schema.ts#L107-L149)

Projection schema evolution is deliberately disposable. The cache domain version drops the whole cache and a projection unit version drops only that row, because replay from the event log can rebuild both. [`packages/session/session-projection-cache/src/spec.ts:16-69`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/spec.ts#L16-L69)

## Compaction and history management

Compaction is logical surface replacement, not durable-log truncation. A summarizing transaction appends `compaction/start`, obtains and records `compaction/summary`, immediately appends a new `user/message` with `surfaceOp: { op: 'replace', start, end }`, and appends `compaction/end`. The replacement cites the start, summary, and all shadowed surface sequences. [`packages/compaction/compaction-basic/src/region.ts:152-254`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction-basic/src/region.ts#L152-L254) [`packages/compaction/compaction-basic/src/region.ts:426-477`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction-basic/src/region.ts#L426-L477)

The underlying `SessionEvent[]`, JSONL artifact, or SQLite event rows still contain the shadowed events and the replacement facts. `Session.surface` applies the replacement, and `deriveMessages()` emits only the resulting visible nodes. Resume reconstructs the same surface by folding operations in sequence. [`packages/core/session/src/types.ts:339-379`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L339-L379) [`packages/core/session/src/index.ts:701-747`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L701-L747)

`compaction/summary` retains the exact summary blocks, replaced range and sequence set, estimated shadow price, provider, model, optional token cap and usage, and optional raw output. A separate `compaction/prune` carries equivalent shadow-price facts for model-free pruning. Both remain log-only. [`packages/compaction/compaction/src/types.ts:16-89`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction/src/types.ts#L16-L89)

There is no backend historical compaction. JSONL files and SQLite rows grow with every logical replacement, and SQLite explicitly lists no background historical compaction. This preserves audit and replay provenance but does not bound storage. [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77) [`packages/session/session-persistence-sqlite/README.md:55-63`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L55-L63)

## Rewind, checkpoints, and fork

There is no destructive rewind or undo operation in `SessionPersistence`. A caller can read an earlier prefix, but no service method changes the active transcript pointer or deletes later events. Persistence checkpoints are durability barriers, and projection checkpoints are fold caches, not execution or filesystem snapshots. [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240) [`packages/session/session-projection/src/index.ts:88-118`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection/src/index.ts#L88-L118)

Fork is a live-session operation. `SessionStore.fork()` selects an inclusive, contiguous prefix of a currently live source, rejects a boundary inside an open turn, creates a new child session, copies the selected event objects into the child's seed, and records `parentSession` plus `seedLength`. The child has its own complete event stream rather than a reference to a shared stored prefix. [`packages/core/session/src/index.ts:1067-1138`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1067-L1138)

Fork does not capture filesystem state, sandbox state, environment state, or tool side effects. It branches only the event history and selected immutable metadata. Any reproducibility of external state must come from other systems. [`packages/core/session/src/index.ts:1081-1094`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1081-L1094) [`packages/core/session/src/types.ts:58-99`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L58-L99)

## Subagents and nested sessions

Session-backed subagents are independent sessions, not nested rows inside a parent log. Their header records `parentSession`, `origin: 'subagent'`, monotonically increasing `delegationDepth`, inherited CWD and preset, and seed length. [`packages/subagent/subagent/src/child-agent.ts:85-120`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/child-agent.ts#L85-L120)

A continuable child receives a UUID-based session ID, its own `Session`, a required persistence seam, and one version-2 `subagent/descriptor` event that preserves provider and resumable composition. The descriptor is log-only and survives surface compaction. [`packages/subagent/subagent/src/continuation.ts:394-475`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L394-L475) [`packages/subagent/subagent/src/descriptor.ts:28-88`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/descriptor.ts#L28-L88)

Orderly process-local shutdown uses `drainContinuableDescendants()`. It closes admission below the exact live parent trees before awaiting, waits for already admitted materializations, propagates cancellation top-down, and releases `AgentHandle`s child-first. Each activation waits for idle and attempts a final session flush, but that flush is best effort so failure cannot pin ancestor ownership. Unrelated parent trees remain live. [`packages/subagent/subagent/src/index.ts:294-325`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/index.ts#L294-L325) [`packages/subagent/subagent/src/continuation.ts:746-841`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L746-L841) [`packages/subagent/subagent/src/continuation.ts:1332-1394`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L1332-L1394)

This drain releases process-local activations, not persisted sessions. Child artifacts and their lineage survive orderly teardown. At this snapshot, the drain path appends no durable child-disposition fact, and no persistence operation records a disposition for a hard process crash, parent terminal state, rewind, or delete. Those cases therefore leave durable child artifacts untouched unless out-of-band maintenance removes them. [`packages/subagent/subagent/src/index.ts:294-325`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/index.ts#L294-L325) [`packages/subagent/subagent/src/continuation.ts:1332-1394`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L1332-L1394) [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240)

Nesting has an optional per-delegation `maxDepth` policy rather than a universal storage limit. Child depth is parent depth plus one, the persisted header is a monotone floor after resume, and creation rejects a child beyond the supplied cap or safe-integer range. [`packages/subagent/subagent/src/child-agent.ts:38-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/child-agent.ts#L38-L56) [`packages/subagent/subagent/src/depth.ts:18-49`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/depth.ts#L18-L49)

Child enumeration merges live sessions with persistent headers and derives descriptor projections, again preferring live state. Direct children and recursive descendants are discovered by following `parentSession`, not by scanning embedded parent messages. [`packages/subagent/subagent/src/list-children.ts:117-180`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/list-children.ts#L117-L180) [`packages/subagent/subagent/src/list-children.ts:183-239`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/list-children.ts#L183-L239)

One-shot providers are not uniformly session-backed. The durable descriptor applies specifically to session-backed children. A provider that executes remotely without a local child `Session` does not automatically produce a local persistence record. [`packages/subagent/subagent/src/descriptor.ts:1-19`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/descriptor.ts#L1-L19)

No cascading delete exists because no delete API exists. Parent and child retention are independent backend artifacts. Out-of-band removal of a parent can leave children whose `parentSession` no longer resolves; out-of-band removal of a child can leave durable parent or workflow facts referring to it. This is an inference from immutable lineage plus the absent deletion operation. [`packages/core/session/src/types.ts:58-99`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L58-L99) [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240)

## Retention, deletion, and multi-host

The persistence seam has no deletion or retention API. JSONL says files accumulate until removed externally; SQLite says normal appends are insert-only and provides no deletion or background historical compaction. Persistent listing is unpaginated and unfiltered. [`packages/session/session-persistence/README.md:81-85`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L81-L85) [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77) [`packages/session/session-persistence-sqlite/README.md:55-63`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L55-L63)

The derived FTS index has a separate deletion behavior. When the persistence observation no longer contains a previously indexed session, reconciliation computes `persistentDeletes`, enters `BEGIN IMMEDIATE`, deletes that session's document and session rows, updates the observed generation, and commits the transaction. This removes missing sources from the search index without adding deletion to the authoritative persistence seam. [`packages/session-query/session-query-sqlite/src/index.ts:395-457`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query-sqlite/src/index.ts#L395-L457) [`packages/session-query/session-query-sqlite/src/index.ts:557-565`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query-sqlite/src/index.ts#L557-L565)

The projection cache differs from the FTS index. It exposes no eviction or retention surface, so per-session projection rows accumulate until an operator prunes them out of band. [`packages/session/session-projection-cache/README.md:58-61`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/README.md#L58-L61)

JSONL explicitly supports one live writer per session. The owning backend instance serializes operations, but a different process or backend instance must wait until that owner is quiescent. Only initial no-overwrite publication is cross-process collision-safe. [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77)

SQLite obtains database write locks with `BEGIN IMMEDIATE`, configures a busy timeout, and checks the current tail before committing. That protects individual database transactions but is not an application-level session lease. The coordinator's revision check may repeatedly retry while an external writer changes a log, and the source notes that this freshness mechanism adds no cross-process exclusion. [`packages/session/session-persistence-sqlite/src/store.ts:173-239`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/store.ts#L173-L239) [`packages/session/session-persistence/README.md:46-59`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L46-L59)

The runtime `SessionStore` remains process-local, the JSONL owner rule is single writer, and the FTS index is single-owner. DeepSeek Harness therefore supplies local durability and cold resume, not a distributed active-active session service. This is an inference from the explicit ownership and concurrency boundaries. [`packages/core/session/src/index.ts:786-794`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L786-L794) [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77) [`packages/session-query/session-query-sqlite/README.md:52-57`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session-query/session-query-sqlite/README.md#L52-L57)

No source-backed network-filesystem lease, host failover protocol, remote writeback queue, or distributed crash detector was found. JSONL relies on local filesystem primitives such as hard links, rename, fsync, and a single live writer, so network-filesystem semantics remain an explicit gap rather than a supported multi-host path. [`packages/session/session-persistence-jsonl/src/index.ts:513-625`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L513-L625) [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77)

## Interop with foreign session stores

The seam is pluggable at the logical `SessionHeader` plus contiguous `SessionEvent[]` boundary, so a third-party backend can implement the abstract service or the coordinator's backend hooks. It must still honor current format refusal, JSON serializability, contiguous sequences, cold repair, immutable inspection, trustworthy revisions, and live-session exclusion. [`packages/session/session-persistence/src/index.ts:78-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L78-L240) [`packages/session/session-persistence/README.md:46-59`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L46-L59)

There is no generic import/export protocol for another product's transcript, no foreign schema adapter, and no migration API. JSONL alone exposes exact raw artifact text through `readRaw`; SQLite intentionally does not expose a per-session raw artifact. Logical export is available only by reading headers and events and translating them externally. [`packages/session/session-persistence/src/index.ts:89-124`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L89-L124) [`packages/session/session-persistence-jsonl/src/index.ts:252-281`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/src/index.ts#L252-L281) [`packages/session/session-persistence-sqlite/src/index.ts:52-132`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/src/index.ts#L52-L132)

Because the payload map is merge-extensible, a foreign consumer needs the complete non-ignorable plugin vocabulary present in the log. An unknown event marked `ignorable: true` is deliberately skippable; an unknown event without that marker must refuse reconstruction. Silently treating only core messages as the session would lose approvals, policy, goals, schedules, lineage descriptors, compaction operations, and other required reconstruction state. [`packages/core/session/src/types.ts:230-240`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L230-L240) [`packages/core/session/src/types.ts:408-440`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L408-L440) [`docs/persistence-catalog.md:1-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1-L20)

## What this implies for our Session Store (our inference)

Our inference: a stored DeepSeek Harness session comes into existence when its first event materializes an immutable header plus a contiguous event prefix. Its durable identity is that header and log, while messages, summaries, titles, listing state, projection checkpoints, and search indexes are replayable views. [`packages/session/session-persistence/src/index.ts:126-143`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/src/index.ts#L126-L143) [`packages/session/session-persistence/README.md:5-7`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L5-L7)

1. **Keep the durable log authoritative and projections disposable.** DeepSeek Harness has a clean failure model because transcript views, titles, child summaries, listing metadata, and FTS search can be recomputed from the same ordered event stream. We should preserve this one-way dependency and never let a projection cache become an alternate write authority. [`packages/session/session-projection-cache/src/index.ts:1-12`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-projection-cache/src/index.ts#L1-L12)

2. **Keep DeepSeek's two commit points explicit at integration boundaries, but do not copy them into our authority model.** A DeepSeek session can acknowledge an in-memory append before backend durability, so an adapter must state whether it observed live acceptance or the persistence barrier. ADR 0035 instead keeps our authoritative success boundary at the durable append; transient UI progress remains a non-authoritative projection. [`packages/core/session/src/index.ts:569-655`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L569-L655) [`packages/core/session/src/index.ts:1009-1039`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1009-L1039) [ADR 0035](../../../../adr/0035-session-store-decider-aggregate.md)

3. **Use a typed envelope and require explicit skippability.** A format version alone cannot safely govern a plugin-extensible event vocabulary. The `ignorable: true` rule is a strong pattern: unknown required facts stop resume instead of silently producing a plausible but wrong state. [`packages/core/session/src/types.ts:408-440`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L408-L440)

4. **Model repair as append-only semantic closure.** Preserving complete facts, dropping only torn bytes, and appending typed assistant interruption closure is safer than truncating to the last successful turn. An unmatched durable tool call must remain outcome-unknown until the operation ledger reconciles it rather than being rewritten as a failure. [`packages/session/session-persistence/README.md:25-30`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L25-L30) [`packages/core/session/src/repair.ts:89-123`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/repair.ts#L89-L123)

5. **Treat local JSONL or SQLite as a recovery artifact, not the product-wide session identity authority.** DeepSeek Harness assumes a local process owner and provides no authorization, tenant scope, lease, or distributed ordering layer. Our store needs those concerns above or inside its durable service if sessions can move across workers or hosts. [`packages/session/session-persistence-jsonl/README.md:70-77`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-jsonl/README.md#L70-L77) [`packages/session/session-persistence/README.md:46-59`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L46-L59)

6. **Use DeepSeek Harness as evidence for explicit fork lineage, not as our storage design.** Its copied-prefix child validates immutable parent and boundary metadata, but ADR 0035 Decision 5 already requires atomic, self-contained fork creation whose inherited conversation prefix is resolved by explicit reference in the context projection. Physical O(history) copies and content-addressed snapshot sharing are rejected. [`packages/core/session/src/index.ts:1067-1138`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/index.ts#L1067-L1138) [`docs/adr/0035-session-store-decider-aggregate.md:938-1025`](../../../../adr/0035-session-store-decider-aggregate.md)

7. **Treat compaction as model-view reduction over a keep-forever log.** DeepSeek Harness's non-truncating surface replacement validates ADR 0035 Decision 7. Our Session log is never truncated or purged; replay is bounded by snapshots, while read-time `RedactionApplied` and `ArtifactErased` handle privacy without removing event facts. [`packages/compaction/compaction-basic/src/region.ts:426-477`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/compaction/compaction-basic/src/region.ts#L426-L477) [`docs/adr/0035-session-store-decider-aggregate.md:1159-1247`](../../../../adr/0035-session-store-decider-aggregate.md)

8. **Keep lifecycle visibility and privacy separate from byte deletion.** DeepSeek Harness exposes the cost of having no durable lifecycle API, but ADR 0035 already settles our path: `SessionHidden` removes default visibility, `RedactionApplied` masks event content, and `ArtifactErased` destroys referenced artifact bytes while the Session log remains keep-forever. Erasure-grade deletion beyond masking remains a named follow-up, not an asynchronous purge proposal. [`packages/session/session-persistence/README.md:81-85`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L81-L85) [`docs/adr/0035-session-store-decider-aggregate.md:1169-1222`](../../../../adr/0035-session-store-decider-aggregate.md)

## Open questions

- What is the intended stable migration path after prerelease `SESSION_FORMAT_VERSION = 0`, especially for logs carrying plugin-defined required events? [`packages/core/session/src/types.ts:33-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/src/types.ts#L33-L56)
- Will the persistence seam gain deletion, retention, pagination, and filtered listing, or remain deliberately local with out-of-band administration? [`packages/session/session-persistence/README.md:81-85`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L81-L85)
- Is SQLite expected to support multiple cooperating runtime processes, or are its lock and revision checks only defensive access around a single application owner? [`packages/session/session-persistence/README.md:46-59`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence/README.md#L46-L59)
- How will DeepSeek Harness bound physical event-log growth while preserving the current audit and reconstruction guarantees of surface compaction? [`packages/session/session-persistence-sqlite/README.md:55-63`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/session/session-persistence-sqlite/README.md#L55-L63)
- Should a future portable export package the complete required plugin event vocabulary, known ignorable records, and attachment references, rather than only a raw JSONL artifact or reconstructed messages? [`docs/persistence-catalog.md:1-20`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/persistence-catalog.md#L1-L20)
