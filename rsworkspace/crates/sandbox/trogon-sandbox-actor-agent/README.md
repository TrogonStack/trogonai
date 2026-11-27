# Sandbox actor agent

`trogon-sandbox-actor-agent` is the process that runs inside every sandbox actor. The
runtime exposes no exec RPC, so the sandbox exec surface reaches an actor through this
agent: the worker calls it over the actor's HTTP ingress, and it answers with the same
`trogon-sandbox-contract` types every other provider replies with.

The guest protocol types are always available. The HTTP server and the
`trogon-sandbox-actor-agent` binary sit behind the `server` feature (on by default), so a
worker that only speaks the protocol depends on the crate with default features off and
does not link the server.

## Guest protocol

- `GUEST_PROTOCOL_VERSION` and the version headers. A worker sends its version in
  `x-sandbox-protocol-version`; a missing header counts as version 0. The agent refuses
  a worker below its minimum with status 426 and a typed `GuestProtocolError`, and
  announces its own version on every reserved path and on `/readyz`, which is never
  gated. `SANDBOX_MIN_WORKER_PROTOCOL_VERSION` lowers the minimum on the agent.
- `GuestCallFailure` reduces a failed reply to what decides whether a legacy path may
  be tried. Only an explicit unimplemented answer allows a fallback; an unexpected reply
  never does, because the first attempt may already have run.
- `ActorId`, `ActorIdentity`, `BindLeaseRequest`, `BindLeaseResponse`,
  `GuestEntropySeed` (its debug form is redacted), `ExecFrame` and `TerminalFrame`
  are the wire types. The paths are exported as constants.

## Server

- `router` builds the axum application from an `AgentState`; `serve` and `serve_vsock`
  run it until a cancellation token fires. `serve_vsock` is Linux only and reports
  `Unsupported` elsewhere.
- Binding a lease installs the environment and working directory later exec, file,
  user, group and terminal calls run under, and reseeds the kernel entropy pool when the
  request carries a seed. Calls naming any other lease are denied.
- `ExecRegistry` runs commands, attached or detached, with sequenced interactive
  stdin, output capped to a head and a tail, timeouts, signals and polling.
- `TerminalSessions` runs pseudo-terminal sessions with gapless output frames and a
  single exit frame.
- `read_file`, `write_files`, `create_user`, `create_group` and
  `modify_group_membership` implement the file and account operations.
- Requests for paths outside the reserved prefix are proxied to the lease's exposed
  port.

The server code assumes a Unix guest. The agent runs as root so it can provision users.

## Running

```sh
trogon-sandbox-actor-agent --port 8110 --bind 0.0.0.0
```

Every flag has a `SANDBOX_ACTOR_AGENT_*` environment variable: `PORT`, `BIND`,
`ACTOR_ID_PATH` and `VSOCK_PORT`. The `Dockerfile` builds the binary into a Debian slim
image; build it from the repository root.

## Related crates

- `trogon-sandbox-contract` defines the request and response types the agent answers
  with.
- `trogon-sandbox-protocol` provides the stdin sequencer, terminal frame helpers and
  the output cap the agent builds on.
