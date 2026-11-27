# trogon-sandbox-capability-matrix

Every sandbox worker declares a capability profile: which snapshot, volume, isolation, lifecycle, resource,
placement, and ingress features it offers. The broker routes leases on those profiles. This crate renders all
of them side by side so the differences between workers are visible in one place, and keeps the tables below
honest: a test fails when they drift from the code.

The tables are generated from each worker's public profile. Serde turns a profile into JSON and every leaf
becomes a dotted row, so a field added to the profile structs shows up here without a change to this crate.

- Workers configured at startup are rendered at the defaults their binaries document.
- `firecracker (networked)` is the firecracker profile with networking turned on.
- Subprocess rows marked `platform-dependent` are decided by the host operating system or by probing the
  host, so they differ between Linux and macOS.
- fly, proxmox, and space are built from their config alone, so no provider, credential, or state directory
  is created. fly has no default region and is rendered in `iad`; that is the only operator value in its
  column.

Regenerate the tables after changing a profile:

```sh
cargo run -p trogon-sandbox-capability-matrix -- --write
```

Run it from `rsworkspace/`. Without `--write` it only checks and exits non-zero when the tables are stale.

## Matrix

<!-- BEGIN GENERATED: capability-matrix -->

### `ingress`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `ingress` | none | none | none | none | - | - | none | - | none | - | none | - | - | - | - | - | - | none |
| `max_exposed_ports` | - | - | - | - | `15` | `0` | - | `1` | - | `15` | - | `15` | `15` | `0` | `0` | `0` | `0` | - |

### `isolation`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `default_tier` | `container` | `user_space_kernel` | `full_vm` | `wasm_micro_vm` | `container` | `micro_vm` | platform-dependent | `user_space_kernel` | `wasm_micro_vm` | `user_space_kernel` | `micro_vm` | `micro_vm` | `micro_vm` | `full_vm` | `micro_vm` | `micro_vm` | `micro_vm` | `container` |
| `egress` | - | - | - | - | - | - | platform-dependent | - | - | - | - | - | - | - | - | - | - | - |
| `egress.cidr_allowlist` | `true` | `true` | `false` | `false` | `true` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `true` | `false` |
| `egress.deny_all` | `true` | `true` | `false` | `true` | `true` | `false` | platform-dependent | `false` | `true` | `true` | `true` | `true` | `false` | `true` | `false` | `true` | `true` | `false` |
| `egress.host_allowlist` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `true` | `false` |
| `egress.proxy_mediated` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` |
| `egress_enforcement` | `best_effort` | `best_effort` | `none` | `best_effort` | `best_effort` | `none` | `mandatory` | `none` | `best_effort` | `best_effort` | `best_effort` | `best_effort` | `none` | `best_effort` | `none` | `mandatory` | `mandatory` | `none` |
| `resource_limits` | `none` | `none` | `none` | `none` | `none` | `none` | platform-dependent | `none` | `runtime` | `none` | `none` | `none` | `none` | `none` | `none` | `none` | `none` | `none` |
| `supported_tiers` | `container` | `user_space_kernel, micro_vm` | `full_vm` | `wasm_micro_vm` | `container` | `micro_vm` | platform-dependent | `user_space_kernel, micro_vm` | `wasm_micro_vm` | `user_space_kernel` | `micro_vm` | `micro_vm` | `micro_vm` | `micro_vm, full_vm` | `micro_vm` | `micro_vm` | `micro_vm` | `container` |
| `supports_browser` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_change_tracking` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_command_cancellation` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_command_launch_cancellation` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_command_restart` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_dir_transfer` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_domain_allowlist` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `true` | `false` |
| `supports_evidence_streaming` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_expose_after_create` | `true` | `false` | `false` | `false` | `true` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_file_archive` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_file_ops` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_file_search` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_file_watch` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_fs_policy` | `false` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_graceful_stop` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_hardening_overrides` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_interactive_stdin` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_masked_files` | `false` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_named_processes` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_network_policy_update` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_output_redaction` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_output_streaming` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_persistent_runtime` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_port_preview` | `true` | `false` | `false` | `false` | `true` | `false` | platform-dependent | `true` | `false` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` |
| `supports_privileged` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `true` | `true` | `false` |
| `supports_process_readiness` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` |
| `supports_sandbox_groups` | `true` | `true` | `false` | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_sandbox_users` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `supports_secret_bindings` | `false` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_secret_substitution` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_terminal` | `true` | `true` | `true` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `true` | `true` | `true` |
| `supports_tls_intercept` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |

### `lifecycle`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `dormancy.freeze` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `true` |
| `dormancy.suspend` | - | - | - | - | `disk` | - | - | - | - | - | `memory` | `disk` | `memory` | - | `memory` | - | - | `memory` |
| `max_lease_lifetime_seconds` | - | - | - | - | - | - | - | - | - | `86400` | `86400` | `86400` | - | - | - | - | - | - |
| `max_warm_pool_size` | `16` | `16` | `4` | `4` | - | - | - | - | - | - | - | - | - | - | - | - | - | - |
| `supports_named_tags` | `true` | `true` | `true` | `true` | `true` | `false` | `true` | `true` | `false` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_native_auto_stop` | `false` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_pause_resume` | `true` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `true` | `false` | `true` | `false` | `false` | `true` |
| `supports_prewarm_per_tenant` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_warm_pool` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |

### `performance`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `cold_restore_p50_ms` | `5000` | `10000` | `30000` | `100` | `5000` | `8000` | `100` | `500` | `0` | `0` | `2000` | `2000` | `0` | `0` | `0` | `0` | `0` | `0` |
| `cold_start_p50_ms` | `500` | `5000` | `5000` | `50` | `2000` | `3000` | `100` | `1500` | `8` | `5000` | `1000` | `1000` | `0` | `0` | `0` | `2000` | `2000` | `0` |
| `max_concurrent_sandboxes` | `4` | `4` | `2` | `8` | - | `4` | `4` | - | - | - | - | - | `4` | `4` | `4` | - | - | - |

### `placement`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `regions` | - | - | - | - | - | - | - | - | - | - | - | - | `iad` | - | - | - | - | - |
| `supports_native_passthrough` | `true` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `true` |

### `provider`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `provider` | `local_docker` | `k8s_micro_vm` | `kube_virt` | `hyperlight` | `daytona` | `azure_container_apps` | `subprocess` | `substrate` | `v8_wasm` | `modal` | `e2b` | `vercel` | `fly` | `proxmox` | `space` | `firecracker` | `firecracker` | `mock` |

### `resources`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `disk_mib` | - | - | - | - | - | - | platform-dependent | - | - | - | - | - | - | - | - | - | - | - |
| `reports_usage_gauges` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `vcpu_millis` | - | - | - | - | - | - | platform-dependent | - | - | - | - | - | - | - | - | - | - | - |
| `vcpu_millis.max` | - | `8000` | `8000` | - | - | - | platform-dependent | - | - | `8000` | - | `8000` | `8000` | `16000` | `32000` | - | - | - |
| `vcpu_millis.min` | - | `100` | `100` | - | - | - | platform-dependent | - | - | `125` | - | `1000` | `1000` | `100` | `1000` | - | - | - |

### `snapshot`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `can_checkpoint_running` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `can_restore` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `can_snapshot` | `true` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `false` |
| `layered_deltas` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supported_artifact_types` | `sandbox_rootfs_v1` | `sandbox_rootfs_v1` | `kubevirt_disk_v1` | `hyperlight_snapshot_v1` | none | none | none | none | none | none | none | none | none | none | none | `firecracker_snapshot_v1` | `firecracker_snapshot_v1` | none |
| `supports_in_place_rollback` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_native_snapshots` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `true` | `true` | `true` | `false` | `false` | `false` | `false` | `false` | `false` |

### `volume`

| Field | docker | k8s | kubevirt | hyperlight | daytona | aca | subprocess | substrate | v8-wasm | modal | e2b | vercel | fly | proxmox | space | firecracker | firecracker (networked) | memory |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| `supported_modes` | `read_only, read_write, copy_on_write` | `read_only, read_write` | `read_only, read_write` | `read_only, read_write` | `read_write` | `read_only, read_write` | `read_only, read_write, copy_on_write` | `read_write` | `read_only, read_write` | `read_only, read_write` | `read_write` | `read_write` | none | none | none | none | none | none |
| `supports_fuse_mounts` | `true` | `false` | `false` | `false` | `false` | `false` | platform-dependent | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |
| `supports_s3_backend` | `true` | `true` | `true` | `false` | `false` | `true` | `false` | `false` | `false` | `true` | `false` | `false` | `false` | `false` | `false` | `false` | `false` | `false` |

<!-- END GENERATED: capability-matrix -->
