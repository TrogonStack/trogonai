# Run the subprocess worker on macOS

This guide starts the subprocess worker on a Mac, where it confines each lease with `sandbox-exec` (Seatbelt) instead of bubblewrap. Use it for local development and for trying the stack without Docker or a Linux machine.

## Prerequisites

- `nats-server` with JetStream on PATH, for example from `brew install nats-server`.
- A Rust toolchain. Run the commands below from `rsworkspace/`.
- `/usr/bin/sandbox-exec`, which ships with macOS.

## Start the worker

```sh
cargo build -p trogon-sandbox-broker -p trogon-sandbox-subprocess-worker -p trogon-sandbox-egress-proxy
nats-server -js -p 4222 &

SANDBOX_NATS_URL=nats://127.0.0.1:4222 \
SANDBOX_AGENT_BINARY=/path/to/your/agent \
SANDBOX_WORKSPACE_ROOT="$HOME/sandbox-workspaces" \
SANDBOX_EGRESS_PROXY_BINARY="$PWD/target/debug/trogon-sandbox-egress-proxyd" \
  target/debug/trogon-sandbox-subprocess-worker
```

The isolation backend defaults to `seatbelt` on macOS. Set `SANDBOX_ISOLATION_BACKEND=none` to run the agent as a plain host process with no confinement.

`trogon-sandbox-dev` does the same in one command and picks the subprocess worker when you pass `--agent-binary` and Docker is not running:

```sh
cargo run -p trogon-sandbox-dev -- --agent-binary /path/to/your/agent
```

See [Run the whole sandbox stack locally with one command](./run-the-stack-locally.md).

## Know the limits of Seatbelt

The worker's profile advertises only what holds on macOS:

- No network namespace and no mount namespace. The payload sees the host file tree, and policy paths under `/workspace` are rewritten to where the workspace really is.
- Rules are path based, so a hard link made before the launch still reaches the same data.
- No masked files and no FUSE volume mounts. A lease that needs either is refused rather than approximated.
- `sandbox-exec` is deprecated by Apple, though still shipped.

Isolation here is a development guard rail, not a boundary for untrusted code. Use the Linux worker or Docker for that. See [Subprocess isolation](../../architecture/sandbox/subprocess-isolation.md) for how the backends differ.
