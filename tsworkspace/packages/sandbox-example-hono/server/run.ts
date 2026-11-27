import { Sandbox, SandboxWsError, tenantId } from '@trogonai/sandbox-ws-client';

import { runResultSchema } from './contracts.ts';
import type { RunResult, StreamEvent } from './contracts.ts';
import { sharedClient, tenantName } from './sandbox.ts';

export type { RunResult, StreamEvent } from './contracts.ts';

async function createSandbox(): Promise<Sandbox> {
  return Sandbox.create(await sharedClient(), {
    tenant: tenantId(tenantName()),
    providerPref: ['local_docker'],
    timeoutMs: 60_000,
    networkPolicy: 'deny-all',
  });
}

/** A fresh sandbox per call, released in a `finally` so a failed request does not hold a lease until its timeout. */
export async function runShell(script: string): Promise<RunResult> {
  const sandbox = await createSandbox();
  try {
    const result = await sandbox.runCommand({ cmd: 'sh', args: ['-c', script], timeoutSeconds: 30 });
    return runResultSchema.parse({ exitCode: result.exitCode, stdout: result.stdout(), stderr: result.stderr() });
  } finally {
    await sandbox.delete();
  }
}

/**
 * Runs `script` detached and yields its output as it is produced. A broker that keeps durable command
 * logs streams chunks live; one that does not (the in-memory fake) answers `unsupported`, and the output
 * is then replayed after the command finishes.
 */
export async function* streamShell(script: string): AsyncGenerator<StreamEvent, void, void> {
  const sandbox = await createSandbox();
  try {
    const command = await sandbox.runCommand({
      cmd: 'sh',
      args: ['-c', script],
      timeoutSeconds: 30,
      detached: true,
      streamOutput: true,
    });
    try {
      for await (const chunk of command.logs({ follow: true })) {
        yield { type: 'output', stream: chunk.stream, data: chunk.data };
      }
    } catch (err) {
      if (!(err instanceof SandboxWsError) || err.code !== 'unsupported') {
        throw err;
      }
      for await (const chunk of command.logs()) {
        if (chunk.data !== '') {
          yield { type: 'output', stream: chunk.stream, data: chunk.data };
        }
      }
    }
    yield { type: 'exit', exitCode: (await command.wait()).exitCode };
  } finally {
    await sandbox.delete();
  }
}
