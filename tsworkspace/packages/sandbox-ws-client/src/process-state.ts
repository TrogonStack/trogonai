import { z } from 'zod';

import { SandboxWsError } from './errors.ts';
import { redactValuesSchema } from './redact.ts';

export const processNameSchema = z.string().regex(/^[A-Za-z0-9._-]{1,64}$/);
export const processReadinessSchema = z.discriminatedUnion('kind', [
  z.strictObject({ kind: z.literal('tcp_port'), port: z.number().int().min(1).max(65535) }),
  z.strictObject({
    kind: z.literal('output_contains'),
    needle: z
      .string()
      .min(1)
      .refine((value) => new TextEncoder().encode(value).byteLength <= 4096),
  }),
]);

export const startProcessSchema = z.strictObject({
  cmd: z.string().min(1).max(4096),
  args: z.array(z.string().max(8192)).max(128).optional(),
  name: processNameSchema.optional(),
  cwd: z.string().min(1).max(4096).optional(),
  env: z
    .record(
      z
        .string()
        .regex(/^[A-Za-z_][A-Za-z0-9_]*$/)
        .max(128),
      z.string().max(8192),
    )
    .refine((value) => Object.keys(value).length <= 128)
    .optional(),
  user: z.string().min(1).max(128).optional(),
  nonInteractive: z.boolean().optional(),
  outputRetention: z.enum(['buffer', 'stream_only']).optional(),
  maxOutputBytes: z.number().int().min(0).max(0xffff_ffff).optional(),
  redactValues: redactValuesSchema.optional(),
  readiness: processReadinessSchema.optional(),
  readyTimeoutMs: z.number().min(0).max(300_000).optional(),
  signal: z.instanceof(AbortSignal).optional(),
});

export const processReadyResponseSchema = z.discriminatedUnion('outcome', [
  z.object({ outcome: z.literal('ready') }),
  z.object({ outcome: z.literal('not_ready') }),
  z.object({ outcome: z.literal('failed'), exit_code: z.number().int().min(-0x8000_0000).max(0x7fff_ffff) }),
  z.object({ outcome: z.literal('no_probe') }),
  z.object({ outcome: z.literal('not_found') }),
  z.object({ outcome: z.literal('unsupported') }),
]);

export type StartProcessParams = z.input<typeof startProcessSchema>;

export const processStopOptionsSchema = z.strictObject({
  graceMs: z.number().min(0).max(300_000).optional(),
});

export function parseProcessInput<T extends z.ZodType>(schema: T, value: unknown): z.output<T> {
  const parsed = schema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_request', 'invalid named process operation');
  return parsed.data;
}

export function parseProcessReady(value: unknown) {
  const parsed = processReadyResponseSchema.safeParse(value);
  if (!parsed.success) throw new SandboxWsError('invalid_response', 'invalid process readiness response');
  return parsed.data;
}

export async function defaultProcessName(cmd: string, args: readonly string[]): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(JSON.stringify([cmd, args])));
  return `process-${[...new Uint8Array(digest)]
    .map((byte) => byte.toString(16).padStart(2, '0'))
    .join('')
    .slice(0, 32)}`;
}
