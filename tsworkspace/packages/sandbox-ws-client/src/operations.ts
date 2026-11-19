import { z } from 'zod';

import type { SandboxClientApi } from './client-api.ts';
import { OperationCancellationError, SandboxWsError } from './errors.ts';
import type { TenantId } from './ids.ts';
import { providerKindSchema } from './wire.ts';

export const operationIdSchema = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9_-]+$/);
export const operationStateSchema = z.enum(['queued', 'running', 'succeeded', 'failed', 'canceled', 'expired']);
export const operationCompensationStateSchema = z.enum(['pending', 'cleaned']);
export type OperationCompensationState = z.infer<typeof operationCompensationStateSchema>;
export const operationCapabilitiesSchema = z.strictObject({ supports_create_cancellation: z.boolean() });
export type OperationCapabilities = z.infer<typeof operationCapabilitiesSchema>;
export const cancelOperationResponseSchema = z.discriminatedUnion('outcome', [
  z.strictObject({ outcome: z.literal('canceled') }),
  z.strictObject({ outcome: z.literal('pending') }),
  z.strictObject({ outcome: z.literal('already_completed'), state: operationStateSchema }),
  z.strictObject({ outcome: z.literal('unsupported') }),
]);
export type CancelOperationResponse = z.infer<typeof cancelOperationResponseSchema>;
export const cancelOperationOptionsSchema = z.strictObject({ compensateCreated: z.boolean().optional() });
export type CancelOperationOptions = z.input<typeof cancelOperationOptionsSchema>;

export function parseCancelOperationOptions(value: unknown): CancelOperationOptions {
  const parsed = cancelOperationOptionsSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid operation cancellation options');
  return parsed.data;
}
export const operationGrantedResultSchema = z.object({
  result: z.literal('granted'),
  lease_id: z.string().min(1),
  provider: providerKindSchema,
  sandbox_id: z.string().min(1),
  evidence_subject: z.string().min(1),
  deadline: z.object({
    secs_since_epoch: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
    nanos_since_epoch: z.number().int().min(0).max(999_999_999),
  }),
  created: z.boolean().optional(),
  capabilities: z.array(z.string()).optional(),
  operation_id: operationIdSchema.optional(),
});
export const operationStatusSchema = z.object({
  id: operationIdSchema,
  tenant: z.string().min(1),
  kind: z.enum([
    'create',
    'restore',
    'fork',
    'snapshot',
    'pause',
    'resume',
    'suspend',
    'wake',
    'stop',
    'delete',
    'rollback',
  ]),
  target: z.string().min(1).optional(),
  state: operationStateSchema,
  compensation: operationCompensationStateSchema.optional(),
  result: z
    .union([
      operationGrantedResultSchema,
      z.object({
        result: z.enum([
          'snapshot',
          'paused',
          'resumed',
          'suspended',
          'woken',
          'stopped',
          'deleted',
          'rolled_back',
          'unsupported',
        ]),
      }),
    ])
    .optional(),
});
export type OperationStatus = z.infer<typeof operationStatusSchema>;
export const getOperationResponseSchema = z.discriminatedUnion('outcome', [
  z.strictObject({ outcome: z.literal('found'), operation: operationStatusSchema }),
  z.strictObject({ outcome: z.literal('not_found') }),
]);
export type GetOperationResponse = z.infer<typeof getOperationResponseSchema>;

export function parseOperationId(value: unknown): string {
  const parsed = operationIdSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid operation id');
  return parsed.data;
}

export function parseOperationResponse<S extends z.ZodType>(schema: S, value: unknown, mutating = false): z.output<S> {
  const parsed = schema.safeParse(value);
  if (!parsed.success)
    throw new SandboxWsError('invalid_response', 'invalid operation control response', { outcomeUnknown: mutating });
  return parsed.data;
}

export async function cancelCreateOperation(
  client: SandboxClientApi,
  operationId: string,
  tenant: TenantId,
  reason: unknown,
  timeoutMs: number,
): Promise<never> {
  let expired = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const unknown = () => new OperationCancellationError(operationId, false, true, reason);
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      expired = true;
      reject(unknown());
    }, timeoutMs);
  });
  const work = (async (): Promise<OperationCancellationError> => {
    while (!expired) {
      const cancellation = parseOperationResponse(
        cancelOperationResponseSchema,
        await client.cancelOperation(operationId, { compensateCreated: true }),
        true,
      );
      if (expired) return unknown();
      if (cancellation.outcome === 'canceled') return new OperationCancellationError(operationId, true, false, reason);
      if (cancellation.outcome === 'unsupported') return unknown();
      if (cancellation.outcome === 'already_completed') {
        const response = parseOperationResponse(getOperationResponseSchema, await client.getOperation(operationId));
        if (expired || response.outcome !== 'found') return unknown();
        const operation = response.operation;
        if (
          operation.id !== operationId ||
          operation.tenant !== tenant ||
          operation.state !== cancellation.state ||
          !['create', 'restore'].includes(operation.kind)
        )
          return unknown();
        if (operation.state === 'canceled') return new OperationCancellationError(operationId, true, false, reason);
        if (operation.state === 'succeeded') {
          const grant = operation.result;
          if (
            grant?.result !== 'granted' ||
            (grant.operation_id !== undefined && grant.operation_id !== operationId) ||
            operation.target !== grant.lease_id
          )
            return unknown();
          return new OperationCancellationError(operationId, false, false, reason, grant.lease_id);
        }
        if (operation.state === 'failed' || operation.state === 'expired') {
          return new OperationCancellationError(operationId, false, false, reason, operation.target);
        }
        return unknown();
      }
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    return unknown();
  })();
  try {
    const result = await Promise.race([work, timeout]);
    throw result;
  } catch (error) {
    throw error instanceof OperationCancellationError ? error : unknown();
  } finally {
    expired = true;
    clearTimeout(timer);
  }
}
