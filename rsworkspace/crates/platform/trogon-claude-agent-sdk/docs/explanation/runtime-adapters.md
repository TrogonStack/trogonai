# Runtime adapters

The protocol runtime uses generic traits at its operating-system boundaries so
tests can replace effects while exercising the same startup, callback and
shutdown logic used in applications.

`ClaudeSdkClient::connect()` and the ordinary query helpers select
`RuntimeAdapters::default()`: `SystemEnvironment`, `TokioProcessSpawner` and
`NativeFileSystem`. Applications can pass `RuntimeAdapters` to
`connect_with_adapters()`, `query_with_adapters()` or
`query_turn_with_adapters()` to replace those implementations.

## Effects and ownership

Each adapter method is a direct pass-through of one native effect and returns
exactly what the native call returns. Adapters do not reshape results into
wrapper types; the runtime reads what it needs from the native value at the
call site.

`EnvironmentVariable` and `CurrentDirectory` supply configuration lookups.
`SpawnProcess::spawn` takes the `tokio::process::Command` the runtime built and
returns the `tokio::process::Child` from `Command::spawn`. The driver reads the
process id once, right after spawn, takes the stdin, stdout and stderr pipes,
and owns that concrete child for awaited shutdown and the native kill-on-drop
fallback. Status checks, waiting and kill requests act on the native child and
are not adapter calls.

`SessionFileSystem` groups the operations store-backed resume uses to build and
remove its temporary configuration directory: `CanonicalizePath`, `OpenFile`
(returns `std::fs::File`), `CreateDirectory`, `ReadMetadata` (returns
`std::fs::Metadata`), `RemoveDirectoryAll` and `RemoveFile`. Each delegates to
the matching `std::fs` call. `TemporaryDirectory` supplies the root for that
directory, and the process adapter runs the macOS keychain lookup, so tests can
refuse it. A failed optional configuration copy removes the partial file and
continues.

`SessionRuntime::prepare_with_adapters(SessionPreparationOptions { ... })`
exposes the same boundary when preparing stored sessions directly.
`TranscriptMirror` needs no filesystem adapter: it parses transcript paths
lexically and sends entries to the configured `SessionStore`.

`InMemorySessionStore::with_clock()` accepts a `WallClock` implementation for
timestamps. Retry delays and deadlines use Tokio time; tests can pause and
advance it without sleeping in real time.

## Dispatch and test scope

The effect adapters use static dispatch through generic parameters and
associated types. Replacing them adds no trait-object dispatch or boxed adapter
futures. The public client remains a single owner type because its spawned
driver owns the concrete child and materialized-session filesystem. Existing
application callbacks and `SessionStore` remain separate interfaces with their
existing dynamic dispatch.

The environment adapter controls SDK lookups. The native process adapter still
inherits the child environment according to `Command` behavior and applies
`ClaudeAgentOptions.env`; replacing the lookup adapter does not replace that
inheritance.

The `test-support` feature provides `FixedEnvironment`. Runtime tests replace
effects with decorators that wrap the native implementation: a decorator
records the inputs of each call, can return an injected `Err` in place of the
effect, and otherwise delegates to the native call. The process decorator
records the command the runtime built and spawns a relay fixture through the
native spawner, so the test scripts what the child writes and when it exits
while the runtime drives a real subprocess. The filesystem decorator delegates
to native calls inside a temporary directory and can replace a file between
the runtime's calls to exercise identity checks. Behavior that only a native
child or file can produce, such as a failed wait, is not covered by these
decorators. Neither kind of test establishes live model-session conformance.
