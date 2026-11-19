import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

const sandboxes: Sandbox[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const sandbox of sandboxes.splice(0)) await sandbox.delete();
  for (const end of endings.splice(0)) end();
});

describe('snapshot creation signal lifetime', () => {
  it.each([undefined, 'web'])(
    'preserves a returned restored server after creation has completed (name=%s)',
    async (name) => {
      const client = new MemorySandboxClient({
        commands: {
          serve: ({ signal }) =>
            new Promise((resolve) => {
              const end = () => resolve({ exitCode: 143 });
              endings.push(end);
              signal.addEventListener('abort', end, { once: true });
            }),
        },
      });
      client.waitSandboxReady = async () => ({ outcome: 'ready' });
      client.operationCapabilities = async () => ({ supports_create_cancellation: true });
      const tenant = tenantId('acme');
      const source = await Sandbox.create(client, { tenant });
      sandboxes.push(source);
      await source.startServer({ cmd: 'serve', name, port: 3000, preview: false });
      const receipt = JSON.parse(JSON.stringify(await source.snapshot()));
      const controller = new AbortController();
      const restored = await Sandbox.fromSnapshot(client, receipt, { tenant, signal: controller.signal });
      sandboxes.push(restored);
      const execution = restored.exportClientState().servers[0]!.executionId;
      controller.abort(new Error('creation is already complete'));
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(await client.pollCommand(restored.sandboxId, execution)).toEqual({ outcome: 'running' });
    },
  );
});
