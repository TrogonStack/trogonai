# Run multiple agents as separate Linux users in one sandbox

This guide creates per-agent Linux users and groups inside a single sandbox lease, so several agents can share one sandbox while keeping isolated home directories and a controlled, shared workspace. SDK authors building multi-agent orchestration and operators provisioning sandbox images should follow these steps.

::: info Availability
Sandbox users and groups are supported on the Docker, Kubernetes, Firecracker and Substrate workers. Every other worker advertises `supports_sandbox_users: false`. The subprocess worker also advertises `false`, but permanently: its isolation model cannot host a real Linux user, so it answers every user, group or membership request with an explicit typed denial. The broker rejects lease requests that require sandbox users when no capable worker is available.
:::

## Why per-user isolation

Running several agents as the same Linux user means every agent can read, overwrite or delete every other agent's files. Creating one Linux user per agent gives you:

- Separate home directories (`mode 770`, owned by the user).
- A shared directory for deliberate hand-off between agents, backed by a Unix group rather than "everyone can see everything".
- The ability to run a command, write a file or read a file as a specific user, so file ownership on disk reflects which agent produced it.
- The ability to revoke an agent's access to the shared directory without tearing down the sandbox or the other agents.

## Prerequisites

In the sandbox image:

- `useradd`, `groupadd`, `usermod` and `gpasswd` must be on `PATH`. They ship in `shadow-utils` or `passwd` on most distributions.
- `/bin/bash` must exist, because new users are created with it as their login shell.
- On Substrate, the in-actor agent must run as root.

If a binary is missing, the Docker, Kubernetes and Substrate workers run a preflight check first and return a `MissingDependency` response naming the absent tool. See [Troubleshooting](#troubleshooting).

On the worker, no additional configuration is required. Sandbox user support is always on; there is no opt-in flag.

## Name validation

`SandboxUserName` and `SandboxGroupName` (in `trogon-sandbox-contract`) validate on construction and on deserialization:

- The name matches `^[a-z_][a-z0-9_-]*$`: lowercase ASCII letters, digits, underscore and hyphen, starting with a lowercase letter or underscore.
- The name is at most 32 bytes.

A name that breaks either rule cannot be constructed and cannot cross the wire. This is the command-injection barrier: spaces, quotes, shell metacharacters and path separators are unrepresentable, so they never reach the `useradd`, `groupadd`, `usermod` or `gpasswd` invocations. Validate client-side to get a faster, clearer error.

## Walkthrough: researcher, coder, reviewer

This walkthrough creates three agent users in one lease. The `researcher` and `coder` share a group directory to hand off a brief; the `reviewer` is added to the group later to read the result, then removed. Assume `lease_id` is the `LeaseId` of an active lease on a capable worker, and `client` is a `SandboxClient`.

### Step 1: Create a user per agent

```rust
use trogon_sandbox_contract::SandboxUserName;

let researcher = SandboxUserName::new("researcher")?;
let coder = SandboxUserName::new("coder")?;
let reviewer = SandboxUserName::new("reviewer")?;

client.create_user(&lease_id, researcher.clone()).await?;
client.create_user(&lease_id, coder.clone()).await?;
client.create_user(&lease_id, reviewer.clone()).await?;
```

Each call runs `useradd -m -s /bin/bash <name>` and then `chmod 770` on the new home directory, giving `/home/researcher`, `/home/coder` and `/home/reviewer`. The response is `Created { home_dir }`, or `AlreadyExists { home_dir }` for an existing user, so re-running setup against a warm sandbox is safe.

### Step 2: Create a shared group for hand-off

```rust
use trogon_sandbox_contract::SandboxGroupName;

let handoff = SandboxGroupName::new("handoff")?;
client.create_group(&lease_id, handoff.clone()).await?;
```

This runs `groupadd handoff`, creates `/shared/handoff`, and applies `chgrp handoff /shared/handoff` and `chmod 2770 /shared/handoff`. The setgid bit (the leading `2`) makes any file a member writes there inherit the `handoff` group, so every current member can read it without a per-file `chgrp`.

### Step 3: Add the researcher and coder to the group

```rust
use trogon_sandbox_contract::GroupMembershipAction;

client
    .modify_group_membership(&lease_id, researcher.clone(), handoff.clone(), GroupMembershipAction::Add)
    .await?;
client
    .modify_group_membership(&lease_id, coder.clone(), handoff.clone(), GroupMembershipAction::Add)
    .await?;
```

Each add runs `usermod -aG handoff <name>`. The researcher and coder can now read and write `/shared/handoff`; the reviewer cannot yet.

### Step 4: Run each agent as its own user

```rust
use trogon_sandbox_client::CommandOptions;

client
    .run_command(
        &lease_id,
        vec!["bash".into(), "-c".into(), "echo 'findings...' > /shared/handoff/brief.md".into()],
        CommandOptions {
            run_as_user: Some(researcher.clone()),
            ..CommandOptions::default()
        },
    )
    .await?;

client
    .run_command(
        &lease_id,
        vec!["bash".into(), "-c".into(), "cat /shared/handoff/brief.md".into()],
        CommandOptions {
            run_as_user: Some(coder.clone()),
            ..CommandOptions::default()
        },
    )
    .await?;
```

`write_files` and `read_file` take an `as_user: Option<SandboxUserName>` argument with the same meaning, so an agent's writes and reads are attributed to its own user.

### Step 5: Bring the reviewer in for the final artifact

```rust
client
    .modify_group_membership(&lease_id, reviewer.clone(), handoff.clone(), GroupMembershipAction::Add)
    .await?;
```

Then run the reviewer's command with `run_as_user: Some(reviewer.clone())`.

### Step 6: Revoke the reviewer's access

```rust
client
    .modify_group_membership(&lease_id, reviewer.clone(), handoff.clone(), GroupMembershipAction::Remove)
    .await?;
```

This runs `gpasswd -d reviewer handoff`. New processes started as `reviewer` can no longer read `/shared/handoff`. A process already running as `reviewer` keeps its group membership in the kernel until it exits, because membership is resolved at process start. To make revocation take effect for an in-flight process, kill it and restart it as `reviewer` after the `Remove` call returns. The researcher and coder keep their access.

## Run a privileged one-off command

`run_as_user` accepts any existing user on the sandbox, including `root`. There is no dedicated sudo mechanism; naming `root` is how you run a one-off command as that user:

```rust
client
    .run_command(
        &lease_id,
        vec!["bash".into(), "-c".into(), "apt-get install -y ripgrep".into()],
        CommandOptions {
            timeout_seconds: 60,
            run_as_user: Some(SandboxUserName::new("root")?),
            ..CommandOptions::default()
        },
    )
    .await?;
```

## Worker support

| Worker | Sandbox users | Notes |
| --- | --- | --- |
| `trogon-sandbox-docker-worker` | Yes | `useradd -m -s /bin/bash`, home mode `770`; groups get `/shared/<group>` mode `2770`; `MissingDependency` preflight before the first call. |
| `trogon-sandbox-k8s-worker` | Yes | Runs the same commands inside the pod; a command run as a user goes through `runuser`. Pod images are often minimal, so the `MissingDependency` preflight matters here. |
| `trogon-sandbox-firecracker-worker` | Yes | Advertised by the profile. |
| `trogon-sandbox-substrate-worker` | Yes | Provisioned by the in-actor agent (`trogon-sandbox-actor-agent`), because the Substrate control API has no exec call. A run as a user drops privileges in the spawned child. An agent that is not root inside the actor says so instead of failing opaquely. |
| `trogon-sandbox-subprocess-worker` | Never | Permanent typed denial; its isolation model has no host-visible place for `useradd` to persist a user. |
| Every other worker | No | `supports_sandbox_users` is unsupported. |

Setting `required_capabilities.needs_sandbox_users` on a lease filters out every provider that does not advertise support. See [Capability profiles](../../architecture/sandbox/capability-profiles.md).

## Client surfaces

Rust, `trogon-sandbox-client`:

- `create_user(&self, lease_id: &LeaseId, username: SandboxUserName) -> Result<CreateSandboxUserResponse, ClientError>`
- `create_group(&self, lease_id: &LeaseId, groupname: SandboxGroupName) -> Result<CreateSandboxGroupResponse, ClientError>`
- `modify_group_membership(&self, lease_id: &LeaseId, username: SandboxUserName, groupname: SandboxGroupName, action: GroupMembershipAction) -> Result<(), ClientError>`
- `CommandOptions` carries `run_as_user: Option<SandboxUserName>`.
- `write_files` and `read_file` take `as_user: Option<SandboxUserName>`.

MCP tools, `trogon-sandbox-mcp`:

- `sandbox_create_user` takes `lease_id` and `username` and returns the outcome `created`, `already_exists` or `missing_dependency`.
- `sandbox_create_group` takes `lease_id` and `groupname` and returns the same outcomes.
- `sandbox_modify_group_membership` takes `lease_id`, `username`, `groupname` and `action` (`add` or `remove`).

TypeScript, `@trogonai/sandbox-ws-client`:

- `sandbox.createUser(username): Promise<SandboxUser>` creates the user and returns a handle bound to it. `sandbox.asUser(username)` returns a handle without creating the user.
- `sandbox.createGroup(groupname): Promise<{ sharedDir: string }>`, `sandbox.addUserToGroup(username, groupname)` and `sandbox.removeUserFromGroup(username, groupname)`. The same methods exist on `SandboxWsClient` with the lease id as the first argument.
- `SandboxUser` exposes `runCommand`, `writeFiles` and `readFile`, each scoped to that user and defaulting `cwd` and relative paths to `/home/<username>`, plus `addToGroup(groupname)` and `removeFromGroup(groupname)`.

```ts
const researcher = await sandbox.createUser('researcher');
const coder = await sandbox.createUser('coder');

const { sharedDir } = await sandbox.createGroup('handoff');
await researcher.addToGroup('handoff');
await coder.addToGroup('handoff');

await researcher.runCommand(['bash', '-c', `echo 'findings...' > ${sharedDir}/brief.md`]);
const brief = await coder.runCommand(['cat', `${sharedDir}/brief.md`]);
console.log(brief.stdout());

await coder.removeFromGroup('handoff');
```

## Troubleshooting

### Missing `useradd`, `groupadd`, `usermod` or `gpasswd`

`create_user` or `create_group` returns `MissingDependency` naming the absent binary. Rebuild the sandbox image with `shadow-utils` (or your distribution's equivalent) and `bash` installed, then retry.

### `NoWorkerSatisfiesCapabilities` or a provider-specific denial

The broker returns `NoWorkerSatisfiesCapabilities` for a lease that sets `required_capabilities.needs_sandbox_users`, or a user, group or membership call against a lease on an unsupported provider comes back `Denied`. Confirm the lease's `provider_pref` includes a provider from the support table and that a matching worker is registered and sending heartbeats.

### Membership change did not take effect

A process running as a removed user can still read the shared directory after a `Remove` returns. Group membership is resolved when a process starts. Start a new process as that user, or wait for the existing one to exit.

### Permission denied on the shared directory despite being a member

Confirm the add completed (not `Denied`) and that the command is scoped with the right `run_as_user`. A process started before the add completed may carry stale group membership; issue a fresh command as that user after the add returns.

## See also

- [Capability profiles](../../architecture/sandbox/capability-profiles.md) for `IsolationCapability`, `CapabilityRequirements` and the honest-defaults principle behind `supports_sandbox_users`.
- [Mount remote storage into a sandbox](./mount-remote-storage.md) for the analogous capability-gated feature (`supports_fuse_mounts`).
