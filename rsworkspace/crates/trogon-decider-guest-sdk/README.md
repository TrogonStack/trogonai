# trogon-decider-guest-sdk

Bridges a native [`Decider`](../trogon-decider) implementation to the Trogon WIT guest
contract, so it can be compiled to a WASM component and executed by
[`trogon-decider-wasm-runtime`](../trogon-decider-wasm-runtime) (or any other host that binds the
same `trogon:decider` world).

This guide covers the `Decider` trait, the `export_decider!` macro, the WIT contract it targets,
building a component, and the two ways to test one.

## The `Decider` trait

A decider is a plain Rust type implementing `trogon_decider::Decider`:

```rust
pub trait Decider: Sized {
    type StreamId: ?Sized;
    type State;
    type Event;
    type DecideError: std::error::Error;
    type EvolveError: std::error::Error;

    const WRITE_PRECONDITION: Option<WritePrecondition> = None;

    fn stream_id(&self) -> &Self::StreamId;
    fn initial_state() -> Self::State;
    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError>;
    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError>;
    fn decide_error_code(error: &Self::DecideError) -> &str { "rejected" }
}
```

`decide` returns a `Decision<Self>`, which is either a flat batch of events
(`Decision::Events`) or an `Act` plan: a chain of steps, each re-evaluating `decide` against the
state produced by the previous step, that folds into one ordered event batch. The guest bridge
(`decide_command` below) evaluates both forms through the same `evaluate_decision` entry point
the native runtime uses, so a decider's WASM behavior is identical to its native behavior,
`Decision::Act` included.

`Self::Event` must also implement `EventEncode` (`encode(&self) -> Result<Vec<u8>, Error>`),
`EventType` (`event_type(&self) -> Result<&'static str, Error>`), and `EventDecode`
(`decode(EventData<'_>) -> Result<EventDecodeOutcome<Self>, Error>`, where `EventDecodeOutcome`
is `Decoded(event)` or `Skipped` for envelopes outside this decider's event set) so the bridge
can serialize decided events and replay stored ones.

## `export_decider!`

`export_decider!` generates the WIT component glue (`wit_bindgen::generate!`, the exported
`Guest`/`GuestSession` impls, snapshot load/store, command dispatch) for one or more commands
that share a `Decider::State` and `Decider::Event`:

```rust
use trogon_decider_guest_sdk::export_decider;

export_decider!(
    CreateSchedule {
        type_url = CREATE_SCHEDULE_TYPE_URL,
        proto = v1::CreateSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
        write_precondition = no_stream,
    },
    PauseSchedule {
        type_url = PAUSE_SCHEDULE_TYPE_URL,
        proto = v1::PauseSchedule,
        module = "scheduler.schedules",
        version = "0.1.0",
        state_schema_version = SCHEDULES_STATE_SCHEMA_VERSION,
    },
);
```

Each entry is `CommandType { ... }`, where `CommandType` implements `Decider` (and `TryFrom<Proto>`
for the wire command). Fields:

- `type_url`: the protobuf `Any` type URL this command decodes from.
- `proto`: the wire (buffa-generated) protobuf message type; `CommandType: TryFrom<proto>`.
- `module` / `version`: the exported `ModuleDescriptor`'s name and version. Every command in a
  bundle must declare the same values (checked at macro-expansion time).
- `state_schema_version`: fed to the snapshot codec; every command in a bundle must declare the
  same value (also checked at macro-expansion time, since a bundle shares one `State`).
- `write_precondition` (optional): `any`, `stream_exists`, `no_stream`, or `default` (the
  decider's own `Decider::WRITE_PRECONDITION`, which is also the default when omitted).

When a bundle has more than one command, `export_decider!` requires every command's `Decider`
impl to share the canonical (first-listed) command's `State` and `Event` types, and asserts this
independently of the WIT bindings so a mismatch fails to compile with a clear error rather than a
confusing `wit-bindgen` diagnostic.

The macro resolves its WIT contract via `trogon_decider_wit::WIT_DIR`, an absolute path baked
into `trogon-decider-wit` at that crate's own compile time. This means a component crate can live
at any directory depth relative to `trogon-decider-wit`; the macro does not depend on the calling
crate's location.

## WIT contract summary

The contract lives in `trogon-decider-wit`'s `wit/world.wit` (package `trogon:decider`, world
`decider`, interface `handler`):

- `module-descriptor { name, version, commands: list<command-spec> }` and
  `command-spec { command-type, write-precondition: option<write-precondition> }`, returned by
  `descriptor()`.
- `command-envelope { type, payload }` and `any-envelope { type, payload }`: type-URL-tagged
  protobuf bytes for commands and events, respectively.
- `domain-error { code, message, details: list<tuple<string, string>> }`: a stable machine
  `code`, a human-readable `message`, and `details` as ordered key/value pairs carrying the
  error's `#[source]` chain (`("cause.0", ...)`, `("cause.1", ...)`, ...) below `message`, so
  causes that `message`'s single `Display` line would otherwise erase remain available in
  structured form.
- `decide-error` is a variant of `rejected(domain-error)` or `faulted(domain-error)`.
- `resource session { constructor(snapshot), evolve(events), decide(command), snapshot() }`: one
  session per component instance, holding in-memory state across a `decide`/`evolve` cycle.

## Building a component

A component crate needs:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
trogon-decider = { workspace = true }
trogon-decider-guest-sdk = { workspace = true }
wit-bindgen = { workspace = true }
# ...your domain crate(s), proto crate(s)

[package.metadata.component]
package = "trogon:decider"
```

`wit-bindgen` must be a direct dependency of the component crate itself (not just of
`trogon-decider-guest-macros`), because `export_decider!`'s generated code calls
`wit_bindgen::generate!` unqualified, resolving in the caller's own dependency graph.

Place the crate under `rsworkspace/wasm-components/<name>/` and build it with:

```sh
mise run artifacts:wasm-components
```

which builds every crate under `wasm-components/*` with `cargo component build --target
wasm32-unknown-unknown --release` and copies each `.wasm` (and its `.binpb` FileDescriptorSet, if
one was generated) to `dist/decider/<alias>.wasm`, where `<alias>` is the crate name with the
`trogon-` prefix and `-decider` suffix stripped. You can also build a single component directly:

```sh
cargo component build -p <crate-name> --target wasm32-unknown-unknown --release
```

## Testing

Two complementary paths exist, both driving the built `.wasm` through an in-memory wasmtime host
rather than a real deployment:

### `trogon-decider-sim`

`trogon-decider-sim` is a Rust-level in-memory host (`SimHost`, `SimInstance`, `SimScenario`) for
integration tests that construct protobuf messages directly in Rust. Add a fixture accessor next
to `SimFixture::schedules()` in `trogon-decider-sim/src/fixture.rs` (feature `test-support`)
pointing at your component's built `.wasm` path, then write a test under
`trogon-decider-sim/tests/` following the pattern in `tests/schedules.rs`: build a `CommandEnvelope`
protobuf, run it through `SimScenario`, and assert on the resulting `AnyEnvelope`s or the returned
`decide-error`.

### `decider-test` CLI

`cli/trogon-decider-test` runs a YAML conformance suite against a compiled component without
writing any Rust:

```sh
cargo build -p trogon-decider-test --release
./target/release/decider-test dist/decider/<alias>.wasm cli/trogon-decider-test/<alias>.yaml
```

A suite file has a top-level `suite` name and a list of `scenarios`, each with:

- `given`: a list of protobuf-JSON `Any` events (`'@type'` plus the message's fields) to replay
  into state before the command runs.
- `when`: the protobuf-JSON `Any` command to decide.
- `then`: one of
  - `events: [...]`: the exact ordered list of protobuf-JSON `Any` events the command must
    produce.
  - `rejected: true`: the command must be rejected (`decide-error`), without checking `code` or
    `message`.
  - `error: { code: ..., message: ... }` (or a bare string): the command must be rejected or
    faulted with a matching `code` or `message`.

See `cli/trogon-decider-test/schedules.yaml` for a full example covering creation, rejection, and
multi-step scenarios.
