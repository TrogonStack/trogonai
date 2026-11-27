# Validate the sandbox stack on a Linux KVM host

Some sandbox workers need a real Linux kernel or `/dev/kvm`, so a macOS machine or a guest without nested virtualization cannot validate them end to end. This guide sets up any Linux host with `/dev/kvm` as a validation target and runs the Firecracker worker checks on it. Replace the placeholders `<host>`, `<user>` and `<repo>` with your own values.

## Prerequisites

- A Linux host you can reach over SSH, with `/dev/kvm` readable by your user.
- The Rust toolchain pinned in `rsworkspace/rust-toolchain.toml`. Install `rustup` and let it fetch that version on first build.
- A `firecracker` binary on the host, and for the jailed path the matching `jailer`.
- A checkout of the repository on the host.

Confirm the host exposes KVM:

```bash
ssh <user>@<host> 'ls -l /dev/kvm && uname -r && lsmod | grep kvm'
```

If `/dev/kvm` is missing, the host is a guest without nested virtualization. Enable nested virtualization on its hypervisor or use bare metal.

## Run the unit tests on Linux

Code paths gated on `target_os = "linux"` are skipped on a macOS machine. Run them on the host:

```bash
ssh <user>@<host> 'cd <repo>/rsworkspace \
  && cargo test -p trogon-sandbox-firecracker-worker \
  && cargo test -p trogon-sandbox-hyperlight-worker'
```

These suites use fake Firecracker sockets and a fake hypervisor, so they do not need KVM. The shipped Hyperlight worker binary uses an unsupported-hypervisor backend and exits at startup on a host without a hypervisor, so a real guest run needs a deployment that supplies one. See [Run the Firecracker worker](./firecracker-worker.md) for the Firecracker setup.

## Run the Firecracker examples

The examples under `rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/examples` exercise a real host. `e2e_lease_exec` and `e2e_network` need `/dev/kvm`, a kernel and a root filesystem with the guest agent installed. `e2e_network_host` exercises the worker's Linux network setup without KVM.

1. Build a musl-linked guest agent and a root filesystem that starts it at boot:

   ```bash
   cargo build --release -p trogon-sandbox-actor-agent --target x86_64-unknown-linux-musl
   rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/scripts/build-guest-rootfs.sh \
     --agent-bin <path-to-agent> --out <work-dir>/rootfs.ext4 \
     --size 2G --kernel-out <work-dir>/vmlinux
   ```

   The script needs sudo or root. For an arm64 host build the agent for `aarch64-unknown-linux-musl` and pass `--arch aarch64`.

2. Lease a microVM, run `uname -a` through the command path and cancel:

   ```bash
   E2E_KERNEL=<work-dir>/vmlinux \
   E2E_ROOTFS=<work-dir>/rootfs.ext4 \
   E2E_STATE_DIR=<work-dir>/state \
     cargo run -p trogon-sandbox-firecracker-worker --example e2e_lease_exec
   ```

   `E2E_FIRECRACKER_BINARY` (default `firecracker`) and `E2E_IMAGE` (default `guest:e2e`) are optional.

For the full worker with a broker and NATS, follow the steps in [Run the Firecracker worker](./firecracker-worker.md). The worker's state directory is `SANDBOX_FIRECRACKER_STATE_DIR` and its checkpoint directory is `SANDBOX_FIRECRACKER_SNAPSHOT_DIR`, which must be outside the state directory.

## Sync changes from a development machine

Do not let `rsync` copy modification times, which `-a` does. A file edited on the development machine before the host's last build arrives older than that build, so cargo keeps the stale binary and the run tests old code. Compare by content and let changed files take the host's clock:

```bash
rsync -rlpDz --checksum --delete \
  --exclude target --exclude .git --exclude node_modules \
  rsworkspace/ <user>@<host>:<repo>/rsworkspace/
```

If a checkout was already synced with `-a`, run `find crates -name '*.rs' -exec touch {} +` there once before building.

## Share the host with another run

Two stacks can run side by side when each has its own compose project, NATS ports and Firecracker state and snapshot directories.

Disk is the scarce resource. Every checkout keeps its own `target` directory, each live lease holds its own root filesystem copy, and each snapshot holds its memory and root filesystem. Check `df -h /` before a run and remove stale build directories you own.
