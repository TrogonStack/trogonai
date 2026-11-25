import { z } from 'zod';

import type { SandboxClientApi } from './client-api.ts';
import { CommandCancellationError, SandboxWsError } from './errors.ts';
import type { LeaseId } from './ids.ts';

export const commandLaunchKeySchema = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9_-]+$/);
const executionIdSchema = z.string().min(1).max(128);
export const cancelCommandLaunchRequestSchema = z.strictObject({
  lease_id: z.string().min(1),
  launch_key: commandLaunchKeySchema,
});
export const cancelCommandLaunchResponseSchema = z.discriminatedUnion('outcome', [
  z.strictObject({ outcome: z.literal('canceled'), execution_id: executionIdSchema.optional() }),
  z.strictObject({ outcome: z.literal('preserved'), execution_id: executionIdSchema }),
  z.strictObject({ outcome: z.literal('pending'), message: z.string().max(4096) }),
  z.strictObject({ outcome: z.literal('unsupported') }),
]);
export type CancelCommandLaunchResponse = z.infer<typeof cancelCommandLaunchResponseSchema>;

export function parseCancelCommandLaunchRequest(value: unknown): z.infer<typeof cancelCommandLaunchRequestSchema> {
  const parsed = cancelCommandLaunchRequestSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid command launch cancellation request');
  return parsed.data;
}

export function parseCancelCommandLaunchResponse(value: unknown): CancelCommandLaunchResponse {
  const parsed = cancelCommandLaunchResponseSchema.safeParse(value);
  if (!parsed.success)
    throw new SandboxWsError('invalid_response', 'invalid command launch cancellation response', {
      outcomeUnknown: true,
    });
  return parsed.data;
}

export function requireCommandLaunchCancellation(value: unknown, detached: unknown): boolean {
  const parsed = z.boolean().optional().safeParse(value);
  if (!parsed.success || (parsed.data === true && detached !== true)) {
    throw new SandboxWsError(
      'invalid_request',
      'launch cancellation requires a detached command and a boolean protocol requirement',
    );
  }
  return parsed.data ?? false;
}

export function retainCommandLaunchAuthority(error: unknown, leaseId: LeaseId, launchKey: string): unknown {
  try {
    if (error instanceof Error && Object.isExtensible(error)) {
      const descriptor = Object.getOwnPropertyDescriptor(error, 'launchKey');
      if (descriptor !== undefined && 'value' in descriptor && descriptor.value === launchKey) return error;
      if (
        descriptor === undefined ||
        ('value' in descriptor && descriptor.value === undefined && descriptor.writable)
      ) {
        Object.defineProperty(error, 'launchKey', { value: launchKey, enumerable: true });
        return error;
      }
    }
  } catch {}
  const typed = error instanceof SandboxWsError ? error : undefined;
  return new SandboxWsError(
    typed?.code ?? 'transport',
    error instanceof Error ? error.message : 'the command start outcome is unknown',
    {
      retryable: typed?.retryable ?? false,
      outcomeUnknown: typed?.outcomeUnknown ?? true,
      hint: typed?.hint,
      variant: typed?.variant,
      provider: typed?.provider,
      leaseId: typed?.leaseId ?? leaseId,
      executionId: typed?.executionId,
      restoreCleanup: typed?.restoreCleanup,
      retryAfterMs: typed?.retryAfterMs,
      launchKey,
      cause: error,
    },
  );
}

export async function cancelPendingCommandLaunch(
  client: SandboxClientApi,
  leaseId: LeaseId,
  launchKey: string,
  reason: unknown,
  timeoutMs: number,
): Promise<never> {
  let expired = false;
  const deadline = Date.now() + timeoutMs;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const incomplete = () => new CommandCancellationError(leaseId, undefined, reason, true, { launchKey });
  const timeout = new Promise<CommandCancellationError>((resolve) => {
    timer = setTimeout(() => {
      expired = true;
      resolve(incomplete());
    }, timeoutMs);
  });
  const settling = (async (): Promise<CommandCancellationError> => {
    while (!expired) {
      let response: CancelCommandLaunchResponse;
      try {
        response = parseCancelCommandLaunchResponse(await client.cancelCommandLaunch(leaseId, launchKey));
      } catch (error) {
        if (expired) return incomplete();
        if (error instanceof SandboxWsError && !error.retryable) return incomplete();
        const delay = error instanceof SandboxWsError ? Math.max(10, error.retryAfterMs ?? 0) : 10;
        if (!Number.isFinite(delay) || delay >= deadline - Date.now()) return incomplete();
        await new Promise((resolve) => setTimeout(resolve, delay));
        continue;
      }
      if (expired) return incomplete();
      if (response.outcome === 'canceled') {
        return new CommandCancellationError(leaseId, response.execution_id, reason, false, { launchKey });
      }
      if (response.outcome === 'preserved') {
        return new CommandCancellationError(leaseId, response.execution_id, reason, false, {
          launchKey,
          preserved: true,
        });
      }
      if (response.outcome === 'unsupported') return incomplete();
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    return incomplete();
  })();
  let failure: CommandCancellationError;
  try {
    failure = await Promise.race([settling, timeout]);
  } finally {
    expired = true;
    clearTimeout(timer);
  }
  throw failure;
}
