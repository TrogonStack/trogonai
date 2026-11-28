# Run a headless browser in a sandbox

Request `browser: true` with an explicit browser image through the TypeScript SDK. A Docker worker can serve that request after its operator configures and verifies the same image. The default Docker profile and the other providers do not advertise browser support.

## Build and configure the worker image

Use the supplied browser recipe at `devops/docker/images/sandbox-browser/Dockerfile`, which includes Chromium, fonts, Node 22.7 or newer and pnpm. Build or load it on the Docker daemon the worker uses:

```bash
docker build --tag registry.example.com/sandbox/browser:node22 \
  devops/docker/images/sandbox-browser
export SANDBOX_BROWSER_IMAGE=registry.example.com/sandbox/browser:node22
```

Pass `--browser-image registry.example.com/sandbox/browser:node22` instead of the environment variable if you configure `trogon-sandbox-docker-worker` through flags. Keep the worker's normal NATS, state-directory and egress-proxy configuration from the [deployment guide](./deploy.md).

The image must already be present on that daemon. At startup the worker resolves its immutable Docker image ID, then runs an offline Chromium screenshot and a Node version check as UID 65532 with `HOME=/tmp`. The probe has bounded resources and removes its container and temporary files. An invalid image or a failed probe stops startup; setting the flag alone never earns `supports_browser`.

The worker advertises the capability only after that proof. Browser-required requests must name the same structured registry reference, including its tag or digest. Requests naming an unspecified, different or snapshot image are refused before credential or image work. Matching requests launch the positively verified image ID, so moving the tag does not change what an already-running worker serves. Load the replacement and restart the worker to verify and adopt new content.

## Capture a screenshot

Use Node 22.7 or newer and pnpm, with the `@trogonai/sandbox-ws-client` package built. The image below matches the operator configuration above:

```ts
import { writeFile } from 'node:fs/promises';
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
    browser: true,
    memoryMib: 1024,
    vcpus: 1,
    networkPolicy: 'deny-all',
    image: {
      kind: 'registry',
      reference: { registry: 'registry.example.com', repository: 'sandbox/browser', tag: 'node22' },
    },
  });
  try {
    await sandbox.mkDir('/tmp/browser-demo');
    const shot = await sandbox.runCommand({
      cmd: 'chromium',
      args: [
        '--headless=new', '--disable-gpu', '--no-sandbox', '--no-first-run',
        '--user-data-dir=/tmp/browser-demo/profile',
        '--screenshot=/tmp/browser-demo/page.png', '--window-size=1280,800',
        'data:text/html,<title>Browser demo</title><h1>Hello from Chromium</h1>',
      ],
      timeoutSeconds: 15,
    });
    const result = z.object({ exitCode: z.number().int(), timedOut: z.boolean(), stderr: z.string() })
      .parse({ exitCode: shot.exitCode, timedOut: shot.timedOut, stderr: shot.stderr() });
    if (result.exitCode !== 0 || result.timedOut) throw new Error(result.stderr || 'screenshot failed');
    const png = await sandbox.readFileToBuffer({ path: '/tmp/browser-demo/page.png' });
    if (png === null || png.length === 0) throw new Error('screenshot is missing');
    await writeFile('page.png', png);
  } finally { await sandbox.delete(); }
} finally { client.close(); }
```

This example is offline. To visit remote sites, supply a network policy allowing the intended hosts. The browser remains subject to the sandbox's egress policy.

## Resource and isolation contract

| Setting | Browser lease contract |
| --- | --- |
| Memory | 512 to 4096 MiB, also subject to tenant quotas. |
| CPU | 1000 to 4000 millicores, also subject to worker capacity. |
| Processes | 128 to 512; the browser ceiling applies even when the general worker ceiling is higher. |
| Shared memory | 128 to 512 MiB; the ordinary default is 256 MiB. |
| User and home | UID 65532 with writable `HOME=/tmp`; incompatible explicit overrides are refused. |
| Privileges | No new privileges, no added capabilities, no swap beyond the memory limit. |

Chromium uses `--no-sandbox` because the container runtime supplies the isolation boundary. Do not grant elevated container privileges to enable Chromium's own sandbox. Compatible process and shared-memory overrides can use the worker's ordinary container or native options; values outside the browser bounds are refused.

Run Playwright or Puppeteer inside the lease using the image's Chromium binary, without downloading another browser. Remote DevTools requires a separately secured WebSocket route: the signed [HTTP preview](./expose-sandbox-ports.md) returns `501` for WebSocket upgrades.

## Verify the local flow

With a local gateway, broker and the configured Docker worker running, export the gateway and tenant variables from the screenshot example and describe the same image for the test:

```bash
export SANDBOX_BROWSER_IMAGE_JSON='{"registry":"registry.example.com","repository":"sandbox/browser","tag":"node22"}'
pnpm --dir tsworkspace/packages/sandbox-ws-client build
node --test tsworkspace/packages/sandbox-ws-client/tests/gateway/browser.gateway.ts
```

The test requires the real lease to report the browser feature, captures an offline PNG, checks its dimensions and hash, refuses an incompatible image and removes every remaining observed lease. Missing configuration fails the test. It is opt-in and never runs in CI.

## Related

- [Provider option matrix](../../reference/sandbox/provider-option-matrix.md) for provider image and ingress boundaries.
