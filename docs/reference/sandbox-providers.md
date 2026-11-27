# Sandbox providers

A sandbox lease requests a place to run work. The provider determines what runs
there, which operations the lease can use, and what survives a pause. The
entries below describe worker implementations in the public repository.
Availability in a deployment depends on which workers the operator connects
and which capabilities they advertise. [Sandbox product overview](../product/sandbox.md)

## Choose by execution environment

| Provider | What runs for a lease | What to check before choosing it |
| --- | --- | --- |
| <span id="azure_container_apps">Azure Container Apps</span> | A Dynamic Sessions instance in an operator-configured pool. | The worker allocates the session and stages an empty, Git or object-store workspace. A request with environment variables or a snapshot image is refused. [Worker documentation][aca] |
| <span id="daytona">Daytona</span> | A Daytona sandbox started from a named snapshot. | Stopping the guest preserves files and ends processes. The worker can expose preview ports and run detached commands. Checkpoint and snapshot export are unsupported. [Daytona worker guide](../how-to/sandbox/daytona-worker.md) |
| <span id="e2b">E2B</span> | An E2B sandbox created from a template. | The worker handles commands, files and terminals. Its native snapshots restore through E2B templates. [Worker documentation][e2b] · [Capability profile][e2b-profile] |
| <span id="fly">Fly.io</span> | A Fly Machine. | Native volume snapshots require a configured workspace volume. A restore uses the snapshot's region and image. [Worker documentation][fly] |
| <span id="modal">Modal</span> | A Modal Sandbox created through the Modal Python SDK. | The worker host needs Python, the Modal SDK and access to the Modal account. The bridge implements commands, file uploads, tunnels and filesystem snapshots. [Worker implementation][modal] |
| <span id="space">SPACE</span> | A Firecracker microVM reached through the SPACE API. | Commands stream through a separate data-plane endpoint. The worker accepts the default egress policy and refuses port exposure and volumes. [Worker documentation][space] |
| <span id="vercel">Vercel</span> | A Vercel Sandbox microVM for a supported runtime or a Vercel-native snapshot. | Team plan limits apply to CPU and lease duration. Persistence across pause and resume is an explicit option. Snapshots stay with Vercel. [Worker documentation][vercel] |
| <span id="firecracker">Direct Firecracker</span> | A microVM launched on the operator's Linux host. | Requires KVM, a guest kernel and a root filesystem with the guest agent. Commands reach the guest over vsock, including when the VM has no network device. [Firecracker worker guide](../how-to/sandbox/firecracker-worker.md) |
| <span id="hyperlight">Hyperlight</span> | A Wasm component in a hypervisor-isolated guest. | The worker's Wasm backend requires a build with `trogon_hyperlight_wasm` and a host with KVM or MSHV on Linux, or WHP on Windows. Compatible snapshots use the Hyperlight OCI artifact type. [Build the worker][hyperlight] |
| <span id="k8s_micro_vm">Kubernetes</span> | A Job and pod for each lease. | The cluster's configured RuntimeClasses determine the isolation tiers the worker advertises. It supports terminals and in-place rollback. [Worker documentation][k8s] · [Capability profile][k8s-profile] |
| <span id="kube_virt">KubeVirt</span> | A full virtual machine on a KubeVirt cluster. | The worker advertises disk checkpoints and restore. Its profile declares no egress enforcement, which matters for workloads that require outbound restrictions. [Capability profile][kubevirt] |
| <span id="local_docker">Local Docker</span> | A container on the worker's local Docker daemon. | Pause and resume keep the container in place. Live checkpoints use CRIU on a compatible host and restore against the original parent image. [Worker documentation][docker] · [Capability profile][docker-profile] |
| <span id="proxmox">Proxmox</span> | An ephemeral linked clone of a Proxmox VE template VM. | Commands and file transfers use the QEMU guest agent. The default clone has no network interface. The worker's lease profile excludes snapshots and in-place rollback. [Worker documentation][proxmox] · [Capability profile][proxmox-profile] |
| <span id="subprocess">Subprocess</span> | A process on the worker host, isolated by bubblewrap on Linux, Seatbelt on macOS, or an AppContainer or network-fenced account on Windows. | Capabilities depend on the platform and backend. The Linux and macOS workers advertise terminals and structured file operations. [Worker documentation][subprocess] · [Capability profile][subprocess-profile] |
| <span id="substrate">Agent Substrate</span> | An actor created from a digest-pinned image. An agent inside the actor serves commands, files and terminals. | A named or persistent actor can be suspended and reused. The worker accepts at most one exposed port. [Worker documentation][substrate] |
| <span id="v8_wasm">V8 / Wasmtime</span> | JavaScript in a V8 isolate or a WebAssembly component in Wasmtime, inside the worker process. | Choose this for a guest program. Persistent JavaScript evaluation is optional. Workloads that need an operating-system shell or hardware virtualization require a different environment. [Worker documentation][v8-wasm] |

## Test worker

| Provider | Public implementation |
| --- | --- |
| <span id="mock">Memory</span> | A NATS worker for integration tests. Files live in a map, commands return scripted results and terminal input is echoed. It starts no guest process. [Worker documentation][memory] |

## Check a deployment

Use the [provider option matrix](./sandbox/provider-option-matrix.md) to compare
declared support and inspect the profile a connected worker advertises.
Include required capabilities in the lease request when the workload depends
on a terminal, checkpointing a running guest, or restricting outbound traffic
to an allowlist. The broker
checks the worker's heartbeat against the request. In TypeScript,
`sandbox.supports('fork')` checks whether the lease advertises fork support.
It returns `false` when the broker omits the capability list. [Worker selection][selection] · [TypeScript client][sdk]

To verify a worker, the provider conformance suite sends requests directly to
that worker over NATS. Supply a running worker and select the checks for its
execution model and advertised features. A suite run against the memory worker
exercises the protocol. Isolation and cleanup for a native workload require
running the checks against the intended backend. [Provider conformance suite][conformance]

[aca]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-aca-worker
[conformance]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-conformance
[e2b]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-e2b-worker
[e2b-profile]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-e2b-worker/src/profile.rs
[fly]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-fly-worker
[hyperlight]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-hyperlight-worker
[k8s]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-k8s-worker
[k8s-profile]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-k8s-worker/src/profile.rs
[kubevirt]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-kubevirt-worker/src/profile.rs
[memory]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-memory-worker
[modal]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-modal-worker/src
[docker]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-docker-worker
[docker-profile]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-docker-worker/src/profile.rs
[proxmox]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-proxmox-worker
[proxmox-profile]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-proxmox-worker/src/profile.rs
[sdk]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-ws-client
[selection]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-broker/src/worker_registry/heartbeat_selection.rs
[space]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-space-worker
[substrate]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-substrate-worker
[subprocess]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-subprocess-worker
[subprocess-profile]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-subprocess-worker/src/profile.rs
[v8-wasm]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-v8-wasm-worker
[vercel]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-vercel-worker
