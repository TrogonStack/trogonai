import { expectedNats, expectedRequest, isRequestOnly } from './expected.ts';
import type {
  BodyFill,
  BoltHttpVerdict,
  BoltSocketVerdict,
  DispatchRecord,
  Expected,
  ExpectedReply,
  Fixture,
  HttpFixture,
  PayloadKind,
  Provenance,
  SocketFixture,
} from './fixture.ts';
import type { BoltOracle } from './oracle.ts';
import {
  CONTENT_TYPE_FORM,
  CONTENT_TYPE_JSON,
  commandBody,
  ENVELOPE_ID,
  eventBody,
  type Headers,
  interactionBody,
  interactionPayloadText,
  type JsonObject,
  REQUEST_TIMESTAMP_SECONDS,
  SIGNING_SECRET,
  type SocketEnvelopeType,
  signedHeaders,
  signSlackRequest,
  socketEnvelope,
} from './wire.ts';

const DOCS = {
  events: 'https://docs.slack.dev/apis/events-api/',
  interactions: 'https://docs.slack.dev/interactivity/handling-user-interaction',
  commands: 'https://docs.slack.dev/interactivity/implementing-slash-commands',
  signing: 'https://docs.slack.dev/authentication/verifying-requests-from-slack',
  socketMode: 'https://docs.slack.dev/apis/events-api/using-socket-mode',
  modals: 'https://docs.slack.dev/surfaces/modals',
  multiSelect: 'https://docs.slack.dev/reference/block-kit/block-elements/multi-select-menu-element',
} as const;

const EVENT_FILE = 'test/unit/helpers/events.ts';
const HTTP_MODULE_SPEC = 'test/unit/receivers/HTTPModuleFunctions.spec.ts';

const DEFAULT_BODY_LIMIT_BYTES = 4 * 1024 * 1024;
const BODY_ABOVE_DEFAULT_LIMIT_BYTES = DEFAULT_BODY_LIMIT_BYTES + 1;
const BODY_ABOVE_BOLT_LIMIT_BYTES = 5 * 1024 * 1024;
const FILL_CHARACTER = 'a';

const URL_VERIFICATION_CHALLENGE = '3eZbrw1aBm2rZgRNFdxV2595E9CY3gmdALWMmHkvFXO7tYXAYM8P';

export function expandBodyFill(fill: BodyFill): string {
  return `${fill.prefix}${fill.filler.repeat(fill.count)}${fill.suffix}`;
}

type RepresentableKind = Extract<PayloadKind, 'event' | 'interaction' | 'command'>;

interface PayloadCase {
  id: string;
  kind: RepresentableKind;
  builder: string;
  args: unknown[];
  acceptsResponsePayload: boolean;
}

const PAYLOAD_CASES: readonly PayloadCase[] = [
  event('app_home_opened', 'createDummyAppHomeOpenedEventMiddlewareArgs'),
  event('member_joined_channel', 'createDummyMemberChannelEventMiddlewareArgs', { type: 'member_joined_channel' }),
  event('member_left_channel', 'createDummyMemberChannelEventMiddlewareArgs', { type: 'member_left_channel' }),
  event('reaction_added', 'createDummyReactionAddedEventMiddlewareArgs'),
  event('message', 'createDummyMessageEventMiddlewareArgs'),
  event('message_bot_subtype', 'createDummyMessageEventMiddlewareArgs', { subtype: 'bot_message' }),
  event('message_direct', 'createDummyMessageEventMiddlewareArgs', { channel_type: 'im' }),
  event('message_threaded', 'createDummyMessageEventMiddlewareArgs', { thread_ts: '1234.56' }),
  event('app_mention', 'createDummyAppMentionEventMiddlewareArgs'),
  event('assistant_thread_started', 'createDummyAssistantThreadStartedEventMiddlewareArgs'),
  event('assistant_thread_context_changed', 'createDummyAssistantThreadContextChangedEventMiddlewareArgs'),
  event('assistant_user_message', 'createDummyAssistantUserMessageEventMiddlewareArgs'),
  event('function_executed', 'createDummyCustomFunctionMiddlewareArgs'),
  interaction('block_actions', 'createDummyBlockActionEventMiddlewareArgs', false),
  interaction('block_actions_function_scoped', 'createDummyFunctionScopedBlockActionEventMiddlewareArgs', false),
  interaction('block_suggestion', 'createDummyBlockSuggestionsMiddlewareArgs', true),
  interaction('view_submission', 'createDummyViewSubmissionMiddlewareArgs', true),
  interaction('view_closed', 'createDummyViewClosedMiddlewareArgs', false),
  interaction('message_action', 'createDummyMessageShortcutMiddlewareArgs', false),
  interaction('shortcut', 'createDummyGlobalShortcutMiddlewareArgs', false),
  command('slash', 'createDummyCommandMiddlewareArgs'),
  command('slash_nested_name', 'createDummyCommandMiddlewareArgs', { command: '/deploy-app' }),
  command('slash_reserved_characters', 'createDummyCommandMiddlewareArgs', {
    slashCommand: {
      token: 'xoxb-1234',
      command: '/slash',
      text: 'a&b=c +d%e é',
      response_url: 'https://slack.com',
      trigger_id: '1234.56',
      user_id: 'U1234',
      user_name: 'filmaj',
      team_id: 'T1234',
      team_domain: 'slack.com',
      channel_id: 'C1234',
      channel_name: '#random',
      api_app_id: 'A1234',
    },
  }),
];

function event(id: string, builder: string, ...args: unknown[]): PayloadCase {
  return { id: `event.${id}`, kind: 'event', builder, args, acceptsResponsePayload: false };
}

function interaction(id: string, builder: string, acceptsResponsePayload: boolean, ...args: unknown[]): PayloadCase {
  return { id: `interaction.${id}`, kind: 'interaction', builder, args, acceptsResponsePayload };
}

function command(id: string, builder: string, ...args: unknown[]): PayloadCase {
  return { id: `command.${id}`, kind: 'command', builder, args, acceptsResponsePayload: true };
}

type AckBody = JsonObject | string;

function consumerReply(ackBody: AckBody | undefined): ExpectedReply {
  if (ackBody === undefined) {
    return { content_type: null, body: '' };
  }
  if (typeof ackBody === 'string') {
    return { content_type: 'text/plain', body: ackBody };
  }
  return { content_type: 'application/json', body: JSON.stringify(ackBody) };
}

interface AckCase {
  id: string;
  payloadCase: string;
  ackBody: AckBody;
  ackSource: string;
  ackNote: string;
}

const ACK_CASES: readonly AckCase[] = [
  {
    id: 'view_submission_errors',
    payloadCase: 'interaction.view_submission',
    ackBody: { response_action: 'errors', errors: { block_id: 'The value is not valid' } },
    ackSource: DOCS.modals,
    ackNote:
      'response_action errors shape from the modals reference; the block id and message are chosen by the exporter',
  },
  {
    id: 'block_suggestion_options',
    payloadCase: 'interaction.block_suggestion',
    ackBody: { options: [{ text: { type: 'plain_text', text: 'First option' }, value: 'first' }] },
    ackSource: DOCS.multiSelect,
    ackNote: 'options shape from the external select reference; the option text and value are chosen by the exporter',
  },
  {
    id: 'slash_in_channel_reply',
    payloadCase: 'command.slash',
    ackBody: { response_type: 'in_channel', text: 'It is 80 degrees right now.' },
    ackSource: DOCS.commands,
    ackNote: 'immediate response shape from the slash commands guide',
  },
  {
    id: 'slash_text_reply',
    payloadCase: 'command.slash',
    ackBody: 'It is 80 degrees right now.',
    ackSource: DOCS.commands,
    ackNote: 'plain text immediate response from the slash commands guide',
  },
];

function socketEnvelopeType(kind: RepresentableKind): SocketEnvelopeType {
  switch (kind) {
    case 'event':
      return 'events_api';
    case 'interaction':
      return 'interactive';
    case 'command':
      return 'slash_commands';
  }
}

function wireSourceFor(kind: RepresentableKind): string {
  switch (kind) {
    case 'event':
      return DOCS.events;
    case 'interaction':
      return DOCS.interactions;
    case 'command':
      return DOCS.commands;
  }
}

export interface CatalogContext {
  oracle: BoltOracle;
}

function provenance(
  oracle: BoltOracle,
  payloadSource: string,
  wireSource: string | null,
  ackSource: string | null = null,
): Provenance {
  return {
    bolt_version: oracle.version,
    bolt_commit: oracle.commit,
    node_slack_sdk_commit: oracle.sdkCommit,
    payload_source: payloadSource,
    wire_source: wireSource,
    ack_source: ackSource,
  };
}

function nonEmpty(payload: JsonObject | undefined): JsonObject | null {
  return payload !== undefined && Object.keys(payload).length > 0 ? payload : null;
}

interface HttpSpec {
  id: string;
  kind: PayloadKind;
  provenance: Provenance;
  headers: Headers;
  body: string | BodyFill;
  wirePayload?: string;
  ackBody?: AckBody;
  challenge?: string;
}

function httpExpected(spec: HttpSpec, verdict: BoltHttpVerdict, parsedBody: JsonObject | null): Expected {
  const status = verdict.status;
  if (verdict.outcome === 'dispatched') {
    if (
      parsedBody === null ||
      verdict.dispatch === null ||
      spec.wirePayload === undefined ||
      (spec.kind !== 'event' && spec.kind !== 'interaction' && spec.kind !== 'command')
    ) {
      throw new Error(`${spec.id}: Bolt dispatched a body the catalog cannot describe`);
    }
    const nats = expectedNats({
      kind: spec.kind,
      transport: 'http',
      body: parsedBody,
      wirePayload: spec.wirePayload,
      dispatch: verdict.dispatch,
      requestTimestamp: spec.headers['x-slack-request-timestamp'],
    });
    if (typeof spec.body !== 'string') {
      nats.payload = '';
      nats.payload_fill = spec.body;
    }
    const replyBody = verdict.body ?? '';
    const replyContentType = replyBody === '' ? null : verdict.content_type;
    const requestOnly = isRequestOnly(nats);
    return {
      outcome: requestOnly ? 'request' : 'publish',
      nats: requestOnly ? null : nats,
      request: expectedRequest(spec.kind, parsedBody, nats, { content_type: replyContentType, body: replyBody }),
      http: { status, body: verdict.body, content_type: replyContentType, challenge: null },
      socket_ack: null,
    };
  }
  if (verdict.outcome === 'inline') {
    return {
      outcome: 'inline',
      nats: null,
      request: null,
      http: {
        status,
        body: spec.challenge === undefined ? verdict.body : null,
        content_type: null,
        challenge: spec.challenge ?? null,
      },
      socket_ack: null,
    };
  }
  return {
    outcome: 'reject',
    nats: null,
    request: null,
    http: { status, body: null, content_type: null, challenge: null },
    socket_ack: null,
  };
}

async function httpFixture(context: CatalogContext, spec: HttpSpec): Promise<HttpFixture> {
  const body = typeof spec.body === 'string' ? spec.body : expandBodyFill(spec.body);
  const { verdict, parsedBody } = await context.oracle.runHttp({ headers: spec.headers, body }, spec.ackBody);
  return {
    schema_version: 1,
    id: spec.id,
    kind: spec.kind,
    transport: 'http',
    provenance: spec.provenance,
    not_representable: null,
    input:
      typeof spec.body === 'string'
        ? { headers: spec.headers, body: spec.body }
        : { headers: spec.headers, body_fill: spec.body },
    bolt: verdict,
    expected: httpExpected(spec, verdict, parsedBody),
  };
}

interface SocketSpec {
  id: string;
  kind: RepresentableKind;
  provenance: Provenance;
  type: SocketEnvelopeType;
  payload: JsonObject;
  acceptsResponsePayload: boolean;
  retryAttempt?: number;
  retryReason?: string;
  ackBody?: AckBody;
}

function socketExpected(spec: SocketSpec, verdict: BoltSocketVerdict, parsedBody: JsonObject | null): Expected {
  if (verdict.outcome !== 'dispatched' || verdict.dispatch === null || parsedBody === null || verdict.ack === null) {
    throw new Error(`${spec.id}: Bolt did not dispatch and acknowledge the envelope`);
  }
  const nats = expectedNats({
    kind: spec.kind,
    transport: 'socket_mode',
    body: parsedBody,
    wirePayload: JSON.stringify(spec.payload),
    dispatch: verdict.dispatch satisfies DispatchRecord,
    envelopeId: ENVELOPE_ID,
  });
  const requestOnly = isRequestOnly(nats);
  return {
    outcome: requestOnly ? 'request' : 'publish',
    nats: requestOnly ? null : nats,
    request: expectedRequest(spec.kind, parsedBody, nats, consumerReply(spec.ackBody)),
    http: null,
    socket_ack: { envelope_id: verdict.ack.envelope_id, payload: nonEmpty(verdict.ack.payload) },
  };
}

async function socketFixture(context: CatalogContext, spec: SocketSpec): Promise<SocketFixture> {
  const frame = socketEnvelope({
    type: spec.type,
    payload: spec.payload,
    acceptsResponsePayload: spec.acceptsResponsePayload,
    retryAttempt: spec.retryAttempt,
    retryReason: spec.retryReason,
  });
  const { verdict, parsedBody } = await context.oracle.runSocket(frame, spec.ackBody);
  return {
    schema_version: 1,
    id: spec.id,
    kind: spec.kind,
    transport: 'socket_mode',
    provenance: spec.provenance,
    not_representable: null,
    input: { frame },
    bolt: verdict,
    expected: socketExpected(spec, verdict, parsedBody),
  };
}

function httpWire(
  kind: RepresentableKind,
  payload: JsonObject,
): { body: string; contentType: string; wirePayload: string } {
  switch (kind) {
    case 'event': {
      const body = eventBody(payload);
      return { body, contentType: CONTENT_TYPE_JSON, wirePayload: body };
    }
    case 'interaction':
      return {
        body: interactionBody(payload),
        contentType: CONTENT_TYPE_FORM,
        wirePayload: interactionPayloadText(payload),
      };
    case 'command': {
      const body = commandBody(payload);
      return { body, contentType: CONTENT_TYPE_FORM, wirePayload: body };
    }
  }
}

function representableHttpSpec(
  context: CatalogContext,
  id: string,
  payloadCase: PayloadCase,
  payload: JsonObject,
  extra: { headers?: Headers; ackBody?: AckBody; provenance?: Provenance } = {},
): HttpSpec {
  const wire = httpWire(payloadCase.kind, payload);
  return {
    id,
    kind: payloadCase.kind,
    provenance:
      extra.provenance ??
      provenance(context.oracle, `${EVENT_FILE}:${payloadCase.builder}`, wireSourceFor(payloadCase.kind)),
    headers: { ...signedHeaders({ contentType: wire.contentType, body: wire.body }), ...extra.headers },
    body: wire.body,
    wirePayload: wire.wirePayload,
    ackBody: extra.ackBody,
  };
}

function oversizedEvent(bytes: number): BodyFill {
  const prefix =
    '{"type":"event_callback","event_id":"Ev0OVERSIZE","team_id":"T1234","event":{"type":"message","text":"';
  const suffix = '"}}';
  return { prefix, filler: FILL_CHARACTER, count: bytes - prefix.length - suffix.length, suffix };
}

function fillHeaders(fill: BodyFill): Headers {
  return signedHeaders({ contentType: CONTENT_TYPE_JSON, body: expandBodyFill(fill) });
}

async function representableFixtures(context: CatalogContext): Promise<Fixture[]> {
  const fixtures: Fixture[] = [];
  const payloads = new Map<string, JsonObject>();
  for (const payloadCase of PAYLOAD_CASES) {
    const payload = context.oracle.dummyBody(payloadCase.builder, ...payloadCase.args);
    payloads.set(payloadCase.id, payload);
    fixtures.push(
      await httpFixture(context, representableHttpSpec(context, `http.${payloadCase.id}`, payloadCase, payload)),
    );
    fixtures.push(
      await socketFixture(context, {
        id: `socket.${payloadCase.id}`,
        kind: payloadCase.kind,
        provenance: provenance(context.oracle, `${EVENT_FILE}:${payloadCase.builder}`, DOCS.socketMode),
        type: socketEnvelopeType(payloadCase.kind),
        payload,
        acceptsResponsePayload: payloadCase.acceptsResponsePayload,
      }),
    );
  }

  for (const ack of ACK_CASES) {
    const payloadCase = PAYLOAD_CASES.find((candidate) => candidate.id === ack.payloadCase);
    const payload = payloads.get(ack.payloadCase);
    if (payloadCase === undefined || payload === undefined) {
      throw new Error(`Unknown payload case ${ack.payloadCase}`);
    }
    const source = `${EVENT_FILE}:${payloadCase.builder} plus ack body: ${ack.ackNote}`;
    fixtures.push(
      await httpFixture(
        context,
        representableHttpSpec(context, `http.ack.${ack.id}`, payloadCase, payload, {
          ackBody: ack.ackBody,
          provenance: provenance(context.oracle, source, wireSourceFor(payloadCase.kind), ack.ackSource),
        }),
      ),
      await socketFixture(context, {
        id: `socket.ack.${ack.id}`,
        kind: payloadCase.kind,
        provenance: provenance(context.oracle, source, DOCS.socketMode, ack.ackSource),
        type: socketEnvelopeType(payloadCase.kind),
        payload,
        acceptsResponsePayload: true,
        ackBody: ack.ackBody,
      }),
    );
  }

  const message = PAYLOAD_CASES.find((candidate) => candidate.id === 'event.message');
  const messagePayload = payloads.get('event.message');
  if (message === undefined || messagePayload === undefined) {
    throw new Error('event.message payload case is missing');
  }
  const retrySource = `${EVENT_FILE}:${message.builder} plus Slack retry metadata`;
  fixtures.push(
    await httpFixture(
      context,
      representableHttpSpec(context, 'http.retry.event_message', message, messagePayload, {
        headers: { 'x-slack-retry-num': '1', 'x-slack-retry-reason': 'http_timeout' },
        provenance: provenance(context.oracle, retrySource, DOCS.events),
      }),
    ),
    await socketFixture(context, {
      id: 'socket.retry.event_message',
      kind: 'event',
      provenance: provenance(context.oracle, retrySource, DOCS.socketMode),
      type: 'events_api',
      payload: messagePayload,
      acceptsResponsePayload: false,
      retryAttempt: 1,
      retryReason: 'timeout',
    }),
  );

  return fixtures;
}

async function negativeFixtures(context: CatalogContext): Promise<Fixture[]> {
  const { oracle } = context;
  const fixtures: Fixture[] = [];
  const message = oracle.dummyBody('createDummyMessageEventMiddlewareArgs');
  const messageBody = eventBody(message);
  const messageSource = `${EVENT_FILE}:createDummyMessageEventMiddlewareArgs`;
  const signingSource = `${HTTP_MODULE_SPEC} and verify-request.spec.ts request verification vectors`;
  const signed = (extra: Headers = {}): Headers => ({
    ...signedHeaders({ contentType: CONTENT_TYPE_JSON, body: messageBody }),
    ...extra,
  });
  const withoutHeader = (headers: Headers, name: string): Headers =>
    Object.fromEntries(Object.entries(headers).filter(([key]) => key !== name));

  const verification = (id: string, headers: Headers, note: string): HttpSpec => ({
    id,
    kind: 'event',
    provenance: provenance(oracle, `${messageSource} (${note}); ${signingSource}`, DOCS.signing),
    headers,
    body: messageBody,
    wirePayload: messageBody,
  });

  const staleTimestamp = REQUEST_TIMESTAMP_SECONDS - 301;
  const driftLimitTimestamp = REQUEST_TIMESTAMP_SECONDS - 300;
  const specs: HttpSpec[] = [
    verification('http.verify.missing_signature', withoutHeader(signed(), 'x-slack-signature'), 'no signature header'),
    verification(
      'http.verify.invalid_signature',
      signed({ 'x-slack-signature': 'v0=invalid-signature' }),
      'bad signature',
    ),
    verification(
      'http.verify.wrong_secret',
      {
        ...signed(),
        'x-slack-signature': signSlackRequest('another-signing-secret', REQUEST_TIMESTAMP_SECONDS, messageBody),
      },
      'signed with another secret',
    ),
    verification(
      'http.verify.missing_timestamp',
      withoutHeader(signed(), 'x-slack-request-timestamp'),
      'no timestamp header',
    ),
    verification(
      'http.verify.non_numeric_timestamp',
      signedHeaders({ contentType: CONTENT_TYPE_JSON, body: messageBody, timestampSeconds: 'not-a-number' }),
      'non numeric timestamp',
    ),
    verification(
      'http.verify.stale_timestamp',
      signedHeaders({ contentType: CONTENT_TYPE_JSON, body: messageBody, timestampSeconds: staleTimestamp }),
      'timestamp 301 seconds old',
    ),
    verification(
      'http.verify.timestamp_at_drift_limit',
      signedHeaders({ contentType: CONTENT_TYPE_JSON, body: messageBody, timestampSeconds: driftLimitTimestamp }),
      'timestamp exactly 300 seconds old',
    ),
  ];
  for (const spec of specs) {
    fixtures.push(await httpFixture(context, spec));
  }

  const invalidJson = '{"type":"event_callback","event":';
  fixtures.push(
    await httpFixture(context, {
      id: 'http.malformed.invalid_json',
      kind: 'malformed',
      provenance: provenance(oracle, `hand written truncated JSON body; ${signingSource}`, DOCS.events),
      headers: signedHeaders({ contentType: CONTENT_TYPE_JSON, body: invalidJson }),
      body: invalidJson,
    }),
  );

  const invalidInteraction = new URLSearchParams({ payload: 'not-json' }).toString();
  fixtures.push(
    await httpFixture(context, {
      id: 'http.malformed.invalid_interaction_payload',
      kind: 'malformed',
      provenance: provenance(oracle, 'hand written form body whose payload field is not JSON', DOCS.interactions),
      headers: signedHeaders({ contentType: CONTENT_TYPE_FORM, body: invalidInteraction }),
      body: invalidInteraction,
    }),
  );

  const urlVerification = JSON.stringify({
    token: 'Jhj5dZrVaK7ZwHHjRyZWjbDl',
    challenge: URL_VERIFICATION_CHALLENGE,
    type: 'url_verification',
  });
  fixtures.push(
    await httpFixture(context, {
      id: 'http.url_verification.signed',
      kind: 'url_verification',
      provenance: provenance(oracle, 'url_verification example from the Events API reference', DOCS.events),
      headers: signedHeaders({ contentType: CONTENT_TYPE_JSON, body: urlVerification }),
      body: urlVerification,
      challenge: URL_VERIFICATION_CHALLENGE,
    }),
    await httpFixture(context, {
      id: 'http.url_verification.unsigned',
      kind: 'url_verification',
      provenance: provenance(oracle, 'url_verification example from the Events API reference, unsigned', DOCS.events),
      headers: { 'content-type': CONTENT_TYPE_JSON },
      body: urlVerification,
    }),
  );

  const sslCheckSource = (title: string): string => `${HTTP_MODULE_SPEC}: ${title}`;
  const unsigned = (contentType: string): Headers => ({ 'content-type': contentType });
  const sslChecks: Array<{ id: string; title: string; body: string; contentType: string }> = [
    {
      id: 'http.ssl_check.unsigned',
      title: 'should parse a ssl_check request body without signature verification',
      body: 'ssl_check=1&token=legacy-fixed-verification-token',
      contentType: CONTENT_TYPE_FORM,
    },
    {
      id: 'http.ssl_check.smuggled_payload',
      title: 'should strip smuggled event payloads from ssl_check requests',
      body: `ssl_check=1&payload=${encodeURIComponent('{"type":"event_callback"}')}`,
      contentType: CONTENT_TYPE_FORM,
    },
    {
      id: 'http.ssl_check.zero',
      title: 'should not bypass signature verification when ssl_check=0',
      body: 'ssl_check=0&token=legacy-fixed-verification-token',
      contentType: CONTENT_TYPE_FORM,
    },
    {
      id: 'http.ssl_check.true',
      title: 'should not bypass signature verification when ssl_check=true',
      body: 'ssl_check=true&token=legacy-fixed-verification-token',
      contentType: CONTENT_TYPE_FORM,
    },
    {
      id: 'http.ssl_check.empty',
      title: 'should not bypass signature verification when ssl_check is empty',
      body: 'ssl_check=&token=legacy-fixed-verification-token',
      contentType: CONTENT_TYPE_FORM,
    },
    {
      id: 'http.ssl_check.json_content_type',
      title: 'should not bypass signature verification for ssl_check=1 with JSON content-type',
      body: '{"ssl_check":"1"}',
      contentType: CONTENT_TYPE_JSON,
    },
  ];
  for (const check of sslChecks) {
    fixtures.push(
      await httpFixture(context, {
        id: check.id,
        kind: 'ssl_check',
        provenance: provenance(oracle, sslCheckSource(check.title), DOCS.signing),
        headers: unsigned(check.contentType),
        body: check.body,
      }),
    );
  }

  const atDefault = oversizedEvent(DEFAULT_BODY_LIMIT_BYTES);
  const aboveDefault = oversizedEvent(BODY_ABOVE_DEFAULT_LIMIT_BYTES);
  const aboveBolt = oversizedEvent(BODY_ABOVE_BOLT_LIMIT_BYTES);
  fixtures.push(
    await httpFixture(context, {
      id: 'http.body_limit.at_default',
      kind: 'event',
      provenance: provenance(
        oracle,
        `${HTTP_MODULE_SPEC}: defaultBodyLimit; signed event body of ${DEFAULT_BODY_LIMIT_BYTES} bytes, exactly at the shared ${DEFAULT_BODY_LIMIT_BYTES} default`,
        DOCS.events,
      ),
      headers: fillHeaders(atDefault),
      body: atDefault,
      wirePayload: expandBodyFill(atDefault),
    }),
    await httpFixture(context, {
      id: 'http.body_limit.above_default',
      kind: 'event',
      provenance: provenance(
        oracle,
        `${HTTP_MODULE_SPEC}: defaultBodyLimit; signed event body of ${BODY_ABOVE_DEFAULT_LIMIT_BYTES} bytes, one byte above the shared ${DEFAULT_BODY_LIMIT_BYTES} default`,
        DOCS.events,
      ),
      headers: fillHeaders(aboveDefault),
      body: aboveDefault,
    }),
    await httpFixture(context, {
      id: 'http.body_limit.above_both',
      kind: 'event',
      provenance: provenance(
        oracle,
        `${HTTP_MODULE_SPEC}: should reject an oversized request body with a 413 error; signed event body of ${BODY_ABOVE_BOLT_LIMIT_BYTES} bytes`,
        DOCS.events,
      ),
      headers: fillHeaders(aboveBolt),
      body: aboveBolt,
    }),
  );

  const receiverEvent = await degenerateReceiverEvent(context);
  fixtures.push(receiverEvent);
  return fixtures;
}

async function degenerateReceiverEvent(context: CatalogContext): Promise<HttpFixture> {
  const { oracle } = context;
  const body = eventBody({ event: { type: 'dummy_event_type' } });
  const { verdict } = await oracle.runHttp({
    headers: signedHeaders({ contentType: CONTENT_TYPE_JSON, body }),
    body,
  });
  return {
    schema_version: 1,
    id: 'http.not_representable.dummy_receiver_event',
    kind: 'malformed',
    transport: 'http',
    provenance: provenance(oracle, `${EVENT_FILE}:createDummyReceiverEvent`, null),
    not_representable:
      'createDummyReceiverEvent is an in-process ReceiverEvent that omits the event_callback envelope, which Slack never sends',
    input: { headers: signedHeaders({ contentType: CONTENT_TYPE_JSON, body }), body },
    bolt: verdict,
    expected: { outcome: 'ignore', nats: null, request: null, http: null, socket_ack: null },
  };
}

export async function buildCatalog(context: CatalogContext): Promise<Fixture[]> {
  const fixtures = [...(await representableFixtures(context)), ...(await negativeFixtures(context))];
  const seen = new Set<string>();
  for (const fixture of fixtures) {
    if (seen.has(fixture.id)) {
      throw new Error(`Duplicate fixture id ${fixture.id}`);
    }
    seen.add(fixture.id);
  }
  return fixtures;
}

export { SIGNING_SECRET };
