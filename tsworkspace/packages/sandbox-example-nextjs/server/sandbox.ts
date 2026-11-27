import { MemorySandboxClient, SandboxWsClient } from '@trogonai/sandbox-ws-client';
import type { SandboxClientApi } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const providerConfiguration = z.object({
  SANDBOX_PROVIDER: z.enum(['local_docker', 'memory']).default('local_docker'),
});

export function isMemoryMode(): boolean {
  return providerConfiguration.parse({ SANDBOX_PROVIDER: process.env.SANDBOX_PROVIDER }).SANDBOX_PROVIDER === 'memory';
}

export function requiredEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value === '') {
    throw new Error(`${name} is not set. Copy .env.example to .env.local, or set SANDBOX_PROVIDER=memory.`);
  }
  return value;
}

export function tenantName(): string {
  return isMemoryMode() ? 'memory' : requiredEnv('SANDBOX_TENANT');
}

/** Memory mode has no shell, so a `sh -c` command is acknowledged and not run. `exit N` sets the exit code. */
function memoryClient(): MemorySandboxClient {
  return new MemorySandboxClient({
    commands: {
      sh: ({ args }) => {
        const script = args.slice(1).join(' ');
        const exit = /^exit (\d+)$/.exec(script);
        return exit === null
          ? { exitCode: 0, stdout: `memory mode, not executed: ${script}\n` }
          : { exitCode: Number(exit[1]), stderr: `memory mode, exiting with ${exit[1]}\n` };
      },
    },
  });
}

interface Cache {
  client: Promise<SandboxClientApi> | undefined;
}

// Kept on globalThis so `next dev` hot reloads and separate route bundles share one connection.
const cache = ((globalThis as { __nextjsExample?: Cache }).__nextjsExample ??= { client: undefined });

/** One connection per server process. The API key is read on the server only. */
export function sharedClient(): Promise<SandboxClientApi> {
  const memory = isMemoryMode();
  const pending =
    cache.client ??
    (memory
      ? Promise.resolve<SandboxClientApi>(memoryClient())
      : SandboxWsClient.connect({
          gatewayHttpUrl: requiredEnv('SANDBOX_GATEWAY_HTTP_URL'),
          gatewayWsUrl: requiredEnv('SANDBOX_GATEWAY_WS_URL'),
          apiKey: requiredEnv('SANDBOX_API_KEY'),
        }));
  cache.client = pending;
  pending.catch(() => {
    cache.client = undefined;
  });
  return pending;
}
