# trogon-sandbox-hyperlight-worker

Serves sandbox leases as Wasm components in hypervisor-isolated guests.

The worker queue-subscribes to Hyperlight lease dispatches, loads the component or snapshot a lease
names into a guest, and keeps that guest until the lease is cancelled. A cancelled guest goes back to a
bounded warm pool. Snapshots travel as OCI artifacts of type `HyperlightSnapshotV1`.

## The hypervisor seam

Everything that touches a hypervisor sits behind the `Hypervisor` trait. The lease rules, the warm pool,
the snapshot transport and the authority guard are plain code that runs and is tested without one.

`WasmHypervisor` runs guests through `hyperlight-wasm`. It is built only when the
`trogon_hyperlight_wasm` rustc cfg is set, and only on Linux (KVM or MSHV) and Windows (WHP). With the
cfg the binary serves leases on it.

Every other build uses `UnsupportedHypervisor`. The binary then exits at startup with a clear error
instead of advertising capacity it does not have. A deployment that has another hypervisor backend
passes it to `serve` or `launch`.

## Building the Hyperlight backend

The backend is a rustc cfg rather than a Cargo feature, so `--all-features` builds leave it out. Its
build script compiles the `hyperlight-wasm` guest runtime for a custom target, which stable Rust stopped
accepting in 1.95, so a feature would break every workspace-wide `--all-features` build on Linux.

To build it you need:

- A toolchain that accepts custom target specifications: Rust 1.94, a nightly, or a newer stable with
  `RUSTC_BOOTSTRAP=1`.
- The `rust-src` component of that toolchain, to build the guest sysroot.
- `clang` on the `PATH`, to compile the guest's C code.

Then set the cfg through `RUSTFLAGS`:

```sh
rustup component add rust-src
RUSTC_BOOTSTRAP=1 RUSTFLAGS="--cfg trogon_hyperlight_wasm" \
  cargo build -p trogon-sandbox-hyperlight-worker
```

`RUSTFLAGS` replaces any `rustflags` from Cargo configuration and changes the fingerprint of every
crate, so the first build with the cfg rebuilds the dependency tree. The cfg is declared in the crate's
`check-cfg` list, so builds without it stay free of `unexpected_cfgs` findings.

## Configuration

| Flag | Environment | Default |
| --- | --- | --- |
| `--component-root` | `SANDBOX_HYPERLIGHT_COMPONENT_ROOT` | required |
| `--nats-url` | `SANDBOX_NATS_URL` | `nats://127.0.0.1:4222` |
| `--max-concurrent-leases` | `SANDBOX_MAX_CONCURRENT_LEASES` | `8` |
| `--max-warm-sandboxes` | `SANDBOX_MAX_WARM_SANDBOXES` | `4` |
| `--snapshot-registry` | `SANDBOX_SNAPSHOT_REGISTRY` | `127.0.0.1:5000` |
| `--insecure-registries` | `SANDBOX_OCI_INSECURE_REGISTRIES` | none |
| `--dispatch-fence-file` | `SANDBOX_DISPATCH_FENCE_FILE` | in memory |
| `--guest-input-buffer-bytes` | `SANDBOX_HYPERLIGHT_GUEST_INPUT_BUFFER_BYTES` | `262144` (256 KiB) |
| `--guest-output-buffer-bytes` | `SANDBOX_HYPERLIGHT_GUEST_OUTPUT_BUFFER_BYTES` | `262144` (256 KiB) |
| `--guest-heap-bytes` | `SANDBOX_HYPERLIGHT_GUEST_HEAP_BYTES` | `1310720` (1.25 MiB) |

The image of a lease is a logical name that resolves to a `.wasm` file under the component root. Names
that are absolute or contain `..` are refused.

The output buffer bounds what a guest can hand back in one return value or host-call argument;
Hyperlight's own default is 16 KiB. A guest's Wasm linear memory, stack included, comes out of its
heap, so a Rust guest built with the default 1 MiB stack barely fits 1.25 MiB. Link guests with a
small stack and initial memory, as `tests/fixtures/passthrough-guest/build.rs` does
(`-zstack-size=65536`, `--initial-memory=131072`), or raise `--guest-heap-bytes`.

## What a lease may ask for

A guest is a sealed Wasm component, so a lease is refused when it asks for `spec.env`, a clone, host
mount or S3 workspace, privileged access, or a provider-native snapshot. A snapshot image must be a
`HyperlightSnapshotV1` artifact, and the broker resolves snapshot tags before dispatch.

## Running against a hypervisor

`tests/wasm_backend.rs` holds the tests that need a real guest. They are ignored by default. Build the
`echo_double` guest in `tests/fixtures/passthrough-guest`, compile it ahead of time as its README
describes, and point `HYPERLIGHT_TEST_MODULE` at the result:

```sh
HYPERLIGHT_TEST_MODULE=/path/to/passthrough_guest.aot \
  RUSTC_BOOTSTRAP=1 RUSTFLAGS="--cfg trogon_hyperlight_wasm" \
  cargo test -p trogon-sandbox-hyperlight-worker --test wasm_backend -- --ignored
```

Without the cfg the file compiles to nothing, so the plain test run skips it. With the cfg,
`tests/binary.rs` also ignores the check that the binary exits when the host has no hypervisor.

Two examples drive the same paths end to end against a local OCI registry:

- `e2e_snapshot_roundtrip` loads a module, snapshots it, pushes and pulls the snapshot, and checks the
  restored guest answers the same.
- `e2e_provider_roundtrip` leases a module through the provider, snapshots the lease, and leases again
  from that snapshot.

Both need the same `RUSTC_BOOTSTRAP` and `RUSTFLAGS` as the tests. Without the cfg they build and
exit with an error saying so.
