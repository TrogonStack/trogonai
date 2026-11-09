import { EventEmitter } from 'node:events';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { Readable } from 'node:stream';

import type { BoltHttpVerdict, BoltSocketVerdict, DispatchRecord, HttpInput, SocketAckFrame } from './fixture.ts';
import { evaluateSnippet, extractBlock, type ModuleExports, TypeScriptLoader } from './loader.ts';
import { createExternals, createRecordingLogger } from './stubs.ts';
import { REQUEST_TIMESTAMP_SECONDS, SIGNING_SECRET } from './wire.ts';
import type { JsonObject } from './wire.ts';

const RESPONSE_DEADLINE_MS = 5_000;
const UNHANDLED_REQUEST_TIMEOUT_MS = 25;

type Constructor<T> = new (options: Record<string, unknown>) => T;

interface ReceiverEvent {
  body: JsonObject;
  retryNum?: number;
  retryReason?: string;
  ack: (response?: unknown) => Promise<void>;
}

interface FakeApp {
  processEvent(event: ReceiverEvent): Promise<void>;
}

interface HttpReceiverUnderTest {
  init(app: FakeApp): void;
  requestListener(request: unknown, response: unknown): void;
}

interface SocketClientUnderTest {
  websocket: unknown;
  onWebSocketMessage(data: string, isBinary: boolean): Promise<void>;
}

interface SocketReceiverUnderTest {
  client: SocketClientUnderTest;
  init(app: FakeApp): void;
}

interface BoltSource {
  userId?: string;
  teamId?: string;
  enterpriseId?: string;
}

interface Helpers {
  IncomingEventType: Record<number, string>;
  assertNever: unknown;
  getTypeAndConversation(body: JsonObject): { type?: number; conversationId?: string };
  isBodyWithTypeEnterpriseInstall(body: JsonObject, type?: number): boolean;
}

export interface OracleRun<Verdict> {
  verdict: Verdict;
  parsedBody: JsonObject | null;
}

export interface BoltOracle {
  readonly version: string;
  readonly commit: string;
  readonly sdkCommit: string;
  dummyBody(builder: string, ...args: unknown[]): JsonObject;
  runHttp(input: HttpInput & { body: string }, ackBody?: unknown): Promise<OracleRun<BoltHttpVerdict>>;
  runSocket(frame: string, ackBody?: unknown): Promise<OracleRun<BoltSocketVerdict>>;
}

export interface OracleOptions {
  boltPath: string;
  sdkPath: string;
  boltCommit: string;
  sdkCommit: string;
}

class FakeResponse {
  statusCode: number | undefined;
  headersSent = false;
  body = '';
  readonly headers: Record<string, string> = {};
  readonly ended: Promise<void>;
  #resolve: () => void = () => undefined;

  constructor() {
    this.ended = new Promise((resolve) => {
      this.#resolve = resolve;
    });
  }

  writeHead(status: number, headers: Record<string, string> = {}): this {
    this.statusCode = status;
    this.headersSent = true;
    for (const [name, value] of Object.entries(headers)) {
      this.headers[name.toLowerCase()] = value;
    }
    return this;
  }

  end(chunk?: string): void {
    this.body += chunk ?? '';
    this.#resolve();
  }
}

function readPackageVersion(root: string): string {
  const manifest = JSON.parse(readFileSync(path.join(root, 'package.json'), 'utf8')) as { version: string };
  return manifest.version;
}

function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

async function withFixedClock<T>(action: () => Promise<T>): Promise<T> {
  const realNow = Date.now;
  Date.now = () => REQUEST_TIMESTAMP_SECONDS * 1000;
  try {
    return await action();
  } finally {
    Date.now = realNow;
  }
}

function buildSocketClientClass(sdkPath: string, silentLogger: unknown): Constructor<SocketClientUnderTest> {
  const source = readFileSync(path.join(sdkPath, 'packages/socket-mode/src/SocketModeClient.ts'), 'utf8');
  const onMessage = extractBlock(
    source,
    /protected async onWebSocketMessage\(/,
    /isBinary: boolean\): Promise<void> \{/,
  );
  const send = extractBlock(source, /private send\(id: string, body = \{\}\)/, /Promise<void> \{/);

  class SMSendWhileDisconnectedError extends Error {}
  class SMSendWhileNotReadyError extends Error {}
  class SMWebsocketError extends Error {}

  return evaluateSnippet(
    `class SocketModeClientUnderTest extends EventEmitter {
      logger = silentLogger;
      autoReconnectEnabled = true;
      connectionResponse = { ok: true };
      websocket = undefined;
      constructor(_options) { super(); }
      ${onMessage}
      ${send}
    }
    module.exports = SocketModeClientUnderTest;`,
    {
      EventEmitter,
      silentLogger,
      State: { Connected: 'connected' },
      LogLevel: { DEBUG: 'debug' },
      WS_READY_STATES: ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED'],
      SMSendWhileDisconnectedError,
      SMSendWhileNotReadyError,
      SMWebsocketError,
    },
  ) as Constructor<SocketClientUnderTest>;
}

export function createOracle(options: OracleOptions): BoltOracle {
  const { boltPath, sdkPath } = options;
  const silentLogger = createRecordingLogger();
  const socketClientClass = buildSocketClientClass(sdkPath, silentLogger);

  const helpersIndex = path.join(boltPath, 'test/unit/helpers/index.ts');
  const loader = new TypeScriptLoader({
    externals: createExternals(socketClientClass),
    overrides: new Map<string, ModuleExports>([[helpersIndex, { createFakeLogger: createRecordingLogger }]]),
  });

  const helpers = loader.load(path.join(boltPath, 'src/helpers.ts')) as unknown as Helpers;
  const events = loader.load(path.join(boltPath, 'test/unit/helpers/events.ts'));
  const HTTPReceiver = loader.load(path.join(boltPath, 'src/receivers/HTTPReceiver.ts'))
    .default as Constructor<HttpReceiverUnderTest>;
  const SocketModeReceiver = loader.load(path.join(boltPath, 'src/receivers/SocketModeReceiver.ts'))
    .default as Constructor<SocketReceiverUnderTest>;

  const appSource = readFileSync(path.join(boltPath, 'src/App.ts'), 'utf8');
  const buildSource = evaluateSnippet(
    `${extractBlock(appSource, /function buildSource</, /\): AuthorizeSourceData<IsEnterpriseInstall> \{/)}
    module.exports = buildSource;`,
    { IncomingEventType: helpers.IncomingEventType, assertNever: helpers.assertNever },
  ) as (type: number, conversationId: string | undefined, body: JsonObject, enterprise: boolean) => BoltSource;

  function describeDispatch(event: ReceiverEvent): DispatchRecord | null {
    const { body } = event;
    const { type, conversationId } = helpers.getTypeAndConversation(body);
    if (type === undefined) {
      return null;
    }
    const isEnterpriseInstall = helpers.isBodyWithTypeEnterpriseInstall(body, type);
    const source = buildSource(type, conversationId, body, isEnterpriseInstall);
    return {
      incoming_type: helpers.IncomingEventType[type] ?? 'Unclassified',
      conversation_id: conversationId ?? null,
      retry_num: event.retryNum ?? null,
      retry_reason: event.retryReason ?? null,
      is_enterprise_install: isEnterpriseInstall,
      team_id: source.teamId ?? null,
      enterprise_id: source.enterpriseId ?? null,
      user_id: source.userId ?? null,
    };
  }

  async function runHttp(input: HttpInput & { body: string }, ackBody?: unknown): Promise<OracleRun<BoltHttpVerdict>> {
    const logger = createRecordingLogger();
    const receiver = new HTTPReceiver({
      signingSecret: SIGNING_SECRET,
      logger,
      unhandledRequestTimeoutMillis: UNHANDLED_REQUEST_TIMEOUT_MS,
    });
    const captured: { dispatch: DispatchRecord | null; body: JsonObject | null } = { dispatch: null, body: null };
    receiver.init({
      async processEvent(event) {
        const dispatch = describeDispatch(event);
        if (dispatch === null) {
          return;
        }
        captured.body = event.body;
        captured.dispatch = dispatch;
        await event.ack(ackBody);
      },
    });

    const request = new Readable();
    request.push(input.body);
    request.push(null);
    Object.assign(request, { headers: input.headers, method: 'POST', url: '/slack/events' });
    const response = new FakeResponse();

    await withFixedClock(async () => {
      receiver.requestListener(request, response);
      const deadline = new Promise<never>((_, reject) => {
        setTimeout(() => reject(new Error('Bolt produced no HTTP response')), RESPONSE_DEADLINE_MS).unref();
      });
      await Promise.race([response.ended, deadline]);
    });

    if (response.statusCode === undefined) {
      throw new Error('Bolt ended the response without a status');
    }
    const outcome = captured.dispatch !== null ? 'dispatched' : response.statusCode === 200 ? 'inline' : 'rejected';
    return {
      verdict: {
        outcome,
        status: response.statusCode,
        body: response.body === '' ? null : response.body,
        content_type: response.headers['content-type'] ?? null,
        reason: logger.warnings[0] ?? null,
        dispatch: captured.dispatch,
      },
      parsedBody: captured.body,
    };
  }

  async function runSocket(frame: string, ackBody?: unknown): Promise<OracleRun<BoltSocketVerdict>> {
    const receiver = new SocketModeReceiver({ appToken: 'xapp-1-conformance', logger: createRecordingLogger() });
    const sent: string[] = [];
    const captured: { dispatch: DispatchRecord | null; body: JsonObject | null; disconnected: boolean } = {
      dispatch: null,
      body: null,
      disconnected: false,
    };
    receiver.client.websocket = {
      readyState: 1,
      isActive: () => true,
      disconnect() {
        captured.disconnected = true;
      },
      send(data: string, callback: (error?: Error) => void) {
        sent.push(data);
        callback();
      },
    };
    receiver.init({
      async processEvent(event) {
        const dispatch = describeDispatch(event);
        if (dispatch === null) {
          return;
        }
        captured.body = event.body;
        captured.dispatch = dispatch;
        await event.ack(ackBody);
      },
    });

    await receiver.client.onWebSocketMessage(frame, false);
    await settle();
    await settle();

    const first = sent[0];
    const ack = first === undefined ? null : (JSON.parse(first) as SocketAckFrame);
    const outcome = captured.dispatch !== null ? 'dispatched' : captured.disconnected ? 'disconnected' : 'ignored';
    return { verdict: { outcome, ack, dispatch: captured.dispatch }, parsedBody: captured.body };
  }

  function dummyBody(builder: string, ...args: unknown[]): JsonObject {
    const factory = events[builder];
    if (typeof factory !== 'function') {
      throw new Error(`Bolt helper ${builder} not found in test/unit/helpers/events.ts`);
    }
    const built = (factory as (...input: unknown[]) => { body: JsonObject })(...args);
    return JSON.parse(JSON.stringify(built.body)) as JsonObject;
  }

  return {
    version: readPackageVersion(boltPath),
    commit: options.boltCommit,
    sdkCommit: options.sdkCommit,
    dummyBody,
    runHttp,
    runSocket,
  };
}
