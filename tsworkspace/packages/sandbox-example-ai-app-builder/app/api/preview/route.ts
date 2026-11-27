import { previewStateSchema } from '../../../contracts.ts';
import type { PreviewState } from '../../../contracts.ts';
import { previewForSandbox } from '../../../server/preview.ts';
import { isMemoryMode, sandboxForSession } from '../../../server/sandbox.ts';
import { sessionFromRequest, withSessionCookie } from '../../../server/session.ts';

export const dynamic = 'force-dynamic';

export type { PreviewState } from '../../../contracts.ts';

/**
 * Resolves an authenticated, expiring preview URL for the app on APP_PORT. The browser only
 * ever gets that URL, never the API key. The page asks again after each agent turn.
 */
export async function GET(request: Request): Promise<Response> {
  const session = sessionFromRequest(request);
  return withSessionCookie(Response.json(previewStateSchema.parse(await previewState(session.id))), session);
}

async function previewState(sessionId: string): Promise<PreviewState> {
  if (isMemoryMode()) {
    return { status: 'unavailable', reason: 'Memory mode runs no real server, so there is nothing to preview.' };
  }
  try {
    const { sandbox } = await sandboxForSession(sessionId);
    return previewForSandbox(sandbox, false);
  } catch (err) {
    return { status: 'unavailable', reason: err instanceof Error ? err.message : String(err) };
  }
}
