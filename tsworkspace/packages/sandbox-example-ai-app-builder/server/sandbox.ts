import { MemorySandboxClient, Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import type { SandboxClientApi } from '@trogonai/sandbox-ws-client';
import type { ToolSet } from 'ai';
import { z } from 'zod';

/** The port the agent is told to serve the generated app on, and the one the preview shows. */
export const APP_PORT = 3000;

export const WORKDIR = '/tmp/workspace';

export interface SessionSandbox {
  sandbox: Sandbox;
  /** Set only with a real gateway; it enables the stack's native file verbs. */
  client: SandboxWsClient | undefined;
  tools?: ToolSet;
}

const providerConfiguration = z.object({
  SANDBOX_PROVIDER: z.enum(['local_docker', 'memory']).default('local_docker'),
});

export function isMemoryMode(): boolean {
  return providerConfiguration.parse({ SANDBOX_PROVIDER: process.env.SANDBOX_PROVIDER }).SANDBOX_PROVIDER === 'memory';
}

function requiredEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set. Copy .env.example to .env.local, or set SANDBOX_PROVIDER=memory.`);
  }
  return value;
}

/** Memory mode has no shell, so a `sh -c` command is acknowledged and not run. */
function memoryClient(): MemorySandboxClient {
  return new MemorySandboxClient({
    commands: {
      sh: ({ args }) => ({ exitCode: 0, stdout: `memory mode, not executed: ${args.slice(1).join(' ')}\n` }),
    },
  });
}

interface Cache {
  client: Promise<{ api: SandboxClientApi; ws: SandboxWsClient | undefined }> | undefined;
  sandboxes: Map<string, Promise<SessionSandbox>>;
}

// Kept on globalThis so `next dev` hot reloads reuse one connection and one sandbox per session.
const cache = ((globalThis as { __appBuilder?: Cache }).__appBuilder ??= { client: undefined, sandboxes: new Map() });

function sharedClient(): Promise<{ api: SandboxClientApi; ws: SandboxWsClient | undefined }> {
  cache.client ??= (async () => {
    if (isMemoryMode()) {
      return { api: memoryClient(), ws: undefined };
    }
    // The API key is read on the server only and never reaches the browser.
    const ws = await SandboxWsClient.connect({
      gatewayHttpUrl: requiredEnv('SANDBOX_GATEWAY_HTTP_URL'),
      gatewayWsUrl: requiredEnv('SANDBOX_GATEWAY_WS_URL'),
      apiKey: requiredEnv('SANDBOX_API_KEY'),
    });
    return { api: ws, ws };
  })();
  cache.client.catch(() => {
    cache.client = undefined;
  });
  return cache.client;
}

/**
 * One sandbox per browser session. The name is a persistent lease name, so after a server
 * restart `Sandbox.create` with the same name reattaches to the lease (`created` is false)
 * instead of starting a new one. The in-process map only saves the round trip.
 */
export function sandboxForSession(sessionId: string): Promise<SessionSandbox> {
  let pending = cache.sandboxes.get(sessionId);
  if (pending === undefined) {
    pending = (async () => {
      const memory = isMemoryMode();
      const { api, ws } = await sharedClient();
      const sandbox = await Sandbox.create(api, {
        tenant: tenantId(memory ? 'memory' : requiredEnv('SANDBOX_TENANT')),
        providerPref: [memory ? 'mock' : 'local_docker'],
        name: `app-builder-${sessionId}`,
        persistent: true,
        image: {
          kind: 'registry',
          reference: { registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' },
        },
        ports: [APP_PORT],
        timeoutMs: 60 * 60 * 1000,
      });
      if (sandbox.created) {
        await sandbox.mkDir(WORKDIR);
      }
      return { sandbox, client: ws };
    })();
    cache.sandboxes.set(sessionId, pending);
    pending.catch(() => cache.sandboxes.delete(sessionId));
  }
  return pending;
}
