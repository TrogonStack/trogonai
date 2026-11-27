# Mount remote storage into a sandbox

Attach S3 bucket prefixes as streaming FUSE mounts inside a sandbox lease. Worker-managed FUSE mounts let the sandbox read large datasets, model weights or reference assets directly from S3 without copying them to the worker disk first. Operators deploying workers and SDK authors building lease requests should follow these steps.

::: info Availability
FUSE mounts are supported on the subprocess worker (`trogon-sandbox-subprocess-worker`, Linux only) and the Docker worker (`trogon-sandbox-docker-worker`). The other workers advertise `supports_fuse_mounts` as unsupported. The broker rejects lease requests that require FUSE mounts if no capable worker is available. See [Volumes](../../architecture/sandbox/volumes.md) for the worker support matrix.
:::

## Prerequisites

On each worker host:

- `mountpoint-s3` (the `mount-s3` binary) must be installed and on the worker's `PATH`, or its absolute path must be set with `SANDBOX_FUSE_MOUNT_S3_BINARY`. It is a single static binary with no runtime dependencies.
- The `fuse3` kernel module must be loaded (`modprobe fuse`). Verify with `ls /dev/fuse`. Most Linux distributions package it as `fuse3`.
- The worker process must be allowed to mount FUSE filesystems. On most systems this means the worker runs as root, or `/dev/fuse` is accessible and `/etc/fuse.conf` sets `user_allow_other` when non-root mounting is needed.

For the AWS identity:

- The worker host's IAM role (or the credentials in its environment) must have `s3:GetObject` (and optionally `s3:PutObject`) on the buckets and prefixes you intend to mount. No credentials are passed to the sandbox; the worker's ambient IAM identity is used exclusively.

## Step 1: Configure the subprocess worker

The subprocess worker reads its FUSE configuration from environment variables at startup. Set them in the worker's systemd unit, container environment or process supervisor:

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `SANDBOX_FUSE_ENABLED` | Yes | `false` | Set to `1` or `true` to enable FUSE support. Capability advertising is static per provider, so a worker with FUSE disabled still advertises `supports_fuse_mounts` on Linux and fails mount-bearing leases at launch time with a clear error. Enable this on every subprocess worker host or on none of them. |
| `SANDBOX_FUSE_MOUNT_S3_BINARY` | No | `mount-s3` | Path to the `mount-s3` binary. If omitted the binary must be on `PATH`. |
| `SANDBOX_FUSE_REGION` | No | empty | AWS region passed to `mount-s3` with `--region`. If omitted, `mount-s3` resolves the region itself. |
| `SANDBOX_FUSE_ENDPOINT_URL` | No | unset | Custom S3 endpoint URL for S3-compatible stores. Passed to `mount-s3` with `--endpoint-url`. Leave unset for AWS S3. |
| `SANDBOX_FUSE_ALLOW_OTHER` | No | `false` | Set to `1` or `true` to pass `--allow-other` to `mount-s3`, which lets users other than the mount owner (including sandbox processes running as a different UID) read the mount. Required in most setups where the sandbox process runs as an unprivileged user that is not the worker user. Requires `user_allow_other` in `/etc/fuse.conf`. |

Example systemd `EnvironmentFile` entry:

```text
SANDBOX_FUSE_ENABLED=1
SANDBOX_FUSE_MOUNT_S3_BINARY=/usr/local/bin/mount-s3
SANDBOX_FUSE_REGION=us-east-1
SANDBOX_FUSE_ALLOW_OTHER=1
```

## Step 2: Verify the Docker worker

The Docker worker builds its FUSE configuration from the same S3 volume settings it uses for volumes: `SANDBOX_VOLUME_S3_REGION` (default `us-east-1`) and `SANDBOX_VOLUME_S3_ENDPOINT`. It always enables `allow_other`, because containers run as a different user than the host worker. There are no additional FUSE-specific environment variables for the Docker worker.

The Docker worker advertises `supports_fuse_mounts` unconditionally. FUSE mounts fail at runtime on non-Linux hosts (the mount call returns `VolumeError::FuseUnsupported`), so do not use FUSE mounts with a Docker worker on macOS or Windows.

## Step 3: Issue a lease request with `spec.mounts`

Add one or more `RemoteMount` entries to `SandboxSpec.mounts`. Each entry specifies a source and a destination path inside the sandbox. The only source backend is `s3`.

### Read-only S3 mount

```json
{
  "tenant": "acme",
  "provider_pref": ["subprocess"],
  "spec": {
    "image": {
      "kind": "registry",
      "reference": {
        "registry": "docker.io",
        "repository": "library/ubuntu",
        "tag": "24.04"
      }
    },
    "workspace": { "empty": {} },
    "cpu_seconds": 30,
    "memory_mib": 512,
    "mounts": [
      {
        "source": {
          "backend": "s3",
          "bucket": "acme-model-weights",
          "prefix": "checkpoints/model-a/"
        },
        "mount_path": "/mnt/weights",
        "readonly": true
      }
    ]
  },
  "ttl_seconds": 300,
  "idempotency_key": "acme-weights-run-01"
}
```

The sandbox process sees `/mnt/weights` populated with the objects under `s3://acme-model-weights/checkpoints/model-a/`. Reads are served directly from S3; the worker disk is not used as an intermediate copy.

### Read-write S3 mount

```json
{
  "tenant": "acme",
  "provider_pref": ["local_docker"],
  "spec": {
    "image": {
      "kind": "registry",
      "reference": {
        "registry": "docker.io",
        "repository": "acme/runner"
      }
    },
    "workspace": { "empty": {} },
    "cpu_seconds": 60,
    "memory_mib": 1024,
    "mounts": [
      {
        "source": {
          "backend": "s3",
          "bucket": "acme-scratch",
          "prefix": "runs/2026-07-03/"
        },
        "mount_path": "/mnt/output",
        "readonly": false
      }
    ]
  },
  "ttl_seconds": 600,
  "idempotency_key": "acme-output-run-01"
}
```

With `readonly` set to `false`, the sandbox can write files under `/mnt/output` and `mountpoint-s3` uploads them to S3. `mountpoint-s3` imposes its own write semantics: sequential writes to new files are supported, and in-place modification of existing objects is not. Plan your output layout accordingly. An omitted `readonly` reads as writable.

### Multiple mounts

You can specify multiple entries in `mounts`. Each entry must use a distinct `mount_path`. The spec is rejected with a `duplicate mount_path` error otherwise.

```json
"mounts": [
  {
    "source": { "backend": "s3", "bucket": "acme-datasets", "prefix": "train/" },
    "mount_path": "/mnt/train",
    "readonly": true
  },
  {
    "source": { "backend": "s3", "bucket": "acme-artifacts", "prefix": "outputs/" },
    "mount_path": "/mnt/out",
    "readonly": false
  }
]
```

## Step 4: Confirm capability inference

You do not need to set `required_capabilities.needs_fuse_mounts` explicitly. The broker infers `needs_fuse_mounts` as required whenever `spec.mounts` is non-empty. If you also have other capability requirements, pass them normally with `required_capabilities`; the broker merges them with the inferred FUSE requirement.

## Troubleshooting

### Mount readiness timeout

Symptom: the lease fails with an error message containing `FuseMountTimeout`.

The worker calls `mount-s3`, then polls `/proc/mounts` for up to 10 seconds waiting for a FUSE entry to appear at the expected host directory. If the poll times out, it returns `FuseMountTimeout`.

Checks:

1. Confirm `mount-s3` is executable at the path given by `SANDBOX_FUSE_MOUNT_S3_BINARY` (or on `PATH`).
2. Run `mount-s3 <bucket> <dir> --read-only` manually as the worker user to see what error the binary itself reports.
3. Check AWS credential availability with `aws sts get-caller-identity` as the worker user. A missing IAM role or misconfigured credential chain produces an auth error from `mount-s3` that surfaces in the worker logs.
4. Confirm the bucket exists and the worker's IAM identity has at minimum `s3:GetObject` and `s3:ListBucket` on the target prefix.

### Missing `/dev/fuse`

Symptom: `mount-s3` fails immediately with a message like `fuse: device not found, try 'modprobe fuse'`.

Run `modprobe fuse` on the worker host. On container-based deployments, ensure the container has access to the `/dev/fuse` device, for example by adding `--device /dev/fuse` to the Docker run command.

### `allow_other` and user mismatch

Symptom: the sandbox process receives `Permission denied` when reading from the mount point even though the mount is visible in `findmnt`.

This happens when the mount is owned by the worker user and the sandbox process runs as a different UID. The FUSE kernel driver blocks cross-user access unless the mount was created with `--allow-other`.

For the subprocess worker, set `SANDBOX_FUSE_ALLOW_OTHER=1` in the worker environment and ensure `/etc/fuse.conf` contains `user_allow_other`.

For the Docker worker, `allow_other` is always enabled. If you still see the error, check that the container's user has read permissions on the mounted directory. The bind mount inherits the host directory ownership; running the container with `--user` may expose this.

### No worker satisfies capabilities

Symptom: the broker returns `NoWorkerSatisfiesCapabilities`.

The lease request includes `spec.mounts` entries, so the broker requires `supports_fuse_mounts` from the selected worker, and no registered worker advertised it.

Checks:

1. Confirm a subprocess or Docker worker is registered and sending heartbeats. A worker with no recent heartbeat is not considered by the broker.
2. Confirm the lease is not restricted to providers without FUSE support by `provider_pref` or `required_capabilities`.

Symptom: the lease fails at launch with `lease requests mounts but FUSE is disabled; set SANDBOX_FUSE_ENABLED=1` or a mount error.

Capability advertising is static per provider, so the broker can route a mount-bearing lease to a worker whose host is not set up for FUSE.

Checks:

1. Confirm the subprocess worker has `SANDBOX_FUSE_ENABLED=1` set and has restarted after that environment change.
2. Confirm `mount-s3` is installed and `/dev/fuse` exists on the worker host.
3. Confirm the worker runs on a Linux host. FUSE mounts return `FuseUnsupported` elsewhere, even though the Docker worker always advertises `supports_fuse_mounts`.

## See also

- [Volumes](../../architecture/sandbox/volumes.md): copy-based modes compared with streaming mounts, and the worker matrix.
- [Capability profiles](../../architecture/sandbox/capability-profiles.md): the `VolumeCapability` flags.
- [Restore from a snapshot](./restore-from-snapshot.md): snapshot restore, which is independent of mounts.
