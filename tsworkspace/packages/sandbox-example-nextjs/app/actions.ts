'use server';

import { runResultSchema, scriptInputSchema } from '../server/contracts.ts';
import { resultFromError, runInSandbox } from '../server/run.ts';
import type { RunResult } from '../server/run.ts';

export async function runScript(formData: FormData): Promise<RunResult> {
  const parsed = scriptInputSchema.safeParse({ script: formData.get('script') });
  if (!parsed.success) {
    return runResultSchema.parse({ exitCode: -1, stdout: '', stderr: 'script must be a non-empty string' });
  }
  try {
    return await runInSandbox(parsed.data.script);
  } catch (err) {
    return resultFromError(err).result;
  }
}
