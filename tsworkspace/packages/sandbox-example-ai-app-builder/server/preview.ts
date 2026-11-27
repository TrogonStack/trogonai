import type { Sandbox } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

import { previewStateSchema } from '../contracts.ts';
import type { PreviewState } from '../contracts.ts';
import { APP_PORT } from './sandbox.ts';

export const HTTP_CHECK_SCRIPT = [
  `const response = await fetch('http://127.0.0.1:${APP_PORT}', { signal: AbortSignal.timeout(2000) });`,
  'if (!response.ok) throw new Error(`HTTP ${response.status}`);',
  'process.stdout.write(await response.text());',
].join('\n');

const readinessScript = [
  'try {',
  HTTP_CHECK_SCRIPT,
  '} catch (error) {',
  'process.stdout.write(error?.cause?.code ?? error?.code ?? String(error));',
  'process.exitCode = 1;',
  '}',
].join('\n');

const probeResultSchema = z.object({
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
  timedOut: z.boolean(),
});

export async function previewForSandbox(sandbox: Sandbox, memory: boolean): Promise<PreviewState> {
  if (memory) {
    return previewStateSchema.parse({
      status: 'unavailable',
      reason: 'Memory mode runs no real server, so there is nothing to preview.',
    });
  }
  try {
    const command = await sandbox.runCommand({
      cmd: 'node',
      args: ['--input-type=module', '-e', readinessScript],
      timeoutSeconds: 5,
    });
    const result = probeResultSchema.parse({
      exitCode: command.exitCode,
      stdout: command.stdout(),
      stderr: command.stderr(),
      timedOut: command.timedOut,
    });
    if (!result.timedOut && result.exitCode === 1 && result.stdout.trim() === 'ECONNREFUSED') {
      return previewStateSchema.parse({ status: 'not_ready' });
    }
    if (result.timedOut || result.exitCode !== 0) {
      return previewStateSchema.parse({
        status: 'unavailable',
        reason: 'The guest HTTP readiness check failed. No additional server was started.',
      });
    }
    const { url } = await sandbox.preview(APP_PORT, { waitMs: 2000 });
    return previewStateSchema.parse({ status: 'ready', url });
  } catch {
    return previewStateSchema.parse({
      status: 'unavailable',
      reason: 'The preview could not be verified. Retry after checking the gateway and guest service.',
    });
  }
}
