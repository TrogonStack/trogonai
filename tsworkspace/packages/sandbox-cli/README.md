# @trogonai/sandbox-cli

Command-line interface for trogon-sandbox-ws-gateway, built on `@trogonai/sandbox-ws-client`. Binaries: `sandbox` and `sbx` (identical, `sbx` is just a short alias).

## Install

From `tsworkspace/`, install the workspace and build the client and the CLI:

```sh
pnpm install
pnpm --filter @trogonai/sandbox-ws-client build
pnpm --filter @trogonai/sandbox-cli build
```

Then run it directly from `packages/sandbox-cli`:

```sh
node dist/main.js --help
```

Or link `sandbox`/`sbx` onto your `PATH` however your package manager supports (e.g. `pnpm link --global`).

## Getting started

```sh
sandbox login --gateway-http https://gateway.example.com --gateway-ws wss://gateway.example.com --api-key <key> --tenant my-tenant
sandbox create --image docker.io/library/alpine:latest
sandbox list
sandbox exec <lease-id> -- echo hello
sandbox stop <lease-id>
```

Config is stored per-profile at `$XDG_CONFIG_HOME/trogon-sandbox/config.json` (or `~/.config/trogon-sandbox/config.json`), file mode `600`. Use `--profile <name>` to manage multiple gateways/tenants side by side. Any of `--gateway-http`, `--gateway-ws`, `--api-key`, `--tenant` can be passed on any command to override the stored profile for that single invocation.

## Command reference

Leases are the unit of identity: there is no separate sandbox name and no account, scope or project layer. A lease id plus a tenant and a bearer API key identify everything.

| Command                                                     | Notes                                                                                                                                                                             |
| ----------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sandbox login`                                             | No prompts, no OAuth; writes a named profile with gateway URLs + bearer API key + tenant.                                                                                         |
| `sandbox logout`                                            | Removes one profile from the config file.                                                                                                                                         |
| `sandbox list` / `ls`                                       | Table of live leases via `listLeases()`.                                                                                                                                          |
| `sandbox create`                                            | Creates a lease from an OCI image ref; no persistent "sandbox name" identity beyond the lease id (an optional `--name` tag is supported by the wire protocol and passed through). |
| `sandbox fork <lease-id>`                                   | Forks an existing lease into a new one.                                                                                                                                           |
| `sandbox run`                                               | Create + exec + stop, one shot; always attempts to stop the lease even on failure.                                                                                                |
| `sandbox exec <lease-id> -- <cmd>`                          | Runs a command in an existing lease; `--detached` mirrors backgrounding.                                                                                                          |
| `sandbox connect <lease-id>`                                | Delegates entirely to the local `ssh` binary against trogon-sandbox-ssh-gateway; see Limitations.                                                                                 |
| `sandbox copy <src> <dst>` / `cp`                           | Docker-style `<lease-id>:/path` remote syntax, one direction at a time.                                                                                                           |
| `sandbox stop <lease-id>`                                   | Stops a lease (worker released, lease record kept).                                                                                                                               |
| `sandbox remove <lease-id>` / `rm`                          | Deletes a lease entirely.                                                                                                                                                         |
| `sandbox config get\|set <key>`                             | Operates on the active profile's `tenant`, `gatewayHttp`, `gatewayWs`, `sshGateway`.                                                                                              |
| `sandbox kill <lease-id> <exec-id>`                         | Sends a Unix signal to a running command.                                                                                                                                         |
| `sandbox logs <lease-id> <exec-id>`                         | Post-hoc by default; `--follow` live-tails the evidence stream, see Limitations.                                                                                                  |
| `sandbox domain <lease-id> <port>`                          | Resolves a reachable `host:port` endpoint for a port exposed by a lease.                                                                                                          |
| `sandbox get <name>`                                        | Looks up a lease by its tenant-scoped `--name`.                                                                                                                                   |
| `sandbox drives list\|create\|delete`                       | `create` best-effort falls back to a lookup-by-name if the create call fails; see Limitations.                                                                                    |
| `sandbox snapshots list\|get\|tree`                         | `tree` ancestry may be flat; see Limitations.                                                                                                                                     |
| `sandbox tags list\|set\|delete\|history`                   | Lists, sets, deletes, and audits named snapshot tags exposed by this backend.                                                                                                     |
| `sandbox sessions list`                                     | Straight alias over `listLeases()`; see Limitations.                                                                                                                              |
| `sandbox user create` / `sandbox group create\|add\|remove` | Per-sandbox Linux user/group management specific to this backend.                                                                                                                 |
| `sandbox fs stat\|ls\|exists\|rm\|mkdir\|cp\|mv\|chmod`     | `run_command` sugar shelling out to coreutils, mirroring the SDK's `FileSystem` class; not a dedicated wire verb.                                                                 |

Not offered, with reasons:

- No account, project or scope flags: there is no hierarchy above a tenant, so `--tenant` plus `--api-key` cover the same ground.
- No runtime selection or port publishing: the wire protocol has neither concept.
- No atomic get-or-create for drives: the wire protocol has no such call; see Limitations for what `sandbox drives create` does instead.

## Limitations

- **Logs are post-hoc by default; `--follow` requires the execution to have been started with `--stream`.** Without `--follow`, `sandbox logs` and non-detached `sandbox exec` poll `pollCommand`/`runCommand` until the command finishes, then print the two buffered stdout/stderr chunks captured at completion (this mirrors `CommandFinished.logs()`/`Command.logs()` in `@trogonai/sandbox-ws-client`, which document the same limitation). `sandbox logs --follow` reads the lease's evidence stream directly and filters it for the given execution id's chunks, but chunks only ever land on that stream if the execution was started with `sandbox exec --stream` (wire `stream_output: true`); for a non-streamed execution, `--follow` silently shows nothing until the post-hoc completion output prints, which is expected, not an error. There is no wire call to retroactively ask whether a given execution id was started with streaming, so `--follow` cannot detect this in advance.
- **`sandbox connect` delegates to the local `ssh` binary.** There is no TTY support in the WS protocol itself. This command builds `ssh -p <port> <lease-id>@gw@<host>` (per trogon-sandbox-ssh-gateway's `parse_lease_user` convention: everything before the first `@` in the SSH username is the lease id) and execs it with inherited stdio if `ssh` is resolvable on `PATH`, or prints the command for you to run manually otherwise. There is no SSO or passwordless flow: the bearer API key must be typed or pasted as the SSH password when prompted. The gateway host has no compiled-in default (`GatewayConfig`'s bind address is operator-configured), so `--ssh-gateway <host[:port]>` or the `sshGateway` config key is required.
- **`sandbox drives create` has no guaranteed get-or-create semantics.** `@trogonai/sandbox-ws-client`'s `wire.ts` (`CreateDriveResponse`) and `errors.ts` (`SandboxWsError` and its subclasses `LeaseDeniedError`/`TenantMismatchError`/`TimeoutError`/`TransportError`) have no dedicated "drive already exists" error code or response variant. On any failure from `createDrive`, this command falls back to `listDrives()` and looks for a drive with the requested name; if found, it reports success instead of the original error. This is a heuristic, not a guarantee: a transient failure that isn't actually an "already exists" case will also match this fallback path if a same-named drive happens to already exist for an unrelated reason.
- **`sandbox snapshots tree` ancestry may be flat.** The wire response (`SnapshotTreeResponse.nodes`) supports multi-node ancestry, but until the backend/builder side actually constructs parent chains, a given digest may only ever resolve to a single node with no `parent`.
- **`sandbox sessions list` is a straight alias, not a distinct concept.** This backend has no sessions abstraction separate from leases; `sessions list` just calls the same `listLeases()` as `sandbox list`, for callers expecting a `sessions` verb.
- **`sandbox fs` is `run_command` sugar, not a dedicated wire verb.** Every `fs` subcommand shells out to coreutils (`stat`/`ls`/`rm`/`mkdir`/`cp`/`mv`/`chmod`) inside the sandbox via `runCommand`, mirroring `@trogonai/sandbox-ws-client`'s `FileSystem` class; there is no dedicated filesystem wire protocol.
- **No port publishing.** Nothing in the wire protocol exposes publishing a sandbox's network port externally (`sandbox domain` only resolves an already-exposed port to a reachable endpoint).
- **There are no project or scope flags.** Leases plus `--tenant` and `--api-key` (a bearer key) are the full identity model; there is no separate project or team layer to select between.
