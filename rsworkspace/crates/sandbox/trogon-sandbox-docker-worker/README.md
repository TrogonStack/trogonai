# trogon-sandbox-docker-worker

A sandbox provider worker that runs each lease in a container on a local Docker daemon.

## Fit

The worker implements the provider side of `trogon-sandbox-protocol` for the `local_docker` provider kind. It drives
the daemon through `trogon-sandbox-providers`, reads leases and writes lifecycle events in the types of
`trogon-sandbox-contract`, and pushes and pulls snapshots as OCI artifacts.

## Live checkpoint

The worker advertises `snapshot.can_checkpoint_running`: it can checkpoint a running container with CRIU and leave it
running.

### Host requirements

- The Docker daemon runs with experimental features enabled, so `docker info` shows `Experimental: true`. On Linux that
  means starting `dockerd` with `--experimental` or setting `"experimental": true` in `/etc/docker/daemon.json`.
- CRIU is installed on the host and `criu check` succeeds.
- Some kernels and cgroup v2 setups still make CRIU fail on interactive workloads unless the container runs with a
  relaxed seccomp profile.

### Export

1. The checkpoint is created with the container left running, into a scratch directory the worker can write, not the
   daemon's own checkpoint directory under `/var/lib/docker`, which needs root.
2. That directory is tarred, gzipped and pushed as a `SandboxRootfsV1` artifact. Its layer carries the annotations
   `trogon.docker.checkpoint.v1=true` and `trogon.docker.checkpoint.name=<name>`, which tell it apart from a root
   filesystem export of the same artifact type.

### Restore

A lease restores a checkpoint through `SandboxImage::Snapshot` with `parent` set to the image the checkpoint was taken
from. The worker pulls the artifact, sees the checkpoint annotations, extracts it into a scratch directory, creates a
stopped container from `parent` and starts it from the checkpoint in that directory. Without `parent` the worker has no
image to recreate the container from.

### Moving checkpoints between hosts

CRIU state is not portable. A restore on another machine needs a matching kernel version and configuration, cgroup
hierarchy, CPU features, seccomp and AppArmor profiles, and Docker storage driver. Treat a checkpoint like a VM
migration image: restore it on the same host or a closely matched peer, and schedule checkpoint restores for locality.

## Testing

`cargo test -p trogon-sandbox-docker-worker` runs the unit tests and the lanes that need only a local `nats-server`.
Lanes that need a Docker daemon or prepared images are ignored by default. Prepare what the ignore reason names, then
run one with `--ignored`:

```sh
cargo test -p trogon-sandbox-docker-worker --test live_checkpoint -- --ignored --nocapture
```

| Test target | Needs |
| --- | --- |
| `live_checkpoint` | Docker in experimental mode, CRIU, and a local zot registry. |
| `egress_sidecar` | Docker, the `sandbox-egress-proxyd:dev` image (override with `SANDBOX_EGRESS_PROXY_IMAGE`), and a cached `busybox:1.36.1`. `only_proxied_tcp_leaves_the_sandbox` also needs internet access: it checks that DNS to `1.1.1.1` is refused and that `example.com` is reached through the proxy. |
| `conformance_egress_probe` | `SANDBOX_DOCKER_EGRESS_PROBE_TEST=1`, a cached Python 3 image without `nc` named in `SANDBOX_DOCKER_PYTHON_NO_NC_IMAGE`, the cached egress image, Docker and `nats-server`. Its invalid endpoint lane runs by default. |
| `command_readiness_process`, `custodian_signal_process`, `targeted_command_fence` | Docker, a cached `node:22`, `nats-server`, and a static command supervisor named in `SANDBOX_DOCKER_COMMAND_SUPERVISOR`. |
| `trusted_native_observer` | `SANDBOX_DOCKER_TRUSTED_OBSERVER=1`, Docker on Unix, and cached `python:3.12-slim` and `node:22` images. |
| `composed_named_cancellation` | `SANDBOX_DOCKER_COMPOSED_NAMED=1`, the command supervisor, Docker, cached `node:22`, SpiceDB and egress proxy images, `grpcurl`, `nats-server` and a built `trogon-sandbox-broker`. Its diagnostic tail lanes run by default. |
