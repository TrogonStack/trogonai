import { z } from 'zod';

export const chatInputSchema = z.object({ messages: z.array(z.unknown()).min(1) });
export const errorResponseSchema = z.object({ error: z.string().min(1) });
export const previewStateSchema = z.discriminatedUnion('status', [
  z.object({
    status: z.literal('ready'),
    url: z
      .string()
      .url()
      .refine((url) => /^https?:/.test(url)),
  }),
  z.object({ status: z.literal('not_ready') }),
  z.object({ status: z.literal('unavailable'), reason: z.string() }),
]);
export type PreviewState = z.infer<typeof previewStateSchema>;
