import { isMemoryMode } from '../../../server/sandbox.ts';
import { mintBrowserSession } from '../../../server/session.ts';

export const dynamic = 'force-dynamic';

export async function POST(): Promise<Response> {
  // Authenticate the user here before minting.
  if (isMemoryMode()) {
    return Response.json({ error: 'memory mode has no gateway to mint a token from' }, { status: 501 });
  }
  return Response.json(await mintBrowserSession());
}
