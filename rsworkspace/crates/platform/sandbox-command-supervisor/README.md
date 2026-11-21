# Sandbox command supervisor

The supervisor holds a Linux command before launch and tracks its process family
through completion or cancellation. It uses pidfds, a private session, and child
subreaping to retain process identity and reap adopted descendants, including
children that create new sessions.

This is a command lifecycle primitive. The surrounding worker must provide its
container or VM isolation, resource policy, and authorization.

## Run a capability probe

From `rsworkspace/`:

```sh
mise exec -- cargo run -p sandbox-command-supervisor --bin sandbox-command-supervisor -- --probe
```

The executable emits a JSON `probe` frame and exits with code `0` when its kernel
prerequisites are available. Unsupported platforms or prerequisites produce code
`78`. Linux requires procfs, pidfds, `close_range`, and subreaper support. The
supervisor cannot run as PID 1.

Failure to deliver the probe frame exits with code `74`.

## Control a command

Start the executable with `--execution-id <id>`, then exchange newline-delimited
JSON on stdin and stdout. Version 1 control flow is:

1. Read `hello` and send `init` with the same execution identity, protocol
   version, argv, and explicit child environment.
2. Read `ready`, which identifies the held root, supervisor, process group, and
   session. Send `release` to let the held command execute, or cancel it while
   held without executing user work.
3. Read output and lifecycle events. A `root_exited` event retains the root's
   status while descendants may still be running. `family_absent` is the
   terminal cleanup receipt.
4. Send `acknowledge` after terminal absence to release the supervisor anchor.

The `release` command opens the launch gate. It does not request cleanup.
Control EOF, deadlines, and hard cancellation drain the owned family. Signal
request IDs must increase for new requests; an identical retry replays the
previous result without delivering the signal again. Soft signals use a bounded
snapshot of pinned workload descendants and send once per retained process. The
snapshot visits every owned process task and its children, with at most 65536
traversal entries for the production capacity. A capacity or ownership uncertainty
starts cleanup instead of sending from a partial snapshot. Forks after a snapshot
remain best effort until cleanup. The worker descriptor limit must accommodate a
snapshot pidfds alongside retained cleanup handles and control and stream
descriptors; descriptor exhaustion also starts cleanup.

The Rust API separates untrusted `CommandWire` and `EventWire` records from
validated `Command` and `Event` values. The decoders validate at the input
boundary. A validated event exposes an immutable wire view through `as_wire()`
or transfers its wire record through `into_wire()`. Event flag enums
retain their version 1 JSON boolean encoding. Stdout and stderr bytes are carried
inside output frames, never interpreted as control messages.

## Verify

```sh
mise exec -- cargo test -p sandbox-command-supervisor --all-features
mise exec -- cargo clippy -p sandbox-command-supervisor --all-features --all-targets
```

The native contracts require Linux and execute real process families. Other
platforms run the protocol contracts. The additive `test-support` feature builds
the workload fixture and a separate supervisor fixture with a capacity of 8 for
overflow controls. The production supervisor always retains a capacity of 4096,
including builds with all features enabled.

Kernel tests run in a fresh executable worker so each case owns its process
family. Full `cargo test` builds that worker automatically. Before a focused
`cargo test --lib` run, build it explicitly. The harness rejects a worker whose
embedded source fingerprint differs from the test binary and prints the rebuild
command. The fingerprint checks source freshness rather than authenticity:

```sh
mise exec -- cargo build -p sandbox-command-supervisor --features test-support --bin supervisor-kernel-test-worker
```

See the repository's [sandbox foundation architecture](../../../../docs/architecture/sandbox-foundation.md)
for how this package fits with the client SDK and future worker integrations.
