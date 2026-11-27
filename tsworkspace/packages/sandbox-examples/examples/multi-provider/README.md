# Multi-provider

Run the same code against any backend. `SANDBOX_PROVIDER` picks it: write and read a file, run a command, stream output, and pause then resume, with each optional step taken only when the sandbox reports the feature.

## Run

Keyless, in process, no gateway:

```bash
node examples/multi-provider/index.ts
```

Against a real provider, with the connection variables from the [README](../../README.md#setup) exported:

```bash
SANDBOX_PROVIDER=local_docker node examples/multi-provider/index.ts
```

## Backends

| `SANDBOX_PROVIDER`     | Backend                                                                                                                                                                                                                                                                        |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `memory` (default)     | `MemorySandboxClient`, a fake with a virtual filesystem. Needs no gateway or key.                                                                                                                                                                                              |
| any wire provider kind | `SandboxWsClient` against the gateway, with the kind as `providerPref`. The kinds are `k8s_micro_vm`, `subprocess`, `local_docker`, `daytona`, `modal`, `e2b`, `vercel`, `space`, `hyperlight`, `v8_wasm`, `kube_virt`, `azure_container_apps`, `substrate` and `firecracker`. |

## Notes

- Pause and resume run only when `sandbox.supports('pause_resume')` is true. The memory backend reports it.
- Streaming runs only when `sandbox.supports('output_streaming')` is true. The memory backend does not, so that step is skipped there.
- A provider that cannot run commands, such as `daytona` today, fails at the command step. The gateway decides what a kind can do, not this example.
- The sandbox is deleted and the client closed even when a step fails.
