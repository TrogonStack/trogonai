# Expose a sandbox service through a signed preview

Use a gateway preview to reach a service running inside a sandbox. The gateway must have preview signing keys and a public base URL configured. Keep those keys on the server and give the browser only the signed preview URL. The gateway settings are documented in the `rsworkspace/crates/sandbox/trogon-sandbox-ws-gateway` README.

## Start a service and verify its preview

Configure the gateway environment variables and run the host application with a Node.js version that supports top-level `await`. The guest image must contain the command you launch and `bash` or `nc` for the port readiness check.

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const env = z.object({
  SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
  SANDBOX_GATEWAY_WS_URL: z.string().url(),
  SANDBOX_API_KEY: z.string().min(1),
  SANDBOX_TENANT: z.string().min(1),
}).parse(process.env);
const client = await SandboxWsClient.connect({
  gatewayHttpUrl: env.SANDBOX_GATEWAY_HTTP_URL,
  gatewayWsUrl: env.SANDBOX_GATEWAY_WS_URL,
  apiKey: env.SANDBOX_API_KEY,
});
try {
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(env.SANDBOX_TENANT),
    providerPref: ['local_docker'],
    image: {
      kind: 'registry',
      reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
    },
    ports: [3000],
    timeoutMs: 120_000,
  });
  try {
    await sandbox.writeFiles([{
      path: '/tmp/preview-server.mjs',
      content: "import { createServer } from 'node:http';\n" +
        "createServer((_request, response) => response.end('ready')).listen(3000, '0.0.0.0');\n",
    }]);
    const server = await sandbox.startServer({
      cmd: 'node', args: ['/tmp/preview-server.mjs'], port: 3000,
    });
    if (server.url === undefined) throw new Error('The gateway did not provide a preview URL');
    const response = await fetch(server.url);
    if (!response.ok || await response.text() !== 'ready') throw new Error('Preview verification failed');
  } finally { await sandbox.delete(); }
} finally { client.close(); }
```

`startServer` waits for the guest port before resolving the preview. To verify a particular HTTP path during startup, pass `path`; that guest-side check needs `curl` or `wget`. The slim Node.js image above has neither, so it uses the port check and the host's built-in `fetch` instead.

An unnamed startup is cleaned up if readiness or preview setup fails. A named startup can reuse an existing process, so failure preserves that execution. Explicitly stop a named process when that is your intent. For snapshots and client reconnection, see `tsworkspace/packages/sandbox-ws-client/docs/how-to/restore-managed-servers.md`.

## Choose a worker

Port support and exposing an undeclared port are separate capabilities. These are the implemented worker contracts:

| Worker | Preview | Port added after creation |
| --- | --- | --- |
| Docker | Worker-host endpoint through the gateway | Yes, through a guest bridge |
| Linux subprocess with bubblewrap and an isolated network namespace | Worker-host endpoint through the gateway | Yes, through a namespace bridge |
| Daytona | Provider preview with its access headers applied by the gateway | Yes |
| E2B | Provider preview with its access information applied by the gateway | Yes |
| Vercel | Provider endpoint for a declared port | No |
| Fly | Fly Proxy endpoint for a declared port | No |
| Modal | Provider tunnel for a declared port | No |
| Substrate | Worker endpoint for a declared port | No |

The Kubernetes, KubeVirt, Firecracker, Hyperlight, ACA, SPACE and Proxmox workers do not currently provide this preview flow. A subprocess lease on macOS, an unconfined backend or the shared host network also returns `Unsupported`.

Declare `ports` when creating a lease for providers that require it. An undeclared port on such a provider returns `PortNotExposed`; create a new lease with the port included. Docker, Daytona and E2B can expose a later service without creating a new lease. Provider configuration, connectivity and access policy still apply.

Admission uses `needs_port_preview` when the lease declares ports. Worker profiles expose `supports_port_preview` and `supports_expose_after_create`; the latter does not follow automatically from the former.

## Resolve an existing service

`sandbox.domain(port)` returns the worker-reachable endpoint. `sandbox.preview(port, { waitMs })` requests a signed gateway URL and waits for HTTP readiness. The protocol distinguishes:

- `Available`: an endpoint was allocated. URL allocation alone does not establish that the application is serving.
- `NotReady`: the gateway's bounded HTTP wait expired.
- `PortNotExposed`: this port cannot be added to the existing lease.
- `Unsupported`: the provider does not implement the operation.
- `Denied`: authorization, lease lookup or provider execution failed.

Gateway HTTP readiness accepts an HTTP response other than the front-proxy startup statuses 502, 503 and 504. It does not establish that a particular application path works. Probe that path separately, or use the guest-side `startServer({ path })` check. Direct broker clients must perform their own HTTP wait because the gateway owns `wait_ms`.

## Use and renew the URL

A signed URL has this shape:

```text
<gateway>/v1/preview/<lease_id>/<port>/<token>/<application-path>
```

The token binds the tenant, lease, port and expiry. Treat the entire URL as a credential. To construct a child path, use `new URL('settings', preview.url)` so the signed prefix remains intact. An absolute path such as `/settings` discards that prefix.

Calling `preview` again mints a fresh URL without invalidating earlier ones. Renew before `expiresAt`. The gateway lifetime comes from `SANDBOX_PREVIEW_TTL_SECONDS` (default 900 seconds, clamped to the session lifetime). Expired or incorrectly scoped tokens are rejected; deleting the lease also makes its service unavailable.

The gateway streams HTTP request and response bodies. WebSocket upgrades return `501`; run clients that need that protocol inside the sandbox or provide a separate authenticated route.

The gateway applies provider access headers itself. Do not copy Daytona tokens or other provider access credentials into browser code. A bare worker endpoint has no gateway token protection and is intended for networks that can reach the worker or provider directly.

## Configure the gateway

Previews are disabled until the gateway has signing keys:

| Variable | Meaning |
| --- | --- |
| `SANDBOX_PREVIEW_SIGNING_KEYS` | `kid=hexkey[,kid=hexkey...]`, each key at least 32 bytes. Unset disables previews and the route answers 404. |
| `SANDBOX_PREVIEW_ACTIVE_KEY_ID` | The key that signs new tokens. Defaults to the first listed. Keep the previous key listed while old tokens drain. |
| `SANDBOX_PREVIEW_PUBLIC_URL` | Externally reachable base URL, also available as `--preview-public-url`. Required with keys. |
| `SANDBOX_PREVIEW_TTL_SECONDS` | Token lifetime, default 900. |

## Docker and subprocess bridges

Docker binds declared ports to OS-assigned ports on the worker's loopback interface. For an undeclared port, a lease using `network_mode: none`, or a restored lease without bindings, it creates a forward through the container. The image needs `socat`, `nc` or `bash` for this bridge. Repeated requests reuse the forward; releases close it. Each lease has a bounded number of forwards.

Linux bubblewrap leases with isolated networking use a helper that joins the lease's own user and network namespaces. The helper reaches that lease's process tree; a terminal launched into a separate namespace does not share its server. The worker's seccomp policy must allow the namespace operations its isolation backend uses.

An endpoint on `127.0.0.1` is reachable from the worker host. Run the gateway where it can reach that address, or provide the necessary network path. Do not expose a bare loopback endpoint as a public URL.

## Keep Fly previews private

Fly's instance-routing header selects the Machine but does not authenticate the requester. With public Fly Proxy networking, the provider endpoint can be publicly reachable independently of the signed gateway URL.

For private access, remove the app's public addresses, allocate a Flycast address and configure the worker with `TROGON_SANDBOX_FLY_PREVIEW_NETWORK=flycast` (the `--preview-network` flag; the default is `public`). Run the gateway inside the organization's private network. See [deployment guide](./deploy.md) for the surrounding worker and gateway setup.

## Troubleshoot a preview

For `Unsupported`, choose a worker with preview support. Declaring a port cannot add an operation the worker lacks.

For `PortNotExposed`, check the declaration and the worker's expose-after-create contract. A listening process alone does not add a provider ingress binding.

For connection refusal, verify the service inside the guest and then the gateway's route to the worker or provider. For Docker bridges, check that the image includes a supported bridge tool. For provider previews, keep required access headers on the gateway.

For an unavailable readiness check, inspect the underlying command or gateway error before starting another server. A transport timeout does not prove the existing process exited.
