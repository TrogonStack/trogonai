# trogon-sandbox-mcp

An MCP server that speaks over stdio and exposes sandbox lease operations as tools. It is a thin adapter over the `trogon-sandbox-client` NATS SDK: every call goes to the sandbox broker over NATS, and the crate has no HTTP or gRPC surface.

One server process serves one tenant. The tenant comes from `--tenant`, so a client cannot lease on behalf of another tenant.

## Install

From `rsworkspace/`:

```sh
cargo build -p trogon-sandbox-mcp --release
```

The binary is `rsworkspace/target/release/trogon-sandbox-mcp`.

## Run

```sh
trogon-sandbox-mcp \
  --tenant acme-corp \
  --nats-url nats://localhost:4222 \
  --log-level info
```

Stdout carries only MCP JSON-RPC frames. Logs go to stderr, so never redirect stderr into stdout.

The process starts even when the broker is unreachable or no API key is set. In both cases each tool call returns an error instead, so the MCP handshake still completes.

### Authentication

The server reads an API key from the first of these that is set:

1. `--api-key`
2. `SANDBOX_API_KEY`
3. `~/.config/trogon-sandbox-mcp/api-key`, which must have mode `0600`

Without a key, every tool call returns an `Unauthenticated` error that names these three places.

### CLI flags

| Flag | Environment variable | Default | Purpose |
| --- | --- | --- | --- |
| `--tenant` | `SANDBOX_TENANT` | required | Tenant every operation runs under. |
| `--api-key` | `SANDBOX_API_KEY` | none | API key sent with each broker request. |
| `--nats-url` | `SANDBOX_NATS_URL` | `nats://localhost:4222` | NATS server the broker listens on. |
| `--log-level` | `SANDBOX_MCP_LOG_LEVEL` | `info` | `tracing` filter for the logs on stderr. A value that does not parse falls back to `info`. |
| `--allow-tool` | `SANDBOX_MCP_ALLOW_TOOLS` | all tools | Offer only matching tools. Repeatable. The variable is comma separated. |
| `--deny-tool` | `SANDBOX_MCP_DENY_TOOLS` | none | Never offer matching tools. Wins over `--allow-tool`. Repeatable. The variable is comma separated. |
| `--allow-command` | `SANDBOX_MCP_ALLOW_COMMANDS` | any | Permit `sandbox_run_command` only when the command matches one of these regexes. Repeatable. The variable is newline separated. |
| `--deny-command` | `SANDBOX_MCP_DENY_COMMANDS` | none | Refuse `sandbox_run_command` when the command matches a regex. Repeatable. The variable is newline separated. |
| `--root` | `SANDBOX_MCP_ROOTS` | none | Confine the file tools to these absolute sandbox directories. Repeatable. The variable is comma separated. |

## Role scoping

Three independent filters narrow what a client can do. They apply to whatever tools the server registers, so a new tool follows the same rules without a change to the server.

- **Tool filter.** A pattern is an exact tool name or a prefix ending in `*`. An empty allow list offers every tool, and a deny pattern always wins. A hidden tool is absent from `tools/list`. Calling it anyway returns an error result, `tool denied by server policy`, not a protocol failure. A pattern that matches no registered tool is logged as a warning at startup, which catches typos.
- **Command policy.** `sandbox_run_command` joins the argv with single spaces and matches that text against the regexes. A command that matches any `--deny-command` is refused. With `--allow-command` set, a command that matches none of them is refused. Regexes use the Rust `regex` syntax and are not anchored, so write `^` and `$` when you mean them.
- **Terminal refusal.** A terminal runs whatever is typed into it, which the command policy cannot check. `sandbox_terminal_open` is therefore refused whenever `--allow-command` or `--deny-command` is set.

The command policy is a guard rail for a cooperative agent, not a sandbox boundary. It matches the argv text and cannot see what a script goes on to run.

Read-only role, hiding everything that mutates a lease or runs code:

```sh
trogon-sandbox-mcp --tenant acme-corp \
  --allow-tool 'sandbox_list_*' --allow-tool 'sandbox_read_*' \
  --allow-tool sandbox_stat --allow-tool sandbox_usage
```

Run only a test command:

```sh
trogon-sandbox-mcp --tenant acme-corp --allow-command '^cargo test( |$)'
```

## Root confinement

With `--root` set, every path the file tools take must sit under one of the roots. This covers reads, writes, listings, edits, search, archives, directory operations and both ends of a rename.

- Roots must be absolute and must not contain `..`. A bad root stops the server at startup.
- A relative path resolves against the first root. An absolute path must be under some root, compared by whole path component, so `/workspace2` is not under `/workspace`.
- A path that contains `..` is refused outright.
- Each component below the root is checked with a `stat` that does not follow links, and a symlink is refused.
- A refusal returns `path denied by server policy`.

The check fails closed. A provider that cannot answer `stat` makes every file tool call under `--root` fail. It runs before the operation, so it guards an agent that wanders out of its workspace. It does not defend against a process inside the sandbox that swaps a path between the check and the operation. Workers built on `trogon-sandbox-file-ops` also check where each opened descriptor landed and refuse one outside the sandbox root, so a swapped path cannot reach the host. A swap can still steer a call to another directory inside the sandbox. Providers reached only through their own API get the pre-check alone.

## Configure an MCP client

Add a stdio server entry to the client's MCP configuration. The exact file depends on the client.

```json
{
  "mcpServers": {
    "sandbox": {
      "command": "/path/to/rsworkspace/target/release/trogon-sandbox-mcp",
      "args": ["--tenant", "acme-corp", "--nats-url", "nats://localhost:4222"],
      "env": {
        "SANDBOX_API_KEY": "<api-key>"
      }
    }
  }
}
```

To scope the client, add flags to `args`, for example `"--root", "/workspace", "--deny-tool", "sandbox_delete_lease"`.

## Tools

Tool names are stable and every one starts with `sandbox_`. The server advertises the tools and logging capabilities. Evidence frames from `sandbox_subscribe_evidence` arrive as logging notifications from the logger `sandbox.evidence`. Ask the client for `tools/list` for the full input schema of each tool.

| Area | Tools |
| --- | --- |
| Leases | `sandbox_lease`, `sandbox_cancel`, `sandbox_wait_released`, `sandbox_list_leases`, `sandbox_get_lease_by_name`, `sandbox_reap_leases`, `sandbox_extend_timeout`, `sandbox_delete_lease`, `sandbox_fork`, `sandbox_usage`, `sandbox_limits`, `sandbox_list_capabilities`, `sandbox_subscribe_evidence` |
| Lifecycle | `sandbox_stop_lease`, `sandbox_pause_lease`, `sandbox_resume_lease`, `sandbox_suspend_lease`, `sandbox_wake_lease`, `sandbox_update_network_policy` |
| Snapshots | `sandbox_snapshot_lease`, `sandbox_snapshot_get`, `sandbox_snapshot_delete`, `sandbox_snapshot_tree`, `sandbox_snapshot_tag_history`, `sandbox_set_snapshot_tag`, `sandbox_delete_snapshot_tag` |
| Commands | `sandbox_run_command`, `sandbox_poll_command`, `sandbox_read_logs`, `sandbox_kill_command`, `sandbox_stop_command`, `sandbox_restart_command`, `sandbox_write_stdin`, `sandbox_list_processes`, `sandbox_wait_ready`, `sandbox_kill_process` |
| Files | `sandbox_write_files`, `sandbox_read_file`, `sandbox_read_range`, `sandbox_list_dir`, `sandbox_stat`, `sandbox_edit_file`, `sandbox_search_files`, `sandbox_mkdir`, `sandbox_remove_path`, `sandbox_rename_path`, `sandbox_archive_get`, `sandbox_archive_put`, `sandbox_checkpoint`, `sandbox_changes`, `sandbox_watch_start`, `sandbox_watch_poll`, `sandbox_watch_stop` |
| Terminals | `sandbox_terminal_open`, `sandbox_terminal_write`, `sandbox_terminal_read`, `sandbox_terminal_resize`, `sandbox_terminal_close` |
| Accounts and access | `sandbox_create_user`, `sandbox_create_group`, `sandbox_modify_group_membership`, `sandbox_get_domain`, `sandbox_native_handle`, `sandbox_ssh_access` |

Which tools work on a lease depends on the capabilities of its provider. A tool the provider cannot serve returns an `unsupported` reply.

### Examples

Request a lease. The input is a `LeaseRequest` without the tenant:

```json
{
  "name": "sandbox_lease",
  "arguments": {
    "provider_pref": ["docker"],
    "spec": {
      "image": {"kind": "registry", "reference": {"registry": "", "repository": "ubuntu"}},
      "workspace": {"empty": {}},
      "cpu_seconds": 60,
      "memory_mib": 512,
      "env": {}
    },
    "ttl_seconds": 600,
    "idempotency_key": "demo-1"
  }
}
```

Run a command and wait for it:

```json
{
  "name": "sandbox_run_command",
  "arguments": {
    "lease_id": "<lease-id>",
    "command": ["sh", "-c", "echo hello"],
    "timeout_seconds": 30
  }
}
```

Start it detached and poll for the result:

```json
{"name": "sandbox_run_command", "arguments": {"lease_id": "<lease-id>", "command": ["sleep", "5"], "timeout_seconds": 30, "detached": true}}
{"name": "sandbox_poll_command", "arguments": {"lease_id": "<lease-id>", "execution_id": "<execution-id>"}}
```

Set `stream_progress` to `true`, with a progress token on the call, to receive the output as MCP progress notifications while an attached command runs. Use `redact_values` to scrub exact literal secrets from the output.

Write and read a file. File content travels as base64:

```json
{"name": "sandbox_write_files", "arguments": {"lease_id": "<lease-id>", "files": [{"path": "hello.txt", "content_base64": "aGVsbG8K"}]}}
{"name": "sandbox_read_file", "arguments": {"lease_id": "<lease-id>", "path": "hello.txt"}}
```

Release the lease:

```json
{"name": "sandbox_cancel", "arguments": {"lease_id": "<lease-id>"}}
```

## Tests

```sh
cargo test -p trogon-sandbox-mcp
```
