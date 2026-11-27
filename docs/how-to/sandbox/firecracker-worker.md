# Run the Firecracker worker

`trogon-sandbox-firecracker-worker` subscribes to `sandbox.provider.firecracker.lease` and the per-lease verb subjects under `sandbox.provider.firecracker.>`. It fulfills each lease with one Firecracker microVM launched directly on the worker host. The worker is Linux only.

Broker clients request this provider with `firecracker` or `fc` in `provider_pref`.

## How a command reaches the guest

By default the guest boots with no network device. Guest control always travels over Firecracker's vsock:

1. The worker connects to the VM's vsock Unix socket, `<state_dir>/<lease_id>/v.sock`.
2. It sends `CONNECT <port>` and reads `OK <n>`.
3. From there the stream is raw bytes to the guest listener, and the worker speaks HTTP to `trogon-sandbox-actor-agent`.

The worker dials a fresh vsock stream per call. A worker that does not hold a lease stays silent, so the worker that does hold it answers.

## Prerequisites

- A Linux host with `/dev/kvm` readable by the worker's user.
- A `firecracker` binary and, for production, the matching `jailer` binary on the host.
- An uncompressed guest kernel (`vmlinux`).
- An ext4 root filesystem with `trogon-sandbox-actor-agent` installed and started at boot, listening on the vsock port (`SANDBOX_ACTOR_AGENT_VSOCK_PORT`, default `8110`).
- NATS reachable from the worker.

The guest agent learns its identity from `ate.actor_id=<lease id>` on the kernel command line. The worker appends it to the boot arguments for every lease, so the root filesystem does not need to be customised per VM.

The crate ships helper scripts in `rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/scripts`: `build-guest-rootfs.sh` builds a root filesystem that starts the agent at boot, `build-guest-kernel.sh` builds a guest kernel with container networking support, and `build-oci-rootfs.sh` converts an OCI image. Run `build-guest-rootfs.sh --agent-bin <path> --out <path>` on a Linux host with a statically linked musl build of the agent.

## Start the worker

For local development, `--insecure-dev` allows starting without the jailer. The VMM then runs as the worker user with no chroot, cgroup or seccomp isolation, and the worker logs a warning.

```sh
trogon-sandbox-firecracker-worker \
  --insecure-dev \
  --kernel /srv/fc/vmlinux \
  --rootfs /srv/fc/rootfs.ext4 \
  --image guest:base \
  --state-dir /var/lib/sandbox-fc
```

## Configuration

Every option is a flag and an environment variable.

| Variable | Purpose |
|----------|---------|
| `SANDBOX_NATS_URL` | NATS connection URL. |
| `SANDBOX_FIRECRACKER_BINARY` | The `firecracker` executable. Default `firecracker`. |
| `SANDBOX_FIRECRACKER_JAILER` | Jailer executable. Required: the worker refuses to start without it unless `SANDBOX_INSECURE_DEV` or `--insecure-dev` is set. |
| `SANDBOX_INSECURE_DEV` | Development only. Allows starting without the jailer. |
| `SANDBOX_FIRECRACKER_JAILER_UID_BASE` and `SANDBOX_FIRECRACKER_JAILER_GID_BASE` | Start of the reserved host UID and GID ranges. Default `200000`. Reserve one identity per concurrent lease. |
| `SANDBOX_FIRECRACKER_JAILER_CGROUP_PARENT` | Parent within the unified cgroup hierarchy. Default `sandbox-fc`. |
| `SANDBOX_FIRECRACKER_JAILER_MEMORY_OVERHEAD_MIB` | Host memory allowed for the VMM beyond guest RAM. Default `128`. |
| `SANDBOX_FIRECRACKER_KERNEL` | Guest kernel image. Required. |
| `SANDBOX_FIRECRACKER_ROOTFS` | Base ext4 image. Each lease gets a sparse copy, so the state directory needs a filesystem with sparse files such as ext4, xfs or btrfs. Required. |
| `SANDBOX_FIRECRACKER_STATE_DIR` | One subdirectory per lease. Default `/var/lib/sandbox-fc`. Keep the path short, because it holds Unix sockets and the socket path limit is about 100 bytes. |
| `SANDBOX_FIRECRACKER_IMAGE` | The only image a lease may name. Required. |
| `SANDBOX_FIRECRACKER_VCPUS` | vCPUs per VM. Default `2`. |
| `SANDBOX_FIRECRACKER_DEFAULT_MEMORY_MIB` | Memory for a lease whose `spec.memory_mib` is `0`. Default `512`. |
| `SANDBOX_FIRECRACKER_AGENT_VSOCK_PORT` | Guest agent vsock port. Default `8110`. |
| `SANDBOX_FIRECRACKER_BOOT_TIMEOUT` | Budget for the API socket to appear and, separately, for the agent to answer. Default `30` seconds. |
| `SANDBOX_FIRECRACKER_MAX_CONCURRENT_LEASES` | Cap on live VMs. Default `8`. |
| `SANDBOX_FIRECRACKER_SNAPSHOT_DIR` | Optional host-local checkpoint store, outside the state directory. |
| `SANDBOX_FIRECRACKER_MAX_SNAPSHOTS` | Host-local checkpoints retained. Default `4`. |
| `SANDBOX_FIRECRACKER_SNAPSHOT_MAX_ROOTFS_BYTES` | Restored root disk budget. Default 4 GiB. |
| `SANDBOX_FIRECRACKER_WARM_POOL_MAX_SNAPSHOTS` | Cached pristine snapshots. Default `0`, which disables the pool. |
| `SANDBOX_FIRECRACKER_WARM_POOL_MAX_BYTES` | Logical disk budget for cached snapshots. |
| `SANDBOX_FIRECRACKER_WARM_POOL_MAX_IDLE_SECONDS` | Idle cache retention. Default `300`. |
| `SANDBOX_FIRECRACKER_RELEASED_MAX_BYTES` | Reserved logical bytes for release snapshots, including pending captures. |
| `SANDBOX_FIRECRACKER_RELEASED_MAX_RECORDS` | Retained export records and receipts. Default `256`. |
| `SANDBOX_FIRECRACKER_RELEASED_TTL_SECONDS` | Export and receipt retention after capture. Default `600`. |
| `SANDBOX_OCI_INSECURE_REGISTRIES` | Comma-separated registries served over HTTP for snapshot transport. |
| `SANDBOX_REQUIRE_COMMITTED_POLICY` | Refuse a lease that needs credentials but has no committed policy. Needs the credential broker URL. |

The state directory also holds `dispatch.json`, a SQLite database containing the stable worker identity and durable attempt fences. Keep this state private and persistent. Back up the database through SQLite or while the worker is stopped. Deleting it loses the authority needed to fence late dispatches and prove cleanup.

## Launch under the jailer

Install matching statically linked Firecracker and jailer binaries. Reserve a host UID and GID range for this worker that no other users or services use. Run the worker as root on a host with a writable cgroup v2 hierarchy and the memory, CPU and pids controllers available. Keep the state directory and its parents root-owned and inaccessible for modification by other host users.

```sh
sudo trogon-sandbox-firecracker-worker \
  --firecracker-binary /srv/fc/firecracker \
  --jailer /srv/fc/jailer \
  --kernel /srv/fc/vmlinux \
  --rootfs /srv/fc/rootfs.ext4 \
  --image guest:base \
  --state-dir /var/lib/sandbox-fc
```

Each live VMM receives its own host UID and GID, and Firecracker's default seccomp filters stay enabled. The memory cgroup allows guest RAM plus the configured VMM overhead. A launch fails if the jailer cannot establish these controls. Use a separate state directory and cgroup parent for each worker daemon. An exclusive process lock rejects another daemon that uses the same state directory.

The chroot adds path components, so use a short state path if the Unix socket length check refuses a lease.

## Give leases network access

By default a lease has no network device. A fresh lease with a `deny_all` network policy, or with no policy, boots without one. Two backends give a lease a device. Choose one, because they cannot be combined, and the jailer uses the host-links backend only.

### Host links

Run the worker with network-administration privileges and install `iproute2` and `nftables` on the host. Set `--host-links` (`SANDBOX_FIRECRACKER_HOST_LINKS`) to an unused private IPv4 pool, for example `10.231.0.0/24`. Each lease reserves a `/30` host link, and the worker refuses overlapping routes. Reserve a different pool for each worker deployment.

A lease with `allow_all` or a supported domain policy receives a tap inside its own network namespace. Namespace firewall rules drop traffic except TCP to that lease's authenticated egress proxy endpoint. The namespace has no default route. The worker sets proxy environment variables and configures the guest interface through vsock. The worker installs the proxy's public CA in the guest before the lease is ready, and if the install fails the lease fails and no workload runs.

A lease that already has a network device accepts supported live policy updates. A networkless guest needs a new lease to attach a device.

To accept `Transform` rules for credential injection, also set `--credential-broker-nats-url` (`SANDBOX_FIRECRACKER_CREDENTIAL_BROKER_NATS_URL`). The proxy fetches each credential per matching request, so the real value never reaches the guest. Without it a `Transform` policy is refused at admission. To accept request forwarding rules, set `--oidc-signing-key-pem-file` (`SANDBOX_FIRECRACKER_OIDC_SIGNING_KEY_PEM_FILE`), a PKCS#8 PEM EC key. It requires host links. Keep the key readable only by the worker user.

### Root helper

`--netns-helper` (`SANDBOX_FIRECRACKER_NETNS_HELPER`) keeps an existing root-helper deployment working. The worker stays unprivileged, and namespaces, taps and nftables are handled by one root-owned helper script, `rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/scripts/fc-netns`. Install it, let the worker's user run only that file through `sudo` without a password, and install `iproute2`, `nftables` and `setpriv`. The prefix must match the worker's `--netns-prefix` (`SANDBOX_FIRECRACKER_NETNS_PREFIX`, default `sbxfc-`). The proxy executable comes from `--egress-proxy-binary` (`SANDBOX_FIRECRACKER_EGRESS_PROXY_BINARY`); point it at `trogon-sandbox-egress-proxyd`.

Both backends pin the guest source address and drop VLAN-tagged and foreign-source-MAC frames on the tap. The dropped-frame count of each lease is exported as `sandbox_firecracker_spoofed_frames_total`. See [Scrape sandbox metrics and export traces](./observability.md) and [Isolation runtimes](../../architecture/sandbox/isolation-runtimes.md).

## Lease translation

| Wire field | Firecracker |
|------------|-------------|
| `spec.memory_mib` | `mem_size_mib` in the machine config |
| worker configuration | `vcpu_count` |
| `spec.env`, `spec.working_dir` | Sent to the agent with the lease bind. `working_dir` defaults to `/workspace`. |
| `lease_id` | Directory name, and `ate.actor_id=` on the kernel command line |

The worker boots in the order Firecracker requires: boot source, root drive, machine config, vsock, then `InstanceStart`. It then polls the agent's readiness endpoint and binds the lease. A failure at any step stops the VMM and deletes the lease directory.

A lease asking for something this worker cannot provide is refused with an `InvalidSpec` that names the field, for example mounts, drives, exposed ports, user overrides, secret bindings, or a workspace other than an empty one. A network policy other than `deny_all` is refused when host networking is not configured.

## Checkpoints and forks

A worker started with `--snapshot-dir` checkpoints running leases and restores new leases from those checkpoints. A checkpoint pauses the guest, writes its memory and device state, copies its root filesystem sparsely and resumes it. The worker keeps the newest `--max-snapshots` checkpoints and reports what it holds in its heartbeat, so the broker routes restores to it.

Snapshots stay on the host that took them. With several workers for the provider, the broker picks the provider, not the worker, so a restore can land on a worker without the snapshot and is refused there. With the root helper, leases that have a network device are neither checkpointed nor restored.

Use `FirecrackerSnapshotV1` as the artifact type for snapshot export. Restore with a digest-pinned snapshot image, request the same guest memory as the snapshot and use a worker with the same vCPU count and a compatible Firecracker version and CPU model. The guest must use `kvm-clock`. See [Capability profiles](../../architecture/sandbox/capability-profiles.md).

## OCI images

Set `--oci-guest-agent` and `--oci-guest-init` to statically linked Linux executables to enable container-image conversion. They must be given together. Install `umoci`, `mkfs.ext4` and util-linux on the host. The converted base images are cached under `--oci-cache-dir` (default `/var/cache/sandbox-fc-images`), bounded by `--oci-cache-max-bytes`. `--oci-rootfs-size-mib` (default 2048) sets each image's root disk size, and `--oci-max-download-bytes` caps compressed image bytes.

## Clean up

`cancel` and `purge` kill the VMM and remove the lease directory, and are safe to repeat or to send for a lease the worker does not hold. Startup takes the state-directory lock before reclaiming stale leases and fails if any orphan, network, cgroup or directory cleanup fails. On shutdown the worker releases every lease it holds, because a restarted worker does not adopt VMs.

## Run as a host service

The files under `rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/deploy` target a root-owned host with systemd and cgroup v2. Copy `worker.env.example` to `/etc/sandbox-fc/worker.env`, owned by root with mode `0600`, then configure NATS and the image paths. Install `sandbox-firecracker-worker.service` in `/etc/systemd/system`. Review its `CPUQuota`, `MemoryMax`, `TasksMax` and lease concurrency against the host's capacity before enabling it. Keep the configured cgroup parent aligned if you change the unit name or slice.

After installation, check the service journal, the worker heartbeat, a lease with a command, and cgroup removal on the host before you treat the deployment as accepted.

## Limitations

- Snapshots stay on the host that took them. Nothing is pushed to a registry, so a restore on another host fails. Pin restores to the worker that holds the snapshot.
- With the root helper (not host-links), a lease with a network policy other than deny-all cannot request `snapshot_out` or restore from a snapshot, because a restored microVM cannot gain a network device. Use host-links mode for networked snapshots.
- A restore is refused when the captured state lacks the realtime clock, or when the restored guest does not report `kvm-clock` as its clock source. Restore only snapshots taken on a worker whose guest kernel uses `kvm-clock`.
- The cold start time in the worker's capability profile is an estimate of 2000 ms, not a measurement. Measure on your own host before you rely on it for scheduling.
- The Firecracker CI kernel has no `xt_comment`, which container networking in the guest needs. Build a kernel with `build-guest-kernel.sh` when the guest runs a container engine, and pass it to `build-guest-rootfs.sh` with `--kernel-config`.

## Test

```sh
cargo test -p trogon-sandbox-firecracker-worker
```

These tests use a fake Firecracker API socket and fake process launches. The `e2e_lease_exec`, `e2e_network` and `e2e_network_host` examples under the crate's `examples` directory exercise a real host. The `e2e_lease_exec` and `e2e_network` examples need `/dev/kvm`, a kernel and a root filesystem.
