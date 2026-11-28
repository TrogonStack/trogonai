---
name: sandbox-ws-client
description: Run code in an isolated sandbox with the TypeScript client for the sandbox gateway. Use when asked to lease a sandbox, run commands or move files in one, check tenant limits and usage, or wire the client into a backend or browser app.
allowed-tools: Read Grep Glob Bash Write Edit
---

# sandbox-ws-client

Client package: `tsworkspace/packages/sandbox-ws-client`. Read its `llms.txt` for the full reference and `docs/hono.md` or `docs/nextjs.md` for backend wiring. This skill is the short path.

## Rules

- The API key stays on the server. Browsers use a token from `POST {gatewayHttpUrl}/v1/sessions` (header `X-Sandbox-API-Key`) with `SandboxWsClient.connectWithToken`.
- Always release the lease in a `finally`. An undeleted lease runs until its timeout.
- Pass `providerPref` (for example `['local_docker']`). An empty list is denied.
- The default image is alpine with no git or compiler. For coding work use the image from `docs/how-to/sandbox/build-an-agent-image.md`.
- A non-zero exit code is a result. Thrown errors extend `SandboxWsError`; retry only when `retryable`, and when `outcomeUnknown` reuse the same `idempotencyKey`.

## Flow

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
  gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
  apiKey: process.env.SANDBOX_API_KEY!,
});
try {
  const limits = await client.limits();
  if (limits.active_leases >= limits.max_active_leases) throw new Error('no lease capacity');

  const sandbox = await Sandbox.create(client, {
    tenant: tenantId('acme'),
    providerPref: ['local_docker'],
    timeoutMs: 300_000,
  });
  try {
    await sandbox.writeFiles([{ path: '/work/hello.sh', content: 'echo hi\n' }]);
    const run = await sandbox.runCommand('sh', ['/work/hello.sh']);
    console.log(run.exitCode, run.stdout());
    console.log(await sandbox.readFile({ path: '/work/hello.sh' }));
    console.log(await sandbox.usage());
  } finally {
    await sandbox.delete();
  }
} finally {
  client.close();
}
```

## Tasks

- Long job: `runCommand({ cmd, args, detached: true })` then `command.wait()`, `command.kill()`.
- Keep state between runs: give the lease a `name`, `sandbox.stop()`, later `Sandbox.get(client, { name })`. Use `persistent: false` to keep no disk state.
- Capacity and spend: `client.limits()` for caps and consumption, `client.tenantUsage()` for live usage, `sandbox.usage()` for one lease.
- Tests without a gateway: `MemorySandboxClient`.

## Check

Run `pnpm typecheck` and `pnpm test` in `tsworkspace/packages/sandbox-ws-client` after changing client code.
