# trogon-sandbox-firecracker-worker

The Firecracker microVM provider for the sandbox stack. A NATS-subscribed worker that fulfills lease requests by
launching one microVM per lease directly on the worker host.

## How it works

- The VMM is controlled through its REST API on a Unix socket (`api`), optionally under the jailer (`jailer`).
- Every command reaches the in-guest agent over HTTP/1.1 through Firecracker's host-initiated vsock (`vsock`,
  `agent`), so a guest needs no network device. One is added only for leases whose network policy allows egress, and
  `network` keeps everything it carries behind the lease's egress proxy (`trogon-sandbox-egress-proxy`).
- `adapter::FirecrackerProvider` implements the protocol `Provider` trait: lease allocation, command execution,
  terminals, snapshots and restore (`snapshot`, `retained`), and release.
- `rootfs` builds root filesystems from OCI images (`trogon-sandbox-oci`), verifying signatures with
  `trogon-sandbox-cosign`; `warm_pool` keeps prepared VMs ready.
- `worker` is the NATS loop: it verifies dispatch envelopes, enforces worker authority and the attempt ledger, and
  publishes heartbeats, lifecycle events and command exit markers.

## Platform support

Launching VMs needs Linux with KVM. Linux-only code is gated with `cfg(target_os = "linux")`, so the crate builds
and its unit tests run on other platforms; the end to end examples and the VM launch paths only run on a KVM host.

## Layout

- `src/main.rs`: the worker binary.
- `scripts/`: guest kernel and rootfs build scripts and the root network helper `fc-netns`.
- `deploy/`: a systemd unit and an example environment file.
- `examples/`: end to end checks run against a real KVM host.

## Related crates

`trogon-sandbox-contract` and `trogon-sandbox-protocol` hold the wire types and the `Provider` trait,
`trogon-sandbox-actor-agent` the guest protocol, `trogon-sandbox-credential-broker` the credential fences, and
`trogon-sandbox-broker` the control plane that dispatches to this worker.
