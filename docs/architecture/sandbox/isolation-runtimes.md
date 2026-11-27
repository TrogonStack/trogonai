# Isolation tiers and runtimes

This page explains how the sandbox stack isolates untrusted workloads, what gVisor (runsc), Kata Containers and Firecracker are, and how they map to the code. It is understanding-oriented. For operational steps see the how-to pages in `docs/how-to/sandbox/`.

## The core idea: tiers, not runtimes

Workloads do not ask for a specific technology. They declare an `IsolationTier` (defined in `rsworkspace/crates/sandbox/trogon-sandbox-contract/src/provider_kind.rs`), which describes how strongly the workload must be separated from the host.

| Tier | Meaning | Backing providers |
|---|---|---|
| `Container` | Plain container, no extra isolation. Trusted internal work only. | subprocess, local Docker, Daytona |
| `Bwrap` | Subprocess confined by bubblewrap namespaces. | subprocess |
| `UserSpaceKernel` | Syscalls intercepted by a user-space kernel. | gVisor (runsc) through the Kubernetes worker, Agent Substrate (`gvisor` class), Modal |
| `MicroVm` | Lightweight VM per workload. | Kata Containers with Firecracker (`kata-fc`), Agent Substrate (`kata-fc` class), Azure Container Apps, the Firecracker worker, Fly, Proxmox, E2B, Vercel, Space |
| `FullVm` | Full, long-lived VM per workload. | KubeVirt, Proxmox |
| `WasmMicroVm` | WASM guest in a microVM with a very short cold start. | Hyperlight, V8 WASM |

The broker uses the tier to pick an eligible provider (`IsolationTier::providers_supporting`), and each provider translates the tier into its own knob. For Kubernetes that knob is the `RuntimeClass`. See [Subprocess isolation](./subprocess-isolation.md) for the `Bwrap` tier.

## gVisor and runsc are the same thing

These two names cause confusion, but they are one project.

- gVisor is the project: a user-space application kernel. The sandboxed process makes syscalls as usual, but gVisor intercepts them and services them itself. The workload never talks to the real host kernel directly, which shrinks the kernel attack surface.
- runsc ("run sandboxed container") is gVisor's OCI runtime binary, the executable the container runtime invokes to start a container inside gVisor.

In Kubernetes terms, the `RuntimeClass` is named `gvisor` for humans and its `handler` is `runsc`, which tells the container runtime which shim to run. Supporting runsc is supporting gVisor. They are one row in the support matrix, not two.

gVisor's trade-off is that it needs no hardware virtualization (it works without nested virtualization), but syscall-heavy workloads run slower because every syscall is mediated.

## Kata Containers and Firecracker

- Kata Containers is a container runtime that runs each pod inside its own lightweight VM. To Kubernetes it looks like a normal container. Under the hood there is a hardware-virtualized boundary.
- Firecracker is a minimal virtual machine monitor built for short-lived microVMs. It is not a container runtime by itself.
- `kata-fc` is Kata Containers configured to use Firecracker as its VMM. On the Kubernetes path, Kata manages the Firecracker process.

The separate `trogon-sandbox-firecracker-worker` launches Firecracker and its jailer directly on a worker host. See [the Firecracker worker how-to](../../how-to/sandbox/firecracker-worker.md) for that deployment path.

Kata can also use other hypervisors, which is why clusters can expose different handlers under different `RuntimeClass` names. The sandbox stack only needs the class name.

## Agent Substrate sits in two tiers at once

Agent Substrate is a control plane rather than a runtime. Its actors carry a sandbox class, and the two classes it offers are the same two technologies as above: `gvisor` and `kata-fc`. So the substrate provider appears in both the `UserSpaceKernel` and `MicroVm` rows. A lease that declares a tier gets the matching class. A lease that declares none falls back to the worker's `--sandbox-class` default (environment variable `SANDBOX_SUBSTRATE_SANDBOX_CLASS`, default `gvisor`).

What it buys over running those runtimes directly is suspend and resume: an actor's memory is snapshotted on suspend and restored on demand, so a named sandbox comes back with its state instead of booting. What it costs is granularity, because CPU and memory belong to the worker pool an actor lands on rather than to the actor. See [Substrate provider](./substrate-provider.md).

## RuntimeClass mapping

The Kubernetes worker (`rsworkspace/crates/sandbox/trogon-sandbox-k8s-worker`) maps tiers to RuntimeClass names through flags and environment variables.

| Environment variable | Flag | Effect |
|---|---|---|
| `SANDBOX_POD_RUNTIME_CLASS` | `--runtime-class` | Default RuntimeClass. Defaults to `kata-fc`. |
| `SANDBOX_POD_RUNTIME_CLASS_MICRO_VM` | `--runtime-class-micro-vm` | RuntimeClass for the `MicroVm` tier. Defaults to the value of `--runtime-class` when unset. |
| `SANDBOX_POD_RUNTIME_CLASS_USER_SPACE_KERNEL` | `--runtime-class-user-space-kernel` | RuntimeClass for the `UserSpaceKernel` tier, typically `gvisor`. |

The cluster must define each named `RuntimeClass` with a handler that the node's container runtime provides. The sandbox stack does not install runtime handlers.

## Choosing a tier

- Trusted internal tooling: `Container`.
- Untrusted code on a single host without a VM layer: `Bwrap`.
- Untrusted code, no nested virtualization available, or the syscall surface is the main concern: `UserSpaceKernel` (gVisor).
- Untrusted code with hardware-virtualized isolation, short-lived: `MicroVm` (Kata with Firecracker).
- Needs a real, stateful machine (own kernel modules, long-lived): `FullVm` (KubeVirt or Proxmox).
- Guest is a WASM component and latency matters: `WasmMicroVm` (Hyperlight).
- A long-lived sandbox that should resume with its state rather than boot, at either of the first two virtualized tiers: Agent Substrate, provided the workload needs no SSH, no exported snapshot and no more than one exposed port.

Kubernetes egress policy is configured separately from the RuntimeClass. The shared provider builds per-lease policies in `rsworkspace/crates/sandbox/trogon-sandbox-providers/src/k8s_network_policy.rs`. Deployment-level policies live in `devops/helm/sandbox/templates/networkpolicies.yaml`. Those policies require a CNI that enforces NetworkPolicy. Selecting an isolation tier alone does not establish egress enforcement.

## Guest network anti-spoofing and residual risk

A microVM that gets a raw network path can forge the source MAC, the source IP or an 802.1Q tag. Which layer stops that depends on the runtime.

| Runtime | Owner of the check | What the worker does |
|---|---|---|
| Firecracker (host links and the `fc-netns` helper) | Per-lease network namespace | Pins `ip saddr` to the guest address in the accept rule, installs a `netdev` ingress chain on the TAP (priority -500) that drops VLAN-tagged frames and frames whose source MAC is not the guest's, counts drops in a `spoofed` counter, and sets `rp_filter=1` on the TAP. |
| Proxmox | Proxmox VM firewall (`macfilter`, `ipfilter`, `firewall=1` on `net0`) | Sets them at lease time, then reads back the VM firewall options, the `ipfilter-net0` set and `net0`. `--antispoof require` (the default, environment variable `TROGON_SANDBOX_PROXMOX_ANTISPOOF`) fails the lease if any is missing. `warn` logs and boots. |
| KubeVirt | The cluster's CNI and NetworkPolicy | The worker cannot enforce L2 rules. It checks the VM object the API server stored: every interface must be `masquerade` on the pod network. A VM that fails the check is refused. |

Residual risk:

- Firecracker: the drop counter lives in the lease's network namespace. Each lease adds its final count to the process-wide metric `sandbox_firecracker_spoofed_frames_total` just before its namespace is deleted, so the metric lags the drops until the lease ends. ARP is only checked through the source MAC. Traffic with the correct MAC and IP is not spoofed and is governed by the egress proxy policy. The `netdev` hook belongs to the TAP device, so it is installed again whenever the TAP is recreated for the jailer.
- Proxmox: the guarantee is only as good as `pve-firewall`. The worker verifies the configuration, not that the firewall service has applied it to the running tap.
- KubeVirt: the masquerade check shows the guest is NATed behind the pod address, but whether the pod itself may reach other pods is decided by NetworkPolicy and the CNI, which the worker does not inspect. A CNI without NetworkPolicy support leaves guests of different leases able to reach each other.
- The `fc-netns` helper (`rsworkspace/crates/sandbox/trogon-sandbox-firecracker-worker/scripts/fc-netns`) must be reinstalled to pick up rule changes. An already installed older copy keeps the previous behavior.

## Limitations

Proxmox leases run one linked clone of a template VM through the QEMU guest agent. The worker has these limits:

- Leases must be ephemeral, not prewarmed and have a TTL. Persistent leases and prewarming are refused.
- Only empty workspaces are supported. Mounts, drives, exposed ports and secret bindings are refused.
- The profile advertises no snapshot, restore or volume support, so use a worker with those capabilities when a lease needs them.
- Without a configured proxy network the clone has no network interface and only `DenyAll` egress is served. With one, domain allowlists are enforced by the egress proxy, and TLS interception is not offered.
- Guest anti-spoofing is requested and read back, but what the Proxmox firewall applies to the running tap is outside the worker, so egress enforcement is reported as best effort.
- Interactive terminals, streamed output and sandbox users are not offered. Use `exec` for commands.
- On start and on shutdown the worker destroys every clone it owns, so leases do not survive a worker restart.
