# Run the sandbox Docker smoke test

This guide verifies the path from NATS to the broker to the Docker worker on one machine. The smoke script starts NATS and the broker in Compose, runs the Docker worker on the host so it can use your Docker socket, and checks that both stay connected to NATS past one worker heartbeat.

## When to use this

- After changing the broker or worker NATS wiring, lease payloads or Docker provider behavior.
- When validating that a machine has Docker, Compose and the Rust toolchain wired for local development.

## Prerequisites

- Docker and Docker Compose (the `docker compose` plugin).
- `curl`, `jq`, `openssl` and a Rust toolchain with `cargo`.
- A clone of the repository. Paths below assume the repository root.

Optional checks:

```sh
docker info
docker compose version
cargo --version
```

## Run the script

From the repository root:

```sh
mise run sandbox:smoke-docker
```

The script does the following:

1. Generates a dispatch signing key with `.config/mise/lib/sandbox-dispatch-keys.sh`, so the worker runs in the `require` posture described in [Sign lease dispatches and verify them on workers](./sign-worker-dispatches.md).
2. Starts NATS with JetStream and the broker through `devops/docker/compose/sandbox/compose.yaml`. The NATS client port on the host is 24222 and the monitoring port is 28222.
3. Waits for NATS to report healthy.
4. Builds `trogon-sandbox-docker-worker` in release mode.
5. Starts the worker on the host with its own temporary state directory.
6. Waits one heartbeat interval, then checks that the worker and the broker are running and that NATS shows at least two connections.

A successful run ends with `OK` and the command that stops the stack.

### Change the ports or the image

Set `SANDBOX_NATS_PORT` and `SANDBOX_NATS_MONITOR_PORT` to change the host ports. Set `SANDBOX_IMAGE` to change the lease image. It defaults to `sandbox-agent:dev`, which you build from `devops/docker/images/sandbox-agent`. See [Build an agent image](./build-an-agent-image.md).

```sh
SANDBOX_NATS_PORT=24222 SANDBOX_IMAGE='sandbox-agent:dev' \
  mise run sandbox:smoke-docker
```

## Start the pieces by hand

1. Start NATS and the broker. Source the key script first, because the Compose file refuses to start the broker without a signing key:

   ```sh
   source .config/mise/lib/sandbox-dispatch-keys.sh
   mise run compose:up sandbox --build
   ```

2. Build the Docker worker from `rsworkspace`:

   ```sh
   cd rsworkspace
   cargo build -p trogon-sandbox-docker-worker --release
   ```

3. Run the worker on the host:

   ```sh
   SANDBOX_NATS_URL='nats://127.0.0.1:24222' \
   SANDBOX_IMAGE='sandbox-agent:dev' \
   SANDBOX_DOCKER_STATE_DIR="$(mktemp -d)" \
     target/release/trogon-sandbox-docker-worker
   ```

   Use the target directory your Cargo configuration reports if it is not `target`.

The worker keeps an owner id in `SANDBOX_DOCKER_STATE_DIR` (default `/var/lib/sandbox-docker`) and labels every container with it. On start it removes the containers a previous run left under the same owner. Give each worker that shares a Docker daemon its own state directory, because a second worker on the same directory is refused. `SANDBOX_DOCKER_WORKER_OWNER` overrides the persisted id.

## What good looks like

- Compose shows `nats` and `trogon-sandbox-broker` running.
- The worker logs show it connected to NATS and serving lease traffic.
- The script ends with `OK`.

## Troubleshoot

| Symptom | Things to check |
|--------|------------------|
| Port already in use | Another NATS or service on 24222 or 28222. Set `SANDBOX_NATS_PORT` and `SANDBOX_NATS_MONITOR_PORT`, or stop the conflicting process. |
| The worker exited during startup | Docker daemon running? The worker runs on the host and needs the Docker socket. Check the worker output for the cause. |
| The broker exited during startup | The script prints the last broker log lines. Confirm `.config/mise/lib/sandbox-dispatch-keys.sh` was sourced so the broker has a signing key. |
| Fewer than two NATS connections | The worker or the broker did not connect. Check that both use the host-forwarded URL `nats://127.0.0.1:24222`. |

The NATS monitoring endpoint is on host port 28222 if you need health or connection data.

## Tear down

```sh
mise run compose:down sandbox
```

Stop the host worker with Ctrl+C if you started it by hand.
