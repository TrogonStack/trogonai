# SDK lifecycle and API changes

Read [the lifecycle decision](docs/explanation/lifecycle.md) before changing
client ownership, startup, query completion, hooks or shutdown.

- Keep one bidirectional protocol runtime behind the interactive client and
  query helpers. Return a usable owner only after asynchronous initialization.
  Decode connection metadata once at that boundary and expose it through the
  non-fallible cached `connection_info()` getter.
- Use named configuration structs for constructors that assemble several
  independent settings. Name SDK-owned public constructor and input settings
  types `*Options`, whether fields are required or have defaults. Keep required
  fields explicit in the current Options pattern.
- Scalar text values (prompts, names, identifiers, tool rules) are infallible
  opaque storage; enforce CLI and wire safety where the runtime builds process
  arguments or frames. Validate other independent field invariants in value
  objects with private fields and typed construction errors, each in its own
  file. Assemble validated fields
  infallibly instead of repeating those checks in an aggregate constructor.
  Validate collection invariants at an immutable collection boundary:
  `SdkTools` rejects duplicate exact tool names before infallible
  `LocalMcpServer::from` assembly. Preserve its private registry and immutable
  access; keep `Default` empty and single-tool conversion infallible.
- Register callbacks before user input. Service controls and callbacks in the
  driver, with bounded queues, callback admission and waits. Preserve explicit
  overflow errors rather than silently dropping messages.
- Distinguish turn results, complete runs, input exhaustion, conversation
  identity and process lifetime. Preserve `query()` versus `query_turn()`.
- Keep `disconnect(self).await` as the observable cleanup boundary. `Drop` requests
  best-effort cleanup; cloned command handles must not prolong the owner's life.
- Preserve callback cancellation signals and request/task identity checks.
  Cancellation suppresses stale responses; it does not undo external effects.
- Treat hooks as CLI events with event-specific semantics. Negotiate blocking
  failure support and reject an explicit failure to apply registered hooks.
  Never depend on `Stop` or `SessionEnd` delivery for application cleanup.
- Preserve upstream functionality through wire behavior and public regressions.
  Rust names and ownership may differ. Update affected examples, documentation,
  tests and compatibility mappings together when changing a public API.
- Keep fixture evidence distinct from native CLI and model-session conformance.
  Document version-dependent limitations without inferring support from a type
  or hook name alone.
- Keep operating-system effects behind the statically dispatched traits in
  `adapters`; native implementations supply defaults. Add a trait only for an
  operation that performs an effect. Each adapter method is a direct
  pass-through of the native effect and returns exactly what the native call
  returns, such as `tokio::process::Child` and `std::fs::Metadata`; never add
  wrapper result types. Test with decorators that record inputs, inject `Err` returns and
  otherwise delegate to the native call. See [runtime adapters](docs/explanation/runtime-adapters.md).

- Preserve the approved Python-aligned configuration: `cli_path: Option<PathBuf>`,
  unrestricted `model: Option<ModelName>`, optional `permission_mode` and
  `setting_sources`. Omission differs from explicit default or an empty list.
  Refuse Windows batch CLI paths at process start, before the spawn adapter.
- Follow Python lifecycle names with Rust casing: `ClaudeSdkClient` and consuming
  `disconnect(self)`. Keep deterministic contract mappings with each capability;
  a separate compatibility gate is required before runtime slices land.
