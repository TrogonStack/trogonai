# Your first sandbox lease

This tutorial takes you from a clean checkout to a sandbox lease that runs a small agent in an isolated subprocess. By the end you will have started the broker and the subprocess worker, allowed an image for a tenant, leased a sandbox from Rust, watched the agent run, and seen the lease released.

You need `cargo` and `nats-server` on your path. On Linux the subprocess worker isolates leases with bubblewrap, so `bwrap` must be on your path as well. On macOS it uses Seatbelt (`sandbox-exec`), which ships with the system.

## What you will build

```text
trogon-sandbox-broker  <->  NATS  <->  trogon-sandbox-subprocess-worker
          ^
   your program (trogon-sandbox-client) requests a lease and waits for its release
```

## Steps

### 1. Build the binaries

From the repository root:

```bash
cd rsworkspace
cargo build -p trogon-sandbox-broker -p trogon-sandbox-admin -p trogon-sandbox-subprocess-worker
export PATH="$PWD/target/debug:$PATH"
```

Run the `export` in every terminal you open below, or call the binaries by their full path.

### 2. Start NATS

```bash
nats-server -js -p 4222
```

JetStream (`-js`) is required: the broker keeps leases, quotas, idempotency records and image policies in JetStream key-value buckets. Leave it running.

### 3. Start the broker

In a second terminal:

```bash
SANDBOX_AUTHZ_ENFORCE=false \
SANDBOX_ENFORCE_WORKER_REGISTRY=false \
trogon-sandbox-broker \
  --nats-url nats://127.0.0.1:4222 \
  --jetstream-replicas 1
```

The broker enforces three checks by default: bearer-key authorization, the per-tenant image allowlist and the worker registry. Authorization needs a SpiceDB server (`SPICEDB_ENDPOINT` and `SPICEDB_TOKEN`), and the worker registry needs registered worker identities, so this tutorial turns both off. The image allowlist stays on, so you will configure it in the next step. The broker creates its buckets with three replicas by default; a single `nats-server` has no cluster, so `--jetstream-replicas 1` lowers that.

`--insecure-dev` turns off all three checks in one switch. Do not use either form outside a development machine.

Leave the broker running.

### 4. Write the agent and allow it

The subprocess worker runs the lease image as a program. A registry image with no registry host, such as an absolute path, names an executable on the worker host. Create a small agent script:

```bash
mkdir -p "$HOME/sandbox-tutorial/workspaces"
cat > "$HOME/sandbox-tutorial/agent.sh" <<'EOF'
#!/bin/sh
echo "lease $SANDBOX_LEASE_ID for tenant $SANDBOX_TENANT says $GREETING"
sleep 5
EOF
chmod +x "$HOME/sandbox-tutorial/agent.sh"
```

The worker sets `SANDBOX_LEASE_ID`, `SANDBOX_TENANT`, `SANDBOX_WORKSPACE` and `SANDBOX_EVIDENCE_ROOT` for every lease. `GREETING` comes from the lease request you will write in step 6.

Allow tenant `demo` to run anything in that directory:

```bash
trogon-sandbox-admin --nats-url nats://127.0.0.1:4222 \
  image-policy set --tenant demo --pattern "$HOME/sandbox-tutorial/*"
```

The command prints the stored policy. A pattern may end in a single `*`; anything else must match exactly. `trogon-sandbox-admin` writes the policy straight to the `sandbox-image-allowlist` bucket, so it works whether or not the broker is running.

### 5. Start the subprocess worker

In a third terminal:

```bash
trogon-sandbox-subprocess-worker \
  --nats-url nats://127.0.0.1:4222 \
  --agent-binary "$HOME/sandbox-tutorial/agent.sh" \
  --workspace-root "$HOME/sandbox-tutorial/workspaces"
```

`--agent-binary` is the program the worker runs when a lease does not name one, and `--workspace-root` is where it creates each lease's workspace. The isolation backend defaults to `bubblewrap` on Linux and `seatbelt` on macOS; `--isolation-backend` overrides it. The worker logs `worker ready` and starts sending heartbeats.

Confirm the broker side can see the worker:

```bash
trogon-sandbox-admin --nats-url nats://127.0.0.1:4222 capability show subprocess
```

The output is the capability profile the worker advertised in its heartbeat. If it prints `no live worker heartbeat received for provider 'Subprocess'`, the worker is not connected yet.

### 6. Lease a sandbox

In a fourth terminal, create a project and add the client crates from your checkout. Set `TROGONAI` to the repository root:

```bash
cargo new first-lease
cd first-lease
cargo add --path "$TROGONAI/rsworkspace/crates/sandbox/trogon-sandbox-client"
cargo add --path "$TROGONAI/rsworkspace/crates/sandbox/trogon-sandbox-contract"
cargo add --path "$TROGONAI/rsworkspace/crates/sandbox/trogon-sandbox-protocol"
cargo add tokio --features macros,rt-multi-thread
```

Replace `src/main.rs` with:

```rust,no_run
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::time::Duration;

use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{
    BootMode, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts,
    TenantId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = std::env::var("AGENT")?;
    let client = SandboxClient::connect("nats://127.0.0.1:4222").await?;

    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(&agent),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::from([("GREETING".to_string(), "hello".to_string())]),
        cpu_seconds: NonZeroU32::new(30).ok_or("cpu_seconds must be positive")?,
        memory_mib: NonZeroU32::new(64).ok_or("memory_mib must be positive")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?;

    let request = LeaseRequest {
        tenant: TenantId::new("demo")?,
        provider_pref: vec![ProviderKind::Subprocess],
        spec,
        ttl_seconds: 15,
        idempotency_key: LeaseRequestKey::new("first-lease-1")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: Default::default(),
        tags: Default::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
        operation_id: None,
    };

    println!("requesting a lease on {}", subjects::LEASE_REQUEST);
    let granted = client.lease(&request, Duration::from_secs(10)).await?;
    println!("granted lease {} on {:?}", granted.lease_id, granted.provider);

    println!("waiting on {}", subjects::lease_released(&granted.lease_id));
    let reason = client
        .wait_released_with_reason(&granted.lease_id, Duration::from_secs(60))
        .await?;
    println!("released: {reason:?}");
    Ok(())
}
```

Run it:

```bash
AGENT="$HOME/sandbox-tutorial/agent.sh" cargo run
```

The program prints the lease id, then waits. In the worker terminal you see `lease accepted`, `sandbox launched` and the agent's line:

```text
lease 01M8... for tenant demo says hello
```

About 15 seconds after the grant the program prints `released: Expired` and exits.

### 7. Try an image the tenant may not run

Run the program again with an agent outside the allowed directory, and change the idempotency key first so the broker treats it as a new request:

```bash
sed -i.bak 's/first-lease-1/first-lease-2/' src/main.rs
AGENT=/bin/echo cargo run
```

The broker refuses the lease before any worker sees it:

```text
Error: Denied(InvalidSpec { reason: "image '/bin/echo' not in allow-list for tenant 'demo'" })
```

## What just happened

1. `SandboxClient::lease` published the `LeaseRequest` on `sandbox.lease.request` and waited for the reply.
2. The broker checked the tenant's image allowlist, quota and idempotency key, then picked a live `Subprocess` worker from the heartbeats it had seen.
3. The broker dispatched the lease to that worker. The worker warned that dispatch verification is in warn mode with no public keys and accepted it; [signing worker dispatches](../../how-to/sandbox/sign-worker-dispatches.md) closes that gap.
4. The worker created a workspace under `--workspace-root` and started the agent under bubblewrap or Seatbelt with the lease environment.
5. The agent exited, but the lease stayed active until its TTL. The broker's sweeper then reclaimed it and published `LeaseReleased` with reason `Expired` on `sandbox.lease.released.<lease id>`, which `wait_released_with_reason` was subscribed to.

The broker deduplicates lease requests by tenant and idempotency key, so a retry with the same key never creates a second lease. That is why step 7 changes the key.

## Next steps

- [Publish a signed snapshot](./publish-signed-snapshot.md) of a sandbox workspace.
- [Onboard a tenant](./onboard-tenant.md) with its own bucket, key, API keys and quotas. With authorization on, mint a key with `trogon-sandbox-admin api-key mint --tenant demo --service-account <account> --name <name>` and pass it to `SandboxClient::with_api_key`.
- [Run the whole stack locally](../../how-to/sandbox/run-the-stack-locally.md), or [run the subprocess worker on macOS](../../how-to/sandbox/run-the-subprocess-worker-on-macos.md) in more depth.
- [Isolation runtimes](../../architecture/sandbox/isolation-runtimes.md) explains the other workers you can put behind the same broker, and [subprocess isolation](../../architecture/sandbox/subprocess-isolation.md) explains what bubblewrap and Seatbelt confine.
- The [sandbox client foundation](../../architecture/sandbox-foundation.md) covers the TypeScript client for the same broker.
