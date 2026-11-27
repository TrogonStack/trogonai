# Run the Modal worker

`trogon-sandbox-modal-worker` subscribes to `sandbox.provider.modal.lease` and
the cancel wildcard `sandbox.provider.modal.cancel.>`. It provisions each lease
as a Modal Sandbox.

Broker clients request this provider with `modal` in `provider_pref`.
`modal-com` and `modal-labs` are accepted as aliases.

## Why the worker needs Python

Modal's control plane is gRPC over Protobuf, and Modal ships its Sandbox client
as an official Python SDK. The worker drives that SDK through a one-shot JSON
bridge, `src/modal_bridge.py`, which is compiled into the binary and unpacked
into a private temporary directory at startup. Every control-plane call spawns
the interpreter, writes one JSON request on stdin and reads one JSON response on
stdout.

The provider depends only on the `ModalSandboxApi` trait, so the bridge can be
replaced by another transport without changing the provider.

The host that runs the worker therefore needs a Python interpreter with the
official `modal` package importable.

## Configure the worker

This worker reads its settings from variables with the `TROGON_SANDBOX_`
prefix.

| Variable / flag | Purpose |
|-----------------|---------|
| `TROGON_SANDBOX_NATS_URL` / `--nats-url` | NATS connection URL. Default `nats://127.0.0.1:4222`. |
| `TROGON_SANDBOX_DISPATCH_FENCE_FILE` / `--dispatch-fence-file` | Journal that keeps the lease fence across restarts. Unset keeps it in memory. |
| `MODAL_TOKEN_ID`, `MODAL_TOKEN_SECRET` | Modal workspace credentials. The Modal SDK reads them inside the bridge child process. The worker does not log or forward them. |
| `TROGON_SANDBOX_MODAL_APP_NAME` / `--app-name` | Modal App every sandbox is created under. Created on first use. Default `trogon-sandboxes`. |
| `TROGON_SANDBOX_MODAL_ENVIRONMENT` / `--environment` | Modal environment name. Unset uses the workspace default. |
| `TROGON_SANDBOX_MODAL_CPU_CORES` / `--cpu-cores` | Cores a sandbox gets when its lease names none. |
| `TROGON_SANDBOX_MODAL_MAX_VCPUS` / `--max-vcpus` | Most vCPUs a lease may ask for. Default 8. |
| `TROGON_SANDBOX_MODAL_REGIONS` / `--regions` | Regions to advertise, comma separated. |
| `TROGON_SANDBOX_MAX_CONCURRENT_LEASES` / `--max-concurrent-leases` | Capacity announced in the heartbeat. Default 4. |
| `TROGON_SANDBOX_MODAL_PYTHON` / `--python` | Interpreter that runs the bridge. It must have `modal` importable. Default `python3`. |
| `TROGON_SANDBOX_MODAL_VOLUME_CREDENTIAL_ID` / `--volume-credential-id` | Pull-secret id under which the credential broker issues S3 volume credentials. Default `modal-volume`. |
| `TROGON_SANDBOX_MODAL_VOLUME_REGION` / `--volume-region` | Region for the S3 volume backend. Default `us-east-1`. |
| `TROGON_SANDBOX_MODAL_VOLUME_ENDPOINT` / `--volume-endpoint` | Custom S3 endpoint, for example a MinIO server. |

Observability flags are shared by all workers. See
[Observe the sandbox stack](./observability.md).

## How a lease maps to Modal

| Wire field | Modal |
|------------|-------|
| `spec.image` | `Image.from_registry(...)` at create. A registry image or a Modal native snapshot reference is accepted. |
| `ttl_seconds` | `timeout`, Modal's maximum sandbox lifetime. The profile advertises 86400 seconds as the lifetime limit. |
| `spec.memory_mib` | `memory` |
| `spec.vcpu_millis` | CPU request, from 125 millis up to `--max-vcpus` |
| `spec.env` | `env`, plus `TROGON_SANDBOX_MODAL_IMAGE`, `TROGON_SANDBOX_MODAL_IMAGE_TAG` and `TROGON_SANDBOX_MODAL_IMAGE_DIGEST` describing the resolved reference |
| `lease_id`, `tenant`, `name` | Sandbox tags `trogon.lease_id`, `trogon.tenant` and `trogon.name` |
| `spec.network_policy` | `block_network` and `outbound_cidr_allowlist` |

The workspace is staged at `/mnt/workspace` inside the sandbox.

## Workspace sources

| Workspace source | Behavior |
|------------------|----------|
| Empty | An empty scratch directory is uploaded as is. |
| Clone | `git clone --depth=1` on the worker host, then `git checkout <sha>`, then the tree is uploaded. |
| S3 volume | Pulled to worker-local scratch, uploaded, and changed files pushed back when the lease is cancelled. Read-write only. |
| Host mount | Rejected. The sandbox runs in Modal's cloud and cannot see the worker's filesystem. |

Remote mounts are rejected for the same reason. A mount is materialized on the
worker host and cannot reach a sandbox that runs in Modal's cloud.

## Snapshots

The profile declares native snapshots only. A live checkpoint that accepts
native receipts snapshots the sandbox filesystem into a Modal image and returns
the image id as a native snapshot reference. A lease whose image is that
reference creates a sandbox from the image. The image holds the filesystem only.

OCI snapshot images, snapshot tags and Hyperlight modules are refused with
`SnapshotUnsupported`.

## Limits

- No memory checkpoint. A snapshot captures the filesystem only.
- No pause and resume, no warm pool, and no change of ports after create.
- No file operations, terminals, interactive stdin or named processes.
- The worker advertises user-space kernel isolation and best-effort egress
  enforcement. The cold start latency in the capability profile is a static
  figure, so measure a real deployment before depending on it.

## Test the worker

```sh
cargo test -p trogon-sandbox-modal-worker
```

The tests run against an in-process `ModalSandboxApi` mock and need no Modal
credentials.

## Related

- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md)
- [Run the Daytona worker](./daytona-worker.md)
