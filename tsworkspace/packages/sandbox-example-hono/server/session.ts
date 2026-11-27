import { z } from 'zod';

import { requiredEnv } from './sandbox.ts';

const gatewaySessionSchema = z.object({
  token: z.string().min(1),
  expires_at: z.number().int().positive(),
});

const browserSessionSchema = z.object({
  token: z.string().min(1),
  expiresAt: z.number().int().positive(),
  gatewayWsUrl: z
    .string()
    .url()
    .refine((url) => /^wss?:/.test(url), 'a WebSocket URL is required'),
});

export type BrowserSession = z.infer<typeof browserSessionSchema>;

/** Calls the gateway's `POST /v1/sessions` with the API key and returns only the short-lived token. */
export async function mintBrowserSession(): Promise<BrowserSession> {
  const response = await fetch(`${requiredEnv('SANDBOX_GATEWAY_HTTP_URL')}/v1/sessions`, {
    method: 'POST',
    headers: { 'X-Sandbox-API-Key': requiredEnv('SANDBOX_API_KEY') },
  });
  if (response.status !== 201) {
    throw new Error(`session mint failed with status ${response.status}`);
  }
  const body = gatewaySessionSchema.parse(await response.json());
  return browserSessionSchema.parse({
    token: body.token,
    expiresAt: body.expires_at,
    gatewayWsUrl: requiredEnv('SANDBOX_GATEWAY_WS_URL'),
  });
}
