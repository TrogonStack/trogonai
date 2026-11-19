import { z } from 'zod';

import type { SandboxClientApi } from './client-api.ts';
import { CommandCancellationError, SandboxWsError } from './errors.ts';
import type { LeaseId } from './ids.ts';
import type { CommandOutput, RunCommandResponse } from './wire.ts';
import { commandLaunchOriginSchema } from './wire.ts';

export const DEFAULT_CANCELLATION_TIMEOUT_MS = 10_000;
const timeoutSchema = z.number().int().min(1).max(60_000);
const outputShape = {
  exit_code: z.number().int(),
  stdout: z.array(z.number().int().min(0).max(255)),
  stderr: z.array(z.number().int().min(0).max(255)),
  timed_out: z.boolean().optional(),
  truncated: z.boolean().optional(),
};
const completedSchema = z.object({ outcome: z.literal('completed'), ...outputShape });
const runSchema = z.discriminatedUnion('outcome', [
  completedSchema,
  z.object({
    outcome: z.literal('started'),
    execution_id: z.string().min(1).max(128),
    origin: commandLaunchOriginSchema.default('unknown'),
  }),
]);
const pollSchema = z.discriminatedUnion('outcome', [
  completedSchema,
  z.object({ outcome: z.literal('running') }),
  z.object({ outcome: z.literal('not_found') }),
]);
const killSchema = z.object({ outcome: z.enum(['signaled', 'already_completed', 'not_found']) });

export function cancellationTimeout(value: unknown): number {
  const parsed = timeoutSchema.safeParse(value === undefined ? DEFAULT_CANCELLATION_TIMEOUT_MS : value);
  if (!parsed.success)
    throw new SandboxWsError('invalid_request', 'cancellation timeout must be an integer from 1 to 60000 ms');
  return parsed.data;
}

export function cancellationRunResponse(value: unknown): RunCommandResponse {
  const parsed = runSchema.safeParse(value);
  if (!parsed.success)
    throw new SandboxWsError('invalid_response', 'invalid cancellable command response', { outcomeUnknown: true });
  return parsed.data;
}

export async function cancelOwnedCommand(
  client: SandboxClientApi,
  leaseId: LeaseId,
  executionId: string,
  timeoutMs: number,
  reason: unknown,
  launchKey?: string,
): Promise<CommandOutput> {
  let expired = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unknown = () => new CommandCancellationError(leaseId, executionId, reason, true, { launchKey });
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      expired = true;
      reject(unknown());
    }, timeoutMs);
  });
  const work = (async () => {
    let killFailed = false;
    try {
      killSchema.parse(await client.killCommand(leaseId, executionId, 'kill'));
    } catch {
      killFailed = true;
    }
    while (!expired) {
      const state = pollSchema.parse(await client.pollCommand(leaseId, executionId));
      if (expired) throw unknown();
      if (state.outcome === 'completed') return state;
      if (state.outcome === 'not_found' || killFailed) throw unknown();
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    throw unknown();
  })();
  try {
    return await Promise.race([work, timeout]);
  } catch {
    throw unknown();
  } finally {
    expired = true;
    clearTimeout(timer);
  }
}
