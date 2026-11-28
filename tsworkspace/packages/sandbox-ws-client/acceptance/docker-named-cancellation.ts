import { createConnection } from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

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
  SANDBOX_DOCKER_NAMED_ACCEPTANCE: z.literal('1'),
  SANDBOX_ACCEPTANCE_TENANT: z.string().min(1),
  SANDBOX_GATEWAY_HTTP_URL: z.url(),
  SANDBOX_GATEWAY_WS_URL: z.string().regex(/^wss?:\/\//),
  SANDBOX_API_KEY: z.string().min(1),
  SANDBOX_ACCEPTANCE_IMAGE_REGISTRY: z.string().default('docker.io'),
  SANDBOX_ACCEPTANCE_IMAGE: z.string().min(1).default('library/node'),
  SANDBOX_ACCEPTANCE_IMAGE_TAG: z.string().min(1).default('22-bookworm-slim'),
  SANDBOX_ACCEPTANCE_NATIVE_OBSERVER_SOCKET: z
    .string()
    .min(1)
    .max(103)
    .startsWith('/')
    .refine((value) => ![...value].some((char) => char === '\0' || char === '\r' || char === '\n'))
    .optional(),
});
const grantSchema = z.object({
  lease_id: z.string().min(1),
  provider: z.literal('local_docker'),
  sandbox_id: z.string().min(1),
  created: z.literal(true),
});
const originSchema = z.enum(['created', 'reused', 'unknown']);
const launchSchema = z.object({ name: z.string(), executionId: z.string().min(1), origin: originSchema });
const identitySchema = z.object({ pid: z.number().int().positive(), startTime: z.string().regex(/^\d+$/) });
const nativeSchema = z.object({
  ready: z.boolean(),
  records: z.array(identitySchema.extend({ marker: z.string() })).max(2),
  members: z.array(identitySchema).max(4096),
  tracked: z.array(identitySchema.extend({ state: z.enum(['alive', 'absent']) })).max(4096),
  starts: z.array(z.string()).max(4096),
});
const nativeProbeDiagnosticSchema = z.object({
  phase: z.literal('native_family'),
  exitCode: z.number().int(),
  timedOut: z.boolean(),
  errorClass: z.enum([
    'syntax_error',
    'reference_error',
    'type_error',
    'access_denied',
    'operation_not_permitted',
    'missing_file',
    'missing_process',
    'census_bound',
    'other',
  ]),
  errnoClass: z.enum(['EACCES', 'EPERM', 'ENOENT', 'ESRCH', 'none', 'other']),
});
const rpcIdentitySchema = z.strictObject({
  pid: z.number().int().min(1).max(4_294_967_295),
  startTime: z.string().regex(/^\d{1,20}$/),
});
const nativeRpcRequestSchema = z.strictObject({
  kind: z.literal('native_family'),
  requestId: z.string().regex(/^[A-Za-z0-9_-]{1,64}$/),
  leaseId: z.string().regex(/^[A-Za-z0-9_-]{1,128}$/),
  containerId: z.string().regex(/^[a-fA-F0-9]{64}$/),
  marker: z.string().regex(/^(sentinel|owned)-[a-fA-F0-9]{32}$/),
  paths: z.tuple([z.string().max(512), z.string().max(512), z.string().max(512)]),
  tracked: z.array(rpcIdentitySchema).max(4096),
});
const nativeRpcStateSchema = z.strictObject({
  ready: z.boolean(),
  records: z.array(rpcIdentitySchema.extend({ marker: z.string().max(128) }).strict()).max(2),
  members: z.array(rpcIdentitySchema).max(4096),
  tracked: z.array(rpcIdentitySchema.extend({ state: z.enum(['alive', 'absent']) }).strict()).max(4096),
  starts: z.array(z.string().max(128)).max(4096),
});
const nativeRpcResponseSchema = z.discriminatedUnion('status', [
  z.strictObject({
    status: z.literal('observed'),
    request_id: z.string().regex(/^[A-Za-z0-9_-]{1,64}$/),
    native: nativeRpcStateSchema,
    proof: z.strictObject({
      observer: rpcIdentitySchema,
      init: rpcIdentitySchema.extend({ pid: z.literal(1) }).strict(),
      uid: z.literal(0),
      ptrace: z.literal(true),
      rootChildLinked: z.boolean(),
    }),
    observer_container: z.string().regex(/^[a-fA-F0-9]{64}$/),
    observer_absent: z.literal(true),
  }),
  z.strictObject({
    status: z.literal('refused'),
    request_id: z.string().max(64),
    code: z.enum(['invalid_input', 'request_scope', 'namespace_changed', 'native_binding_changed', 'observer_changed']),
  }),
  z.strictObject({
    status: z.literal('failed'),
    request_id: z.string().max(64),
    code: z.enum([
      'deadline',
      'access_denied',
      'operation_not_permitted',
      'probe_failed',
      'output_invalid',
      'transport',
    ]),
  }),
]);
const nativeObserverSchema = z.object({
  mode: z.literal('independent_readonly_per_probe'),
  observations: z.number().int().nonnegative(),
  exactObserverAbsenceConfirmed: z.boolean(),
});
const caseNameSchema = z.enum(['fresh_named_lost_started', 'reused_named_lost_started']);
const caseSchema = z.object({
  name: caseNameSchema,
  status: z.enum(['passed', 'failed']),
  launchKey: z.string().optional(),
  executionId: z.string().optional(),
  origin: originSchema.optional(),
  suppressedFrames: z.number().int().nonnegative(),
  targetNative: nativeSchema.optional(),
  sentinelNative: nativeSchema.optional(),
  cancellationCode: z.string().optional(),
});
const requestSchema = z.looseObject({
  kind: z.literal('request'),
  id: z.string(),
  tool: z.string(),
  payload: z.looseObject({
    lease_id: z.string().optional(),
    launch_key: z.string().optional(),
    require_launch_cancellation: z.boolean().optional(),
    process: z.looseObject({ name: z.string() }).optional(),
  }),
});
const startedSchema = z.looseObject({
  kind: z.literal('response'),
  id: z.string(),
  result: z.looseObject({
    outcome: z.literal('started'),
    execution_id: z.string().min(1),
    origin: originSchema.default('unknown'),
  }),
});
const reportSchema = z.object({
  status: z.enum(['failed', 'checks_passed_native_cleanup_required']),
  phase: z.string(),
  checks: z.array(z.string()),
  grant: grantSchema.optional(),
  advertised: z.object({ named: z.boolean(), cancellation: z.boolean(), launchCancellation: z.boolean() }).optional(),
  ordinaryNativeControl: nativeSchema.optional(),
  nativeProbeDiagnostic: nativeProbeDiagnosticSchema.optional(),
  nativeObserver: nativeObserverSchema.optional(),
  missingFeatureProbe: z
    .object({
      errorCode: z.string(),
      namedRequestsBefore: z.number().int().nonnegative(),
      namedRequestsAfter: z.number().int().nonnegative(),
      native: nativeSchema,
      sentinelRunning: z.boolean(),
    })
    .optional(),
  launches: z.array(launchSchema),
  cases: z.array(caseSchema),
  suppressedFrames: z.number().int().nonnegative(),
  transportConnections: z.number().int().nonnegative(),
  brokerDeleted: z.boolean(),
  nativeCleanupVerified: z.literal(false),
  cleanupErrors: z.array(z.string()),
  nodeVersion: z.string(),
  errorCode: z
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

function processScript(): string {
  const child = `const fs=require('node:fs');process.on('SIGTERM',()=>{});const f=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,startTime:f.slice(f.lastIndexOf(')')+1).trim().split(/\\s+/)[19],marker:process.argv[2]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);setInterval(()=>{},1000);`;
  return `const fs=require('node:fs');process.on('SIGTERM',()=>{});require('node:child_process').spawn(process.execPath,['-e',${JSON.stringify(child)},process.argv[2],process.argv[4]],{stdio:'ignore'});const f=fs.readFileSync('/proc/self/stat','utf8');fs.appendFileSync(process.argv[3],process.argv[4]+'\\n');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,startTime:f.slice(f.lastIndexOf(')')+1).trim().split(/\\s+/)[19],marker:process.argv[4]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);process.stdout.write(process.argv[4]);setInterval(()=>{},1000);`;
}

const probeScript = `const fs=require('node:fs');const paths=JSON.parse(process.argv[1]),marker=process.argv[2],extra=JSON.parse(process.argv[3]);const readStat=(pid)=>{try{const value=fs.readFileSync('/proc/'+pid+'/stat','utf8');return {pid:Number(pid),startTime:value.slice(value.lastIndexOf(')')+1).trim().split(/\\s+/)[19]};}catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')return undefined;throw error;}};const ready=paths.slice(0,2).every(path=>fs.existsSync(path));const records=ready?paths.slice(0,2).map(path=>JSON.parse(fs.readFileSync(path,'utf8'))):[];const pids=fs.readdirSync('/proc').filter(value=>/^\\d+$/.test(value));if(pids.length>4096)throw Error('native_process_census_exceeded');const members=[];for(const pid of pids){let env;try{env=fs.readFileSync('/proc/'+pid+'/environ').toString().split('\\0');}catch(error){if(error.code==='ENOENT'||error.code==='ESRCH')continue;throw error;}if(env.includes('SANDBOX_ACCEPTANCE_FAMILY='+marker)){const identity=readStat(pid);if(identity)members.push(identity);}}const pinned=new Map([...records,...extra].map(identity=>[identity.pid+':'+identity.startTime,identity]));const tracked=[...pinned.values()].map(identity=>{const observed=readStat(identity.pid);return {pid:identity.pid,startTime:identity.startTime,state:observed&&observed.startTime===identity.startTime?'alive':'absent'};});const starts=fs.existsSync(paths[2])?fs.readFileSync(paths[2],'utf8').trim().split('\\n').filter(Boolean):[];process.stdout.write(JSON.stringify({ready,records,members,tracked,starts}));`;

type Paths = [string, string, string];
type Identity = z.output<typeof identitySchema>;

async function nativeRpc(paths: Paths, marker: string, tracked: Identity[]): Promise<z.output<typeof nativeSchema>> {
  requireThat(nativeObserverSocket !== undefined && grant !== undefined, 'native_observer_binding_missing');
  const request = nativeRpcRequestSchema.parse({
    kind: 'native_family',
    requestId: crypto.randomUUID(),
    leaseId: grant.lease_id,
    containerId: grant.sandbox_id,
    marker,
    paths,
    tracked,
  });
  const body = Buffer.from(JSON.stringify(request));
  requireThat(body.length <= 512 * 1024, 'native_observer_input_bound_exceeded');
  observerTeardownConfirmed = false;
  const response = await new Promise<z.output<typeof nativeRpcResponseSchema>>((resolve, reject) => {
    const connection = createConnection({ path: nativeObserverSocket! });
    const chunks: Buffer[] = [];
    let length = 0;
    let settled = false;
    const timer = setTimeout(() => fail('native_observer_deadline'), 5000);
    function fail(code: string): void {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      connection.destroy();
      reject(new Error(code));
    }
    connection.once('connect', () => connection.end(body));
    connection.on('data', (bytes: Buffer) => {
      length += bytes.length;
      if (length > 1024 * 1024) {
        fail('native_observer_output_bound_exceeded');
        return;
      }
      chunks.push(bytes);
    });
    connection.once('error', () => fail('native_observer_transport_failed'));
    connection.once('end', () => {
      if (settled) return;
      try {
        const result = nativeRpcResponseSchema.parse(JSON.parse(Buffer.concat(chunks).toString()));
        requireThat(result.request_id === request.requestId, 'native_observer_response_mismatch');
        settled = true;
        clearTimeout(timer);
        connection.destroy();
        resolve(result);
      } catch {
        fail('native_observer_output_invalid');
      }
    });
    connection.once('close', () => {
      if (!settled) fail('native_observer_closed_early');
    });
  });
  if (response.status !== 'observed') throw new Error(`native_observer_${response.code}`);
  observerTeardownConfirmed = true;
  nativeObserverObservations += 1;
  return nativeSchema.parse(response.native);
}

function classifyNativeProbeFailure(
  stderr: string,
): Pick<z.output<typeof nativeProbeDiagnosticSchema>, 'errorClass' | 'errnoClass'> {
  const tail = stderr.slice(-8192);
  for (const [name, errorClass] of [
    ['SyntaxError', 'syntax_error'],
    ['ReferenceError', 'reference_error'],
    ['TypeError', 'type_error'],
  ] as const) {
    if (new RegExp(`(?:^|\\n)${name}:`).test(tail)) return { errorClass, errnoClass: 'none' };
  }
  const code =
    /(?:^|\n)\s*code:\s*['"]([A-Z][A-Z0-9_]{1,31})['"]/.exec(tail)?.[1] ??
    /(?:^|\n)Error:\s*(EACCES|EPERM|ENOENT|ESRCH)(?=[:\s])/.exec(tail)?.[1];
  switch (code) {
    case 'EACCES':
      return { errorClass: 'access_denied', errnoClass: code };
    case 'EPERM':
      return { errorClass: 'operation_not_permitted', errnoClass: code };
    case 'ENOENT':
      return { errorClass: 'missing_file', errnoClass: code };
    case 'ESRCH':
      return { errorClass: 'missing_process', errnoClass: code };
    default:
      return {
        errorClass: /(?:^|\n)Error: native_process_census_exceeded(?:\n|$)/.test(tail) ? 'census_bound' : 'other',
        errnoClass: code === undefined ? 'none' : 'other',
      };
  }
}

async function nativeFamily(
  sandbox: Sandbox,
  paths: Paths,
  marker: string,
  tracked: Identity[] = [],
): Promise<z.output<typeof nativeSchema>> {
  const deadline = Date.now() + 10_000;
  while (true) {
    let state: z.output<typeof nativeSchema>;
    if (nativeObserverSocket !== undefined) {
      state = await bounded(nativeRpc(paths, marker, tracked), 5000);
    } else {
      const output = await bounded(
        sandbox.runCommand({
          cmd: 'node',
          args: ['-e', probeScript, JSON.stringify(paths), marker, JSON.stringify(tracked)],
        }),
        5000,
      );
      if (output.exitCode !== 0 || output.timedOut) {
        nativeProbeDiagnostic = nativeProbeDiagnosticSchema.parse({
          phase: 'native_family',
          exitCode: output.exitCode,
          timedOut: output.timedOut,
          ...classifyNativeProbeFailure(output.stderr()),
        });
      }
      output.throwIfFailed();
      state = nativeSchema.parse(JSON.parse(output.stdout()));
    }
    if (state.ready) {
      requireThat(
        state.records.length === 2 && state.records.every((identity) => identity.marker === marker),
        'native_family_identity_missing',
      );
      requireThat(state.records[0]!.pid !== state.records[1]!.pid, 'native_child_identity_not_distinct');
      return state;
    }
    requireThat(Date.now() < deadline, 'native_family_did_not_start');
    await delay(100);
  }
}

function requireLive(state: z.output<typeof nativeSchema>, marker: string): void {
  requireThat(state.starts.length === 1 && state.starts[0] === marker, 'native_user_effect_duplicated');
  requireThat(
    state.tracked.length >= 2 && state.tracked.every((identity) => identity.state === 'alive'),
    'native_family_not_alive',
  );
  requireThat(
    state.records.every((identity) =>
      state.members.some((member) => member.pid === identity.pid && member.startTime === identity.startTime),
    ),
    'native_family_marker_control_failed',
  );
}

function requireSameFamily(before: z.output<typeof nativeSchema>, after: z.output<typeof nativeSchema>): void {
  const identities = (state: z.output<typeof nativeSchema>) =>
    state.members.map((identity) => `${identity.pid}:${identity.startTime}`).sort();
  requireThat(
    JSON.stringify(identities(before)) === JSON.stringify(identities(after)),
    'named_native_family_census_changed',
  );
}

type Loss = {
  name: z.output<typeof caseNameSchema>;
  processName: string;
  requestId?: string;
  launchKey?: string;
  executionId?: string;
  origin?: z.output<typeof originSchema>;
  suppressedFrames: number;
  observed: Promise<void>;
  resolve: () => void;
};
let activeLoss: Loss | undefined;
let suppressedFrames = 0;
let transportConnections = 0;
let namedWireRequests = 0;
const RealWebSocket = globalThis.WebSocket;

class LostNamedStartedWebSocket extends RealWebSocket {
  constructor(url: string | URL, protocols?: string | string[]) {
    super(url, protocols);
    transportConnections += 1;
    this.addEventListener('message', (event: MessageEvent) => {
      if (typeof event.data !== 'string' || activeLoss?.requestId === undefined) return;
      let value: unknown;
      try {
        value = JSON.parse(event.data);
      } catch {
        return;
      }
      const parsed = startedSchema.safeParse(value);
      if (!parsed.success || parsed.data.id !== activeLoss.requestId) return;
      event.stopImmediatePropagation();
      activeLoss.suppressedFrames += 1;
      suppressedFrames += 1;
      activeLoss.executionId = parsed.data.result.execution_id;
      activeLoss.origin = parsed.data.result.origin;
      activeLoss.resolve();
    });
  }

  override send(data: Parameters<WebSocket['send']>[0]): void {
    if (typeof data === 'string') {
      let value: unknown;
      try {
        value = JSON.parse(data);
      } catch {
        value = undefined;
      }
      const parsed = requestSchema.safeParse(value);
      if (
        parsed.success &&
        parsed.data.tool === 'sandbox_run_command' &&
        parsed.data.payload.process?.name !== undefined
      )
        namedWireRequests += 1;
      if (
        parsed.success &&
        parsed.data.tool === 'sandbox_run_command' &&
        parsed.data.payload.require_launch_cancellation === true &&
        activeLoss !== undefined
      ) {
        requireThat(activeLoss.requestId === undefined, 'unexpected_strong_named_request');
        requireThat(
          parsed.data.payload.process?.name === activeLoss.processName,
          'strong_request_changed_process_name',
        );
        activeLoss.requestId = parsed.data.id;
        activeLoss.launchKey = parsed.data.payload.launch_key;
        requireThat(Boolean(activeLoss.launchKey), 'missing_wire_launch_key');
      }
    }
    super.send(data);
  }
}

let phase = 'configuration';
const checks: string[] = [];
const launches: z.output<typeof launchSchema>[] = [];
const cases: z.output<typeof caseSchema>[] = [];
const cleanupErrors: string[] = [];
let client: SandboxWsClient | undefined;
let grant: z.output<typeof grantSchema> | undefined;
let advertised: z.output<typeof reportSchema>['advertised'];
let ordinaryNativeControl: z.output<typeof nativeSchema> | undefined;
let nativeProbeDiagnostic: z.output<typeof nativeProbeDiagnosticSchema> | undefined;
let nativeObserverSocket: string | undefined;
let nativeObserverObservations = 0;
let observerTeardownConfirmed = true;
let missingFeatureProbe: z.output<typeof reportSchema>['missingFeatureProbe'];
let brokerDeleted = false;
let failure: unknown;

try {
  const config = configSchema.parse(process.env);
  nativeObserverSocket = config.SANDBOX_ACCEPTANCE_NATIVE_OBSERVER_SOCKET;
  globalThis.WebSocket = LostNamedStartedWebSocket;
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
      token: 'invalid-docker-named-session',
      retry: false,
      requestTimeoutMs: 2000,
    });
    invalid.close();
  } catch {
    rejected = true;
  }
  requireThat(rejected, 'invalid_session_token_accepted');
  checks.push(phase);
  phase = 'actual_worker_readiness';
  await bounded(
    (async () => {
      while (true) {
        const providers = await client!.listCapabilities();
        if (providers.providers.some((provider) => provider.kind === 'local_docker' && provider.live_workers > 0))
          return;
        await delay(100);
      }
    })(),
    30_000,
  );
  checks.push(phase);
  const allocate = client.lease.bind(client);
  client.lease = async (...args: Parameters<SandboxWsClient['lease']>) => {
    const response = await allocate(...args);
    grant = grantSchema.parse(response);
    return response;
  };
  phase = 'owned_docker_lease';
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(config.SANDBOX_ACCEPTANCE_TENANT),
    providerPref: ['local_docker'],
    persistent: false,
    image: {
      kind: 'registry',
      reference: {
        registry: config.SANDBOX_ACCEPTANCE_IMAGE_REGISTRY,
        repository: config.SANDBOX_ACCEPTANCE_IMAGE,
        tag: config.SANDBOX_ACCEPTANCE_IMAGE_TAG,
      },
    },
    timeoutMs: 180_000,
  });
  requireThat(grant !== undefined && sandbox.provider === 'local_docker', 'missing_actual_owned_docker_grant');
  advertised = {
    named: sandbox.supports('named_processes'),
    cancellation: sandbox.supports('command_cancellation'),
    launchCancellation: sandbox.supports('command_launch_cancellation'),
  };
  requireThat(advertised.named, 'actual_worker_has_not_earned_named_processes');
  checks.push(phase);
  const nonce = crypto.randomUUID().replaceAll('-', '');
  const root = `/tmp/named-cancellation-${nonce}`;
  (await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', root] })).throwIfFailed();
  const sentinelName = `sentinel-${nonce}`;
  const targetName = `owned-${nonce}`;
  const pathsFor = (name: string): Paths => [
    `${root}/${name}.root.json`,
    `${root}/${name}.child.json`,
    `${root}/${name}.starts`,
  ];
  const sentinelPaths = pathsFor(sentinelName);
  const targetPaths = pathsFor(targetName);
  const paramsFor = (name: string, paths: Paths) => ({
    cmd: 'node',
    args: ['-e', processScript(), ...paths, name],
    env: { SANDBOX_ACCEPTANCE_FAMILY: name },
    process: { name },
    detached: true as const,
    deadline: 'lease' as const,
  });
  phase = 'ordinary_created_reused_native_controls';
  const sentinel = await sandbox.runCommand(paramsFor(sentinelName, sentinelPaths));
  launches.push(launchSchema.parse({ name: sentinelName, executionId: sentinel.cmdId, origin: sentinel.origin }));
  const firstNative = await nativeFamily(sandbox, sentinelPaths, sentinelName);
  requireLive(firstNative, sentinelName);
  const reused = await sandbox.runCommand(paramsFor(sentinelName, sentinelPaths));
  launches.push(launchSchema.parse({ name: sentinelName, executionId: reused.cmdId, origin: reused.origin }));
  const sentinelNative = await nativeFamily(sandbox, sentinelPaths, sentinelName, firstNative.members);
  ordinaryNativeControl = sentinelNative;
  requireLive(sentinelNative, sentinelName);
  requireSameFamily(firstNative, sentinelNative);
  requireThat(
    sentinel.origin === 'created' && reused.origin === 'reused' && sentinel.cmdId === reused.cmdId,
    'ordinary_named_origin_control_failed',
  );
  requireThat(
    (await client.pollCommand(sandbox.sandboxId, sentinel.cmdId)).outcome === 'running',
    'ordinary_named_sentinel_not_running',
  );
  checks.push(phase);
  phase = 'actual_named_provider_strong_cancellation';
  if (!advertised.cancellation || !advertised.launchCancellation) {
    const namedRequestsBefore = namedWireRequests;
    const controller = new AbortController();
    const request = sandbox.runCommand({
      ...paramsFor(sentinelName, sentinelPaths),
      signal: controller.signal,
      cancellationTimeoutMs: 2000,
    });
    let refusal: unknown;
    try {
      await bounded(request, 5000);
    } catch (error) {
      refusal = error;
    } finally {
      controller.abort(new Error('missing feature probe cleanup'));
      try {
        await bounded(
          request.then(
            () => undefined,
            () => undefined,
          ),
          3000,
        );
      } catch {
        cleanupErrors.push('missing_feature_probe_did_not_settle');
      }
    }
    const after = await bounded(nativeFamily(sandbox, sentinelPaths, sentinelName, sentinelNative.members), 15_000);
    const sentinelRunning =
      (await bounded(client.pollCommand(sandbox.sandboxId, sentinel.cmdId), 5000)).outcome === 'running';
    missingFeatureProbe = {
      errorCode:
        refusal instanceof SandboxWsError && /^[a-z_]{1,96}$/.test(refusal.code)
          ? refusal.code
          : refusal === undefined
            ? 'unexpected_acceptance'
            : 'untyped_failure',
      namedRequestsBefore,
      namedRequestsAfter: namedWireRequests,
      native: after,
      sentinelRunning,
    };
    requireThat(
      refusal instanceof SandboxWsError && refusal.code === 'unsupported',
      'actual_named_signal_refusal_not_typed_unsupported',
    );
    requireThat(namedWireRequests === namedRequestsBefore, 'unsupported_named_signal_was_dispatched');
    requireLive(after, sentinelName);
    requireSameFamily(sentinelNative, after);
    requireThat(sentinelRunning, 'missing_feature_probe_canceled_sentinel');
    checks.push('actual_public_named_signal_refused_without_effect');
  }
  requireThat(
    advertised.cancellation && advertised.launchCancellation,
    'actual_named_provider_has_not_earned_strong_cancellation',
  );
  checks.push(phase);

  for (const name of caseNameSchema.options) {
    phase = name;
    const fresh = name === 'fresh_named_lost_started';
    const processName = fresh ? targetName : sentinelName;
    const paths = fresh ? targetPaths : sentinelPaths;
    let resolve!: () => void;
    const loss: Loss = {
      name,
      processName,
      suppressedFrames: 0,
      observed: new Promise<void>((done) => {
        resolve = done;
      }),
      resolve: () => resolve(),
    };
    activeLoss = loss;
    const result: z.output<typeof caseSchema> = { name, status: 'failed', suppressedFrames: 0 };
    const controller = new AbortController();
    try {
      const waiting = sandbox
        .runCommand({ ...paramsFor(processName, paths), signal: controller.signal, cancellationTimeoutMs: 30_000 })
        .then(
          () => {
            throw new Error('named_strong_launch_finished_before_abort');
          },
          (error: unknown) => error,
        );
      await bounded(
        Promise.race([
          loss.observed,
          waiting.then(() => {
            throw new Error('named_launch_returned_before_started_loss');
          }),
        ]),
        20_000,
      );
      requireThat(
        loss.suppressedFrames === 1 && loss.executionId !== undefined && loss.launchKey !== undefined,
        'actual_named_started_frame_not_lost',
      );
      requireThat(loss.origin === (fresh ? 'created' : 'reused'), 'actual_named_started_origin_incorrect');
      requireThat(
        fresh ? loss.executionId !== sentinel.cmdId : loss.executionId === sentinel.cmdId,
        'named_started_execution_ownership_incorrect',
      );
      const before = await nativeFamily(sandbox, paths, processName, fresh ? [] : sentinelNative.members);
      requireLive(before, processName);
      controller.abort(new Error('named acceptance lost Started frame'));
      const error = await bounded(waiting, 35_000);
      requireThat(
        error instanceof CommandCancellationError &&
          !error.cleanupPending &&
          !error.outcomeUnknown &&
          error.launchKey === loss.launchKey,
        'named_abort_outcome_not_settled',
      );
      result.launchKey = loss.launchKey;
      result.executionId = loss.executionId;
      result.origin = loss.origin;
      result.suppressedFrames = loss.suppressedFrames;
      result.cancellationCode = error.code;
      if (fresh) {
        requireThat(
          error.code === 'command_cancelled' && !error.preserved && error.executionId === loss.executionId,
          'fresh_named_abort_not_owned_cancellation',
        );
        const deadline = Date.now() + 10_000;
        do {
          result.targetNative = await nativeFamily(sandbox, paths, processName, before.members);
          if (
            result.targetNative.members.length === 0 &&
            result.targetNative.tracked.every((identity) => identity.state === 'absent')
          )
            break;
          await delay(100);
        } while (Date.now() < deadline);
        requireThat(
          result.targetNative.members.length === 0 &&
            result.targetNative.tracked.every((identity) => identity.state === 'absent'),
          'fresh_named_native_family_survived_abort',
        );
        requireThat(
          result.targetNative.starts.length === 1 && result.targetNative.starts[0] === processName,
          'fresh_named_user_effect_replayed',
        );
        requireThat(
          (await client.pollCommand(sandbox.sandboxId, executionId(loss.executionId))).outcome === 'completed',
          'fresh_named_execution_not_completed',
        );
        const repeated = await client.cancelCommandLaunch(sandbox.sandboxId, loss.launchKey);
        requireThat(
          repeated.outcome === 'canceled' && repeated.execution_id === loss.executionId,
          'fresh_named_cancel_receipt_not_replayable',
        );
      } else {
        requireThat(
          error.code === 'command_preserved' && error.preserved && error.executionId === sentinel.cmdId,
          'reused_named_abort_did_not_report_preservation',
        );
        result.targetNative = await nativeFamily(sandbox, sentinelPaths, sentinelName, sentinelNative.members);
        requireLive(result.targetNative, sentinelName);
      }
      result.sentinelNative = await nativeFamily(sandbox, sentinelPaths, sentinelName, sentinelNative.members);
      requireLive(result.sentinelNative, sentinelName);
      requireSameFamily(sentinelNative, result.sentinelNative);
      requireThat(
        (await client.pollCommand(sandbox.sandboxId, sentinel.cmdId)).outcome === 'running',
        'unrelated_native_sentinel_was_canceled',
      );
      result.status = 'passed';
      checks.push(phase);
    } catch (error) {
      failure ??= error;
    } finally {
      controller.abort(new Error('owned acceptance fixture cleanup'));
      activeLoss = undefined;
      cases.push(caseSchema.parse(result));
    }
    if (failure !== undefined) break;
  }
} catch (error) {
  failure ??= error;
} finally {
  globalThis.WebSocket = RealWebSocket;
  if (client !== undefined && grant !== undefined) {
    try {
      const response = await bounded(client.deleteLease(grant.lease_id as LeaseId), 20_000);
      brokerDeleted = response.outcome === 'deleted';
      requireThat(brokerDeleted, 'broker_delete_not_confirmed');
    } catch (error) {
      cleanupErrors.push('owned_lease_cleanup_unconfirmed');
      failure ??= error;
    }
  }
  client?.close();
}

const errorCode =
  failure instanceof SandboxWsError ? failure.code : failure instanceof Error ? failure.message : undefined;
const report = reportSchema.parse({
  status: failure === undefined ? 'checks_passed_native_cleanup_required' : 'failed',
  phase,
  checks,
  grant,
  advertised,
  ordinaryNativeControl,
  nativeProbeDiagnostic,
  missingFeatureProbe,
  launches,
  cases,
  suppressedFrames,
  transportConnections,
  brokerDeleted,
  nativeCleanupVerified: false,
  cleanupErrors,
  nodeVersion: process.versions.node,
  nativeObserver:
    nativeObserverSocket === undefined
      ? undefined
      : {
          mode: 'independent_readonly_per_probe',
          observations: nativeObserverObservations,
          exactObserverAbsenceConfirmed: observerTeardownConfirmed,
        },
  errorCode:
    errorCode !== undefined && /^[a-z_]{1,96}$/.test(errorCode)
      ? errorCode
      : failure === undefined
        ? undefined
        : 'named_acceptance_failed',
});
process.stdout.write(`${JSON.stringify(report)}\n`);
if (failure !== undefined) process.exitCode = 1;
