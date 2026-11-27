---
title: Sandbox
description: Run agent workloads under a lease. Explore sandbox providers, follow a workspace through its lifecycle and run your first lease.
aside: false
pageClass: wide-page
product:
  name: Sandbox
  tagline: Give an agent a workspace to run code, edit files and share a preview.
  order: 8
  status: in_progress
  icon: box
  color: "#0E7490"
capabilities:
  - icon: terminal
    title: Run commands and services
    text: Read a command's output and exit status, follow logs, or start a named process and reconnect to it. A terminal-capable worker can also open an interactive session.
  - icon: file-diff
    title: Work with files
    text: Read, write and search files, apply an edit that checks the expected text, or transfer a directory with checksum verification. Language-server helpers can query code inside the workspace.
  - icon: globe
    title: Share a running app
    text: Start a server, wait for readiness and request an expiring HTTP preview URL for its port. The gateway signs the URL when the operator enables previews.
  - icon: archive
    title: Pick up where a run left off
    text: Reconnect to a named sandbox, attach a drive or restore a snapshot on a compatible worker. A disk snapshot lets a new run reopen files. Resuming live processes requires a runtime that preserves them.
---

<script setup>
import SandboxLeaseRoute from '../.vitepress/theme/SandboxLeaseRoute.vue'
import SandboxRun from '../.vitepress/theme/SandboxRun.vue'
import SandboxStateMap from '../.vitepress/theme/SandboxStateMap.vue'
import SandboxProviders from '../.vitepress/theme/SandboxProviders.vue'
import SandboxLifecycle from '../.vitepress/theme/SandboxLifecycle.vue'
import SandboxCredentialFlow from '../.vitepress/theme/SandboxCredentialFlow.vue'
import Icon from '../.vitepress/theme/Icon.vue'
</script>

<ProductHeader />

A **sandbox** gives a coding task its own runtime and files. A **lease** gives
the agent time to work there. It can check out a project, run the tests and
hand a reviewer a preview of the running app.

[Run your first sandbox lease](../get-started/sandbox/first-sandbox-lease.md) to
start a local broker and subprocess worker, allow an executable for a tenant,
and watch a Rust client run it through to release.

<nav class="sandbox-sections" aria-label="Explore Sandbox">
  <a href="#providers"><Icon name="boxes" :size="15" />Browse providers</a>
  <a href="#sandbox-protocols"><Icon name="route" :size="15" />Trace a request</a>
  <a href="#take-a-failing-test-from-red-to-green"><Icon name="terminal" :size="15" />Replay a run</a>
  <a href="#how-credentials-reach-an-upstream-service"><Icon name="key-round" :size="15" />Trace a credential</a>
  <a href="#try-the-client-in-memory"><Icon name="code" :size="15" />Try the SDK</a>
  <a href="#build-with-sandbox"><Icon name="boxes" :size="15" />Build with Sandbox</a>
</nav>

## Follow a review workspace {#sandbox-lifecycle}

A reviewer has finished testing a pull request. It can pause the sandbox
until the next comment, stop this run and keep the name for another, or delete
the workspace. Choose an action to trace what happens to its lease.

<SandboxStateMap />

::: details How the workspace reaches Running
Provisioning leaves the named sandbox in `Stopped`. Starting it binds a lease
and moves it to `Starting`. The worker's ready event for that lease moves it
to `Running`. A failed start can recover to `Stopped` or retry with a fresh
lease. [Named sandbox state transitions][entity-transitions]
:::

## Providers

The provider determines what the agent can run. For example, a V8 isolate
can evaluate JavaScript, a Kubernetes worker can start a pod with a terminal,
and a KubeVirt worker can provision a whole VM. A deployment offers the
capabilities of the workers its operator connects.

<SandboxProviders />

[Compare provider requirements and limits](../reference/sandbox-providers.md) · [Check a worker's advertised capabilities](../reference/sandbox/provider-option-matrix.md)

## A lease on the wire {#sandbox-protocols}

A web app calls `Sandbox.create`. An agent calls the `sandbox_lease` tool.
Both need a worker. Pick a route to follow the request to the lease broker
and see what the caller gets back when a worker accepts it.

<SandboxLeaseRoute />

Inspect the [lease request][lease-request] and [grant][lease-grant] fields, or
explore [how MCP connects an agent to its tools](./protocols.md#mcp-tools-and-context).

## Take a failing test from red to green

A test fails at the free-shipping threshold. Play the example to follow the
agent from the first failing command through the fix and cleanup, or select a
stage to inspect the file, command output and process state together.

<SandboxRun />

## What survives the end of a session?

A dropped connection, a paused guest and a deleted sandbox have different
consequences. Compare what happens to the lease, files and processes for each
action. [Lease ownership and completion](../architecture/sandbox-foundation.md#ownership-and-completion)

<SandboxLifecycle />

## What a lease can do

A shell, an HTTP preview and a snapshot each require different worker
capabilities. Include the ones your application depends on when requesting
the lease.

<FeatureGrid :items="$frontmatter.capabilities" />

## How credentials reach an upstream service

Suppose a build needs a private package. The guest sends a placeholder. On a
worker configured for credential substitution, the egress proxy checks the
destination and request against its HTTPS transform rule. Follow an allowed
request to the registry, then switch to a denied request to see where it stops.

<SandboxCredentialFlow />

See the [egress proxy][egress] and [credential broker][credentials] for
supported policies and credential scopes.

## Keep workspace data

For files that need to outlive a lease, the volume library can pull an
object-store prefix into the workspace and write back changed files. A native
snapshot restores through the provider that captured it. Registry-backed
snapshots use OCI artifacts. When a released lease requests a snapshot, the
snapshot builder asks the worker for an export. Its publication pipeline can
scan the artifact, attach a software bill of materials and sign the manifest
before it records the digest.
[Publish a signed snapshot](../get-started/sandbox/publish-signed-snapshot.md) · [Restore a snapshot](../how-to/sandbox/restore-from-snapshot.md) · [Mount remote storage](../how-to/sandbox/mount-remote-storage.md)

## Choose a way in

A deployment needs NATS with JetStream, a configured broker and a worker that
can satisfy the lease. Choose the interface that fits how you work.

| Entry point | How to use it |
| --- | --- |
| A web app or Node service | Use the TypeScript `Sandbox` handle over WebSocket. A trusted backend exchanges the tenant API key for a short-lived session token that the browser can use. [TypeScript SDK][sdk] |
| A Rust service on NATS | Use the typed client to request leases, run commands, stream logs and wait for release. [Rust client][rust-client] |
| An agent that calls MCP tools | Run the stdio MCP server to expose sandbox operations to the agent over its existing tool interface. [MCP server][mcp] |
| A developer at the command line | Use the `sandbox` CLI to work with leases through the WebSocket gateway. [Sandbox CLI][cli] |
| An interactive shell | Connect over the SSH gateway to a worker that supports SSH attachment. [SSH gateway][ssh] |
| An operations dashboard | Inspect the tenant's leases, snapshot inventory, worker heartbeats and tags in the read-only dashboard. Sandbox services can also export OpenTelemetry traces and Prometheus metrics. [Dashboard][dashboard] · [Observability][observability] |
| An operator at the command line | Inspect live worker capabilities, drain a worker, set tenant limits and manage API keys through the operator CLI. [Operator CLI][admin] |

## Try the client in memory

From a package that depends on `@trogonai/sandbox-ws-client` in the TypeScript
workspace, this example creates a lease, writes and reads a file, and releases
the lease even if a file operation fails. The in-memory client models these
operations without starting a guest or contacting a provider.

```ts
import { MemorySandboxClient, Sandbox, tenantId } from '@trogonai/sandbox-ws-client';

const client = new MemorySandboxClient();
const sandbox = await Sandbox.create(client, { tenant: tenantId('demo') });
try {
  await sandbox.writeFiles([{ path: '/work/result.txt', content: 'ready' }]);
  console.log(await sandbox.readFile({ path: '/work/result.txt' })); // ready
} finally {
  await sandbox.delete();
}
```

For real execution, connect a `SandboxWsClient` to your gateway and pass it
to the same `Sandbox.create` call. Use `requiredCapabilities` for features
your workload depends on. The development supervisor can start NATS, a broker,
a worker and the gateway from installed binaries. Select the memory worker to
exercise that route locally with scripted commands. [Gateway connection example][sdk] · [Development supervisor][dev]

## Build with Sandbox

Give an agent tools backed by a single sandbox, or start a web application
that hands a browser a short-lived gateway session.
[Agent tools][agent-tools] · [Hono example][hono] · [Next.js example][nextjs] · [AI app builder example][builder-example]

For a Rust application that only needs to run a local command under filesystem
and network restrictions, the policy execution library works without a broker.
Its command-line front end reads a settings file. [Policy execution library][policy] · [Policy CLI][policy-cli]

One development command can start the broker, gateway and Docker worker on
your machine. For a shared cluster, the Helm chart installs the control plane
and the workers you enable. [Run the local stack](../how-to/sandbox/run-the-stack-locally.md) · [Deploy with Helm](../how-to/sandbox/deploy.md)

[sdk]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-ws-client
[hono]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-example-hono
[nextjs]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-example-nextjs
[builder-example]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-example-ai-app-builder
[cli]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-cli
[agent-tools]: https://github.com/TrogonStack/trogonai/tree/main/tsworkspace/packages/sandbox-agent-tools
[policy]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-policy-exec
[policy-cli]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-policy-cli
[rust-client]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-client
[lease-request]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-contract/src/lease_request.rs
[lease-grant]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-contract/src/lease_granted.rs
[mcp]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-mcp
[ssh]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-ssh-gateway
[dashboard]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-dashboard
[admin]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-admin
[observability]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-observability
[dev]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-dev
[egress]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-egress-proxy
[credentials]: https://github.com/TrogonStack/trogonai/tree/main/rsworkspace/crates/sandbox/trogon-sandbox-credential-broker
[entity-transitions]: https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-protocol/src/entity_transition.rs

<style scoped>
h2[id] { scroll-margin-top: var(--vp-nav-height); }
@media (max-width: 959px) {
  h2[id] { scroll-margin-top: calc(var(--vp-nav-height) + 48px); }
}
.sandbox-sections {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin: 18px 0 0;
}
.sandbox-sections a {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 5px 11px;
  border: 1px solid var(--vp-c-divider);
  border-radius: 7px;
  background: var(--vp-c-bg);
  color: var(--vp-c-text-1);
  font-size: 13px;
  font-weight: 600;
  text-decoration: none;
}
.sandbox-sections a:first-child {
  border-color: #0e6479;
  background: #0e6479;
  color: white;
}
.sandbox-sections a:hover, .sandbox-sections a:focus-visible {
  outline: 2px solid var(--vp-c-brand-1);
  outline-offset: 2px;
}
</style>
