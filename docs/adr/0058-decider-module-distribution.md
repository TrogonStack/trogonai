---
number: "0058"
slug: decider-module-distribution
status: draft
date: 2026-08-15
---

# ADR#0058: Decider Module Distribution

## Context

`WasmDeciderModule::load(engine, bytes)`
(`crates/decider/trogon-decider-wasm-runtime/src/module.rs`) compiles a decider
component from bytes it is handed. Until
[ADR#0057](./0057-decider-command-nats-binding.md)'s host landed, nothing in the
workspace produced those bytes outside of tests. The host produces them the only
way a first host can: it reads local files named on its command line.

That is enough to serve, and not enough to operate. A file path is a property of
one machine, so it cannot be the thing a deployment names, a rollout references,
or an audit records. Three concrete gaps follow from it:

- **A module has no name a deployment can use.** The component itself declares a
  `ModuleName` and `ModuleVersion` in its descriptor, and the registry, the
  snapshot identity, and the event subtree are all keyed by them. The one place
  that does *not* use them is the way an operator asks for a module.
- **Nothing checks that the bytes are the module that was asked for.** `load`
  accepts whatever descriptor the component reports. A file copied to the wrong
  name, or a bucket entry overwritten with a different build, loads and becomes
  routable under its own declared identity rather than the requested one.
- **[Issue #470](https://github.com/TrogonStack/trogonai/issues/470)'s gate has
  nowhere to stand.** A conformance gate rejects an *artifact*, which requires
  there to be a moment at which an artifact is published. Copying a file to a
  server is not that moment.

The two candidate stores are an OCI registry and a JetStream object store. The
decision below picks between them, but the more important decision is that the
host does not know which one it is talking to.

## Decision

### 1. A module is named by a reference, never by a path

A `ModuleReference` is a `ModuleName` and a `ModuleVersion`, written
`{name}@{version}` (`scheduler.schedules@0.1.0`). Both halves already reject `@`
and `/` (`module_name.rs`, `module_version.rs`), so the split is unambiguous and
the projection is total in both directions.

This is the same identity the component declares, the registry routes on, and
`WasmSnapshotId` folds into its snapshot key. Naming the module the same way
everywhere is what makes "which build is serving `scheduler.schedules`" a
question with one answer.

A host is configured with a list of references and one store to resolve them
against. It is never configured with a path to a `.wasm` file, because a path
answers "where on this machine" when the operational question is "which module".

### 2. The store is behind a trait, and the host does not know which one it has

`ModuleSource` is one method: given a `ModuleReference`, produce the component's
bytes. Everything above it, the host included, sees only that.

This is not speculative generality. It is what keeps the choice in section 3
reversible: an OCI source, a filesystem source for local development, and a test
double are the same shape, and adding the first does not touch the host, the
registry, or the runtime.

### 3. The first real store is a JetStream object store, not OCI

A JetStream object store, because everything it needs is already deployed and
already wrapped:

- Every decider host already holds a `jetstream::Context`. It has to, to reach
  its event stream. An object store adds no new dependency, no new port, no new
  credential, and no new failure mode to a deployment that is already running
  NATS.
- `trogon-nats` already has `NatsObjectStore` with the `ObjectStoreGet` /
  `ObjectStorePut` seam (`src/jetstream/object_store.rs`), built for the claim
  check path. Module distribution is the same primitive with a different key.
- The bucket is replicated, mirrored, and access-controlled by the same
  mechanisms as every other stream. An operator who can reason about the event
  stream can reason about the module bucket.

OCI is the better answer *eventually*, and specifically when modules cross an
organizational boundary: it has signing, provenance attestation, and a
distribution network, and none of those are things to reinvent. It is not the
better answer *first*, because it introduces a registry to run, credentials to
rotate, and a media-type contract to define, all to deliver a property (bytes,
by name and version) that the object store already delivers today. Section 2 is
what makes deferring it cheap.

The object key is the reference with its `@` replaced by `/`
(`scheduler.schedules/0.1.0`). The object-name grammar async-nats enforces is
`[-/_=.a-zA-Z0-9]+`, which admits `/` and excludes `@`; the substitution is the
whole of the projection, and it is injective because neither half of a reference
can contain either character.

### 4. A fetched component must be the module that was requested

After compiling the bytes, the host compares the descriptor's declared name and
version against the reference it fetched. A mismatch fails startup and names both
sides.

Without this, the reference is a hint rather than an identity: the store can be
asked for `scheduler.schedules@0.2.0`, return the `0.1.0` build because someone
overwrote the object, and the host will serve it, route it correctly, and report
`0.1.0` in every span. The failure is silent precisely because every subsystem
downstream trusts the descriptor. The check belongs here, at the one place that
knows both what was asked for and what arrived.

This is the load-time analogue of
[ADR#0057](./0057-decider-command-nats-binding.md) section 3's rule that a subject whose
recovered command type no activation claims is rejected rather than guessed at.

### 5. Publishing is the gate, serving is not

The conformance gate of
[issue #470](https://github.com/TrogonStack/trogonai/issues/470) runs when an
artifact enters the store, not when a host reads it. A host that re-ran a YAML
suite on every start would be paying for it on every rollout, on every replica,
and would still be a fresh opportunity for a component that already failed the
gate to reach production by being copied into the bucket directly.

Two properties are checked at publish time and neither is rechecked at serve
time:

| Property | Checked by |
| --- | --- |
| Declares zero imports and is a valid component | `assert_zero_imports` plus the structural empty-`Linker` instantiation `load` already performs |
| Passes its `decider-test` YAML suite | `cli/trogon-decider-test`, which is already a hard gate (see the CLI half of #470) |

Both run inside `cli/trogon-decider-publish`, which is the only supported way a
component enters the store. It runs the suite through the same
`trogon_decider_test::conformance::run_suite` the CI gate calls, so there is one
definition of "conformant" rather than a second one that drifts; then it loads
the component exactly as a host will, and takes the reference it publishes under
out of the component's own descriptor. Nothing is written unless every step
passes, down to not provisioning the bucket, so a refused publish and a bucket
nobody ever published into are the same observable state.

Zero imports is the one property that *is* also enforced at serve time, and only
because it costs nothing: `load` already instantiates against an empty `Linker`,
so a component with imports cannot compile into a `DeciderPre` at all. That is
structural, not a second gate.

## Invariants

- A host is configured with references and a store. A `.wasm` path never appears
  in a host's configuration.
- A module's identity is its descriptor's name and version. The reference, the
  object key, the registry key, the snapshot key, and the event subtree all
  derive from that one identity and never from a filename.
- A component whose descriptor disagrees with the reference that fetched it never
  becomes routable.
- The host never learns which `ModuleSource` implementation it holds, and never
  branches on it.
- An artifact that has not passed the conformance gate is not published. The
  gate is not re-run at serve time and a host does not verify that it ran.

## Alternatives Considered

### Keep configuring hosts with filesystem paths

Rejected on all three gaps in the context. It is worth naming what it does get
right: a path needs no store to be running before the host can start, which is
why the filesystem stays as a `ModuleSource` implementation for local
development and tests. What it cannot be is the deployment interface.

### OCI registry first

Rejected for now, on cost rather than on merit; section 3 gives the reasoning and
section 2 keeps the door open. The trigger to revisit is modules crossing an
organizational boundary, at which point signing and provenance stop being
features the object store lacks and start being the point.

### JetStream KV instead of the object store

Rejected. KV values are single messages, so a component larger than the server's
`max_payload` (1 MiB by default) cannot be stored at all, and a decider component
compiled from Rust is comfortably above that. The object store chunks, which is
the entire difference that matters here.

### Content-address modules by digest instead of by version

Rejected as the *primary* identity, kept as a possible addition. A digest is the
right thing to pin a deployment to and the wrong thing to route on: the registry,
the snapshot key, and every span already speak `name` and `version`, and a
digest-keyed store would need a version-to-digest map, which is a second naming
system that can disagree with the descriptor. Section 4's check gives the
integrity property a digest would have given, against the identity the rest of
the system already uses.

### Verify the reference by trusting the store's key

Rejected. It checks that the object was filed correctly, not that the bytes are
what the file claims. The descriptor is the component's own statement of what it
is, so comparing against the descriptor is the only comparison that can catch a
mis-published artifact.

### Re-run the conformance suite at host startup

Rejected per section 5. It moves a publish-time cost onto every replica start,
and it cannot be a real gate anyway, because the host has no way to obtain the
YAML suite that describes the module it just fetched.

## Consequences

- A deployment gains a bucket to provision and fill. `mise run
  artifacts:wasm-components` produces the artifacts; publishing them becomes a
  deliberate, gated step rather than a file copy.
- Rolling a module back is publishing nothing: both versions are resident in the
  bucket, and the host's reference list is the only thing that changes.
- `WasmDeciderModule::load` keeps taking bytes. The source seam sits above it,
  which keeps the runtime free of any notion of where a component came from and
  keeps the sim host, the tests, and the CLI loading from memory.
- The reference grammar is now load-bearing in two places, the human-facing
  `name@version` and the object key `name/version`. They are one projection in
  one type; anything that needs a third encoding adds it there.

## Related ADRs

- [ADR#0016: Protocol Buffers RPC over NATS micro Binding](./0016-protobuf-rpc-over-nats-micro-binding.md)
- [ADR#0045: Aggregate-Oriented Module Layout for Event-Sourced Services](./0045-event-sourced-service-module-layout.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
