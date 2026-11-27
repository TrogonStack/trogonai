import { z } from 'zod';

export const scriptInputSchema = z.object({
  script: z.string().refine((script) => script.trim().length > 0, 'script must be a non-empty string'),
});

export const runResultSchema = z.object({
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
});
export type RunResult = z.infer<typeof runResultSchema>;

export const errorResponseSchema = z.object({ error: z.string().min(1), hint: z.string().optional() });

export const reapQuerySchema = z.object({
  maxAgeSeconds: z.string().trim().min(1).transform(Number).pipe(z.number().finite().nonnegative()).default(900),
  dryRun: z
    .enum(['0', '1'])
    .default('0')
    .transform((value) => value === '1'),
});

export const reapResultSchema = z.object({
  dryRun: z.boolean(),
  matched: z.number().int().nonnegative(),
  released: z.array(z.string()),
  failed: z.array(z.object({ lease_id: z.string(), message: z.string() })),
});

export const streamEventSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('output'), stream: z.enum(['stdout', 'stderr']), data: z.string() }),
  z.object({ type: z.literal('exit'), exitCode: z.number().int() }),
  z.object({ type: z.literal('error'), message: z.string() }),
]);
export type StreamEvent = z.infer<typeof streamEventSchema>;

export const capacitySchema = z.object({
  available: z.boolean(),
  activeLeases: z.number().int().nonnegative(),
  maxActiveLeases: z.number().int().nonnegative(),
});
