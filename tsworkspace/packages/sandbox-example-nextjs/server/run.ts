import { Sandbox, SandboxWsError, tenantId } from '@trogonai/sandbox-ws-client';

import { runResultSchema } from './contracts.ts';
import type { RunResult } from './contracts.ts';
import { sharedClient, tenantName } from './sandbox.ts';

export type { RunResult } from './contracts.ts';

/** A fresh sandbox per call, released in a `finally` so a failed run does not hold a lease until its timeout. */
export async function runInSandbox(script: string): Promise<RunResult> {
  const client = await sharedClient();
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(tenantName()),
    providerPref: ['local_docker'],
    timeoutMs: 60_000,
    networkPolicy: 'deny-all',
  });
  try {
    const result = await sandbox.runCommand({ cmd: 'sh', args: ['-c', script], timeoutSeconds: 30 });
    return runResultSchema.parse({ exitCode: result.exitCode, stdout: result.stdout(), stderr: result.stderr() });
  } finally {
    await sandbox.delete();
  }
}

/** Maps a sandbox error to a result the caller can show; anything else is a bug and rethrows. */
export function resultFromError(err: unknown): { result: RunResult; retryable: boolean } {
  if (err instanceof SandboxWsError) {
    return { result: runResultSchema.parse({ exitCode: -1, stdout: '', stderr: err.hint }), retryable: err.retryable };
  }
  throw err;
}
