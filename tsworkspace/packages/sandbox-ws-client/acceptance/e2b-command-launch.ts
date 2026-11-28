import {
  CommandCancellationError,
  Sandbox,
  SandboxWsClient,
  SandboxWsError,
  executionId,
  tenantId,
} from '@trogonai/sandbox-ws-client';
import type { LeaseId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const configSchema = z.object({
  SANDBOX_E2B_COMPOSED_LAUNCH: z.literal('1'),
  SANDBOX_ACCEPTANCE_TENANT: z.string().min(1),
  SANDBOX_GATEWAY_HTTP_URL: z.url(),
  SANDBOX_GATEWAY_WS_URL: z.string().regex(/^wss?:\/\//),
  SANDBOX_API_KEY: z.string().min(1),
});
const grantSchema = z.object({
  lease_id: z.string().min(1),
  provider: z.literal('e2b'),
  sandbox_id: z.string().min(1),
  created: z.literal(true),
});
const requestSchema = z.looseObject({
  kind: z.literal('request'),
  id: z.string(),
  tool: z.string(),
  payload: z.looseObject({
    lease_id: z.string().optional(),
    launch_key: z.string().optional(),
    require_launch_cancellation: z.boolean().optional(),
  }),
});
const startedSchema = z.looseObject({
  kind: z.literal('response'),
  id: z.string(),
  result: z.looseObject({ outcome: z.literal('started'), execution_id: z.string().min(1) }),
});
const reportSchema = z.object({
  status: z.enum(['failed', 'checks_passed_native_cleanup_required']),
  phase: z.string(),
  checks: z.array(z.string()),
  grant: grantSchema.optional(),
  advertisedLaunchCancellation: z.boolean().optional(),
  sentinelId: z.string().optional(),
  suppressedStartedId: z.string().optional(),
  launchKey: z.string().optional(),
  suppressedFrames: z.number().int().nonnegative(),
  transportConnections: z.number().int().nonnegative(),
  nodeVersion: z.string(),
  brokerDeleted: z.boolean(),
  nativeCleanupVerified: z.literal(false),
  errorCode: z
    .string()
    .regex(/^[a-z_]{1,96}$/)
    .optional(),
  errorVariant: z
    .string()
    .regex(/^[a-z_]{1,96}$/)
    .optional(),
});

function requireThat(condition: unknown, code: string): asserts condition {
  if (!condition) throw new Error(code);
}

async function bounded<T>(work: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error('acceptance_bound_exceeded')), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

let phase = 'configuration';
const checks: string[] = [];
let grant: z.output<typeof grantSchema> | undefined;
let advertisedLaunchCancellation: boolean | undefined;
let client: SandboxWsClient | undefined;
let sandbox: Sandbox | undefined;
let sentinelId: string | undefined;
let suppressedStartedId: string | undefined;
let launchKey: string | undefined;
let pendingRequest: string | undefined;
let suppressedFrames = 0;
let transportConnections = 0;
let brokerDeleted = false;
let failure: unknown;
let observedStarted!: () => void;
const suppressed = new Promise<void>((resolve) => {
  observedStarted = resolve;
});
const RealWebSocket = globalThis.WebSocket;

class LostStartedWebSocket extends RealWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    super(url, protocols);
    transportConnections += 1;
    this.addEventListener('message', (event: MessageEvent) => {
      if (typeof event.data !== 'string' || pendingRequest === undefined) return;
      const parsed = startedSchema.safeParse(JSON.parse(event.data));
      if (!parsed.success || parsed.data.id !== pendingRequest) return;
      requireThat(suppressedFrames === 0, 'more_than_one_started_frame');
      event.stopImmediatePropagation();
      suppressedFrames += 1;
      suppressedStartedId = parsed.data.result.execution_id;
      observedStarted();
    });
  }

  override send(data: Parameters<WebSocket['send']>[0]): void {
    if (typeof data === 'string') {
      const parsed = requestSchema.safeParse(JSON.parse(data));
      if (
        parsed.success &&
        parsed.data.tool === 'sandbox_run_command' &&
        parsed.data.payload.require_launch_cancellation === true
      ) {
        requireThat(pendingRequest === undefined, 'more_than_one_strong_launch');
        pendingRequest = parsed.data.id;
        launchKey = parsed.data.payload.launch_key;
        requireThat(Boolean(launchKey), 'missing_wire_launch_key');
      }
    }
    super.send(data);
  }
}

try {
  const config = configSchema.parse(process.env);
  globalThis.WebSocket = LostStartedWebSocket;
  phase = 'authenticated_session';
  client = await SandboxWsClient.connect({
    gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
    gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
    apiKey: config.SANDBOX_API_KEY,
    retry: false,
    requestTimeoutMs: 60_000,
  });
  requireThat(transportConnections > 0, 'sdk_did_not_use_actual_loss_transport');
  let rejected = false;
  try {
    const invalid = await SandboxWsClient.connectWithToken({
      gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
      token: 'invalid-composed-launch-token',
      retry: false,
      requestTimeoutMs: 2_000,
    });
    invalid.close();
  } catch {
    rejected = true;
  }
  requireThat(rejected, 'invalid_session_token_was_accepted');
  checks.push(phase);
  phase = 'actual_worker_readiness';
  await bounded(
    (async () => {
      while (true) {
        const capabilities = await client!.listCapabilities();
        if (capabilities.providers.some((provider) => provider.kind === 'e2b' && provider.live_workers > 0)) return;
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
    })(),
    30_000,
  );
  checks.push(phase);
  phase = 'secure_lease';
  const allocate = client.lease.bind(client);
  client.lease = async (...args: Parameters<SandboxWsClient['lease']>) => {
    const response = await allocate(...args);
    grant = grantSchema.parse(response);
    return response;
  };
  sandbox = await Sandbox.create(client, {
    tenant: tenantId(config.SANDBOX_ACCEPTANCE_TENANT),
    providerPref: ['e2b'],
    image: { kind: 'registry', reference: { registry: '', repository: 'base' } },
    persistent: false,
    timeoutMs: 60_000,
  });
  requireThat(grant !== undefined, 'missing_owned_native_grant');
  checks.push(phase);
  phase = 'ordinary_native_command';
  const ordinary = await sandbox.runCommand({ cmd: 'printf', args: ['composed-native-ready'], user: 'root' });
  requireThat(
    ordinary.exitCode === 0 && ordinary.stdout() === 'composed-native-ready',
    'ordinary_native_command_failed',
  );
  const sentinel = await sandbox.runCommand({
    cmd: 'sleep',
    args: ['60'],
    user: 'root',
    detached: true,
    deadline: 'lease',
  });
  sentinelId = sentinel.cmdId;
  requireThat(
    (await client.pollCommand(sandbox.sandboxId, sentinel.cmdId)).outcome === 'running',
    'native_sentinel_not_running',
  );
  checks.push(phase);
  phase = 'actual_advertised_strong_launch';
  advertisedLaunchCancellation = sandbox.supports('command_launch_cancellation');
  requireThat(
    advertisedLaunchCancellation && sandbox.supports('command_cancellation'),
    'actual_worker_has_not_earned_launch_cancellation',
  );
  checks.push(phase);
  phase = 'lost_started_response';
  const controller = new AbortController();
  const waiting = sandbox
    .runCommand({
      cmd: 'bash',
      args: ['-c', 'echo launched >>/tmp/composed-launch-count; sleep 60 & wait'],
      user: 'root',
      signal: controller.signal,
      cancellationTimeoutMs: 30_000,
    })
    .then(
      () => {
        throw new Error('strong_launch_finished_before_abort');
      },
      (error: unknown) => error,
    );
  await bounded(
    Promise.race([
      suppressed,
      waiting.then(() => {
        throw new Error('strong_launch_returned_before_suppressed_started');
      }),
    ]),
    20_000,
  );
  requireThat(
    suppressedFrames === 1 && suppressedStartedId !== undefined && launchKey !== undefined,
    'missing_actual_started_loss',
  );
  controller.abort(new Error('composed missing Started response'));
  const error = await bounded(waiting, 35_000);
  requireThat(
    error instanceof CommandCancellationError && !error.cleanupPending && error.launchKey === launchKey,
    'launch_cancellation_did_not_settle',
  );
  requireThat(
    (await client.pollCommand(sandbox.sandboxId, executionId(suppressedStartedId))).outcome === 'completed',
    'canceled_native_command_not_completed',
  );
  requireThat(
    (await client.pollCommand(sandbox.sandboxId, sentinel.cmdId)).outcome === 'running',
    'unrelated_native_sentinel_was_canceled',
  );
  checks.push(phase);
} catch (error) {
  failure = error;
} finally {
  globalThis.WebSocket = RealWebSocket;
  if (client !== undefined && grant !== undefined) {
    try {
      const response = await bounded(client.deleteLease(grant.lease_id as LeaseId), 20_000);
      brokerDeleted = response.outcome === 'deleted';
      requireThat(brokerDeleted, 'broker_delete_not_confirmed');
    } catch (error) {
      failure ??= error;
    }
  }
  client?.close();
}

const report = reportSchema.parse({
  status: failure === undefined ? 'checks_passed_native_cleanup_required' : 'failed',
  phase,
  checks,
  grant,
  advertisedLaunchCancellation,
  sentinelId,
  suppressedStartedId,
  launchKey,
  suppressedFrames,
  brokerDeleted,
  nativeCleanupVerified: false,
  transportConnections,
  nodeVersion: process.versions.node,
  errorCode:
    failure instanceof SandboxWsError && /^[a-z_]{1,96}$/.test(failure.code)
      ? failure.code
      : failure instanceof Error && /^[a-z_]{1,96}$/.test(failure.message)
        ? failure.message
        : failure === undefined
          ? undefined
          : 'composed_acceptance_failed',
  errorVariant:
    failure instanceof SandboxWsError && failure.variant !== undefined && /^[a-z_]{1,96}$/.test(failure.variant)
      ? failure.variant
      : undefined,
});
process.stdout.write(`${JSON.stringify(report)}\n`);
if (failure !== undefined) process.exitCode = 1;
