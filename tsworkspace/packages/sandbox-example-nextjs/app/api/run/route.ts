import { errorResponseSchema, scriptInputSchema } from '../../../server/contracts.ts';
import { resultFromError, runInSandbox } from '../../../server/run.ts';

// A run can outlast a host's default function timeout: sandbox timeout plus lease startup.
export const maxDuration = 90;
export const dynamic = 'force-dynamic';

export async function POST(request: Request): Promise<Response> {
  const parsed = scriptInputSchema.safeParse(await request.json().catch(() => undefined));
  // A real app authenticates the caller before this point.
  if (!parsed.success) {
    return Response.json(errorResponseSchema.parse({ error: 'script must be a non-empty string' }), { status: 400 });
  }
  try {
    return Response.json(await runInSandbox(parsed.data.script));
  } catch (err) {
    const { result, retryable } = resultFromError(err);
    // 503 tells callers a retry may work, for example when no worker is online.
    return Response.json(errorResponseSchema.parse({ error: result.stderr }), { status: retryable ? 503 : 500 });
  }
}
