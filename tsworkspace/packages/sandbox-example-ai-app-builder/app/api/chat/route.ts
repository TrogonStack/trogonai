import { convertToModelMessages, getToolName, isToolUIPart, safeValidateUIMessages, stepCountIs, streamText } from 'ai';

import { chatInputSchema, errorResponseSchema } from '../../../contracts.ts';
import { chatModel, SYSTEM_PROMPT } from '../../../server/model.ts';
import { isMemoryMode, sandboxForSession } from '../../../server/sandbox.ts';
import { sessionFromRequest, withSessionCookie } from '../../../server/session.ts';
import { historyTools, toolsForSession } from '../../../server/tools.ts';

// A build is several tool calls long; give the host room.
export const maxDuration = 300;

export async function POST(request: Request): Promise<Response> {
  const parsed = chatInputSchema.safeParse(await request.json().catch(() => undefined));
  if (!parsed.success) {
    return Response.json(errorResponseSchema.parse({ error: 'messages must be a non-empty array of chat messages' }), {
      status: 400,
    });
  }
  const contracts = historyTools(isMemoryMode());
  const validated = await safeValidateUIMessages({ messages: parsed.data.messages, tools: contracts });
  if (
    !validated.success ||
    validated.data.some((message) =>
      message.parts.some((part) => isToolUIPart(part) && !Object.hasOwn(contracts, getToolName(part))),
    )
  ) {
    return Response.json(errorResponseSchema.parse({ error: 'messages contain invalid chat history' }), {
      status: 400,
    });
  }
  const session = sessionFromRequest(request);
  const tools = toolsForSession(await sandboxForSession(session.id));

  const result = streamText({
    model: chatModel(),
    system: SYSTEM_PROMPT,
    messages: await convertToModelMessages(validated.data),
    tools,
    stopWhen: stepCountIs(20),
  });
  return withSessionCookie(result.toUIMessageStreamResponse(), session);
}
