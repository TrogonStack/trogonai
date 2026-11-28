import { setTimeout as delay } from 'node:timers/promises';

import { Sandbox, SandboxWsClient, SandboxWsError, TimeoutError, tenantId } from '@trogonai/sandbox-ws-client';
import type { LeaseId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const configurationSchema = z.object({
  SANDBOX_DOCKER_NAMED_ACCEPTANCE: z.literal('1'),
  SANDBOX_ACCEPTANCE_TENANT: z.string().min(1),
  SANDBOX_GATEWAY_HTTP_URL: z.url(),
  SANDBOX_GATEWAY_WS_URL: z.string().regex(/^wss?:\/\//),
  SANDBOX_API_KEY: z.string().min(1),
  SANDBOX_ACCEPTANCE_IMAGE_REGISTRY: z.string().default('docker.io'),
  SANDBOX_ACCEPTANCE_IMAGE: z.string().min(1).default('library/node'),
  SANDBOX_ACCEPTANCE_IMAGE_TAG: z.string().min(1).default('22-bookworm-slim'),
});
const grantSchema = z.object({
  lease_id: z.string().min(1),
  provider: z.literal('local_docker'),
  sandbox_id: z.string().min(1),
  created: z.boolean().optional(),
});
const launchSchema = z.object({
  name: z.string().min(1),
  executionId: z.string().min(1),
  origin: z.enum(['created', 'reused', 'unknown']),
});
const nativeIdentitySchema = z.object({
  pid: z.number().int().positive(),
  startTime: z.string().regex(/^\d+$/),
  marker: z.string().min(1),
  state: z.enum(['alive', 'absent']),
});
const caseSchema = z.object({
  name: z.enum(['created_named_setup_cleanup', 'reused_named_setup_preservation']),
  status: z.enum(['passed', 'failed']),
  setupFailureCode: z.string().optional(),
  executionId: z.string().min(1).optional(),
  origin: z.enum(['created', 'reused', 'unknown']).optional(),
  targetNative: nativeIdentitySchema.optional(),
  sentinelNative: nativeIdentitySchema.optional(),
  sentinelRunning: z.boolean().optional(),
  targetCompleted: z.boolean().optional(),
});
const reportSchema = z.object({
  status: z.enum(['checks_passed_native_cleanup_required', 'failed']),
  phase: z.string(),
  grant: grantSchema.optional(),
  brokerDeleted: z.boolean(),
  launches: z.array(launchSchema),
  cases: z.array(caseSchema),
  exactOwnedFallbackExecutionIds: z.array(z.string().min(1)),
  nativeCleanupVerified: z.literal(false),
  errorCode: z.string().optional(),
});

function requireThat(value: unknown): asserts value {
  if (!value) throw new Error('acceptance assertion failed');
}

function processScript(): string {
  return `const fs=require('node:fs');process.on('SIGTERM',()=>{});const stat=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(process.argv[1],JSON.stringify({pid:process.pid,startTime:stat.slice(stat.lastIndexOf(')')+1).trim().split(/\\s+/)[19],marker:process.argv[2]}));process.stdout.write(process.argv[2]);setInterval(()=>{},1000);`;
}

async function nativeState(
  sandbox: Sandbox,
  path: string,
  marker: string,
): Promise<z.output<typeof nativeIdentitySchema>> {
  const deadline = Date.now() + 10_000;
  while (true) {
    const bytes = await sandbox.readFileToBuffer({ path });
    if (bytes !== null) {
      const identity = nativeIdentitySchema.omit({ state: true }).parse(JSON.parse(new TextDecoder().decode(bytes)));
      requireThat(identity.marker === marker);
      const output = await sandbox.runCommand({
        cmd: 'node',
        args: [
          '-e',
          `const fs=require('node:fs');let live;try{const stat=fs.readFileSync('/proc/'+process.argv[1]+'/stat','utf8');live=stat.slice(stat.lastIndexOf(')')+1).trim().split(/\\s+/)[19]===process.argv[2];}catch(error){if(error.code!=='ENOENT')throw error;live=false;}process.stdout.write(live?'alive':'absent');`,
          String(identity.pid),
          identity.startTime,
        ],
      });
      output.throwIfFailed();
      return nativeIdentitySchema.parse({ ...identity, state: output.stdout() });
    }
    requireThat(Date.now() < deadline);
    await delay(100);
  }
}

const launches: z.output<typeof launchSchema>[] = [];
const cases: z.output<typeof caseSchema>[] = [];
const exactOwnedFallbackExecutionIds: string[] = [];
let client: SandboxWsClient | undefined;
let leaseId: LeaseId | undefined;
let grant: z.output<typeof grantSchema> | undefined;
let brokerDeleted = false;
let failure: unknown;
let phase = 'configuration';

try {
  const config = configurationSchema.parse(process.env);
  phase = 'authenticated_connection';
  client = await SandboxWsClient.connect({
    gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
    gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
    apiKey: config.SANDBOX_API_KEY,
    requestTimeoutMs: 30_000,
    retry: false,
  });
  const allocate = client.lease.bind(client);
  client.lease = async (...args: Parameters<SandboxWsClient['lease']>) => {
    const response = await allocate(...args);
    grant = grantSchema.parse(response);
    if (grant.created === true) leaseId = response.lease_id;
    return response;
  };
  const dispatch = client.runCommand.bind(client);
  client.runCommand = async (...args: Parameters<SandboxWsClient['runCommand']>) => {
    const response = await dispatch(...args);
    const name = args[2]?.process?.name;
    if (name !== undefined && response.outcome === 'started') {
      launches.push(
        launchSchema.parse({ name, executionId: response.execution_id, origin: response.origin ?? 'unknown' }),
      );
    }
    return response;
  };
  phase = 'owned_docker_lease';
  const sandbox = await Sandbox.create(client, {
    tenant: tenantId(config.SANDBOX_ACCEPTANCE_TENANT),
    providerPref: ['local_docker'],
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
  requireThat(grant?.created === true && sandbox.provider === 'local_docker');
  requireThat(sandbox.supports('named_processes'));
  const nonce = crypto.randomUUID().replaceAll('-', '');
  const root = `/tmp/provider-named-cleanup-${nonce}`;
  (await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', root] })).throwIfFailed();
  const sentinelName = `sentinel-${nonce}`;
  const targetName = `owned-${nonce}`;
  const sentinelPath = `${root}/sentinel.json`;
  const targetPath = `${root}/target.json`;
  const sentinelArgs = ['-e', processScript(), sentinelPath, sentinelName];
  await sandbox.start({ cmd: 'node', args: sentinelArgs, name: sentinelName });
  const sentinel = launches.find((launch) => launch.name === sentinelName);
  requireThat(sentinel?.origin === 'created');
  requireThat((await nativeState(sandbox, sentinelPath, sentinelName)).state === 'alive');
  const unused = await sandbox.runCommand({
    cmd: 'node',
    args: [
      '-e',
      `const server=require('node:net').createServer();server.listen(0,'127.0.0.1',()=>{process.stdout.write(String(server.address().port));server.close();});`,
    ],
  });
  unused.throwIfFailed();
  const unusedPort = z.coerce.number().int().min(1).max(65535).parse(unused.stdout());

  for (const name of ['created_named_setup_cleanup', 'reused_named_setup_preservation'] as const) {
    phase = name;
    const result: z.output<typeof caseSchema> = { name, status: 'failed' };
    try {
      const processName = name === 'created_named_setup_cleanup' ? targetName : sentinelName;
      const args =
        name === 'created_named_setup_cleanup' ? ['-e', processScript(), targetPath, targetName] : sentinelArgs;
      let setupFailure: unknown;
      try {
        await sandbox.startServer({
          cmd: 'node',
          args,
          name: processName,
          port: unusedPort,
          preview: false,
          timeoutMs: 2000,
        });
      } catch (error: unknown) {
        setupFailure = error;
      }
      if (setupFailure instanceof SandboxWsError) result.setupFailureCode = setupFailure.code;
      const launched = launches.filter((launch) => launch.name === processName).at(-1);
      requireThat(launched !== undefined);
      result.executionId = launched.executionId;
      result.origin = launched.origin;
      requireThat(setupFailure instanceof TimeoutError && setupFailure.executionId === launched.executionId);
      result.sentinelRunning =
        (await client.pollCommand(sandbox.sandboxId, sentinel.executionId)).outcome === 'running';
      result.sentinelNative = await nativeState(sandbox, sentinelPath, sentinelName);
      if (name === 'created_named_setup_cleanup') {
        requireThat(launched.origin === 'created' && launched.executionId !== sentinel.executionId);
        const cleanupDeadline = Date.now() + 10_000;
        do {
          result.targetCompleted =
            (await client.pollCommand(sandbox.sandboxId, launched.executionId)).outcome === 'completed';
          result.targetNative = await nativeState(sandbox, targetPath, targetName);
          if (result.targetCompleted && result.targetNative.state === 'absent') break;
          await delay(100);
        } while (Date.now() < cleanupDeadline);
        requireThat(result.targetCompleted && result.targetNative.state === 'absent');
      } else {
        requireThat(launched.origin === 'reused' && launched.executionId === sentinel.executionId);
      }
      requireThat(result.sentinelRunning && result.sentinelNative.state === 'alive');
      result.status = 'passed';
    } catch (error: unknown) {
      failure ??= error;
    }
    cases.push(caseSchema.parse(result));
  }
} catch (error: unknown) {
  failure ??= error;
} finally {
  if (client !== undefined && leaseId !== undefined) {
    for (const launch of launches) {
      if (launch.origin !== 'created') continue;
      exactOwnedFallbackExecutionIds.push(launch.executionId);
      try {
        await client.killCommand(leaseId, launch.executionId, 'kill');
      } catch (error: unknown) {
        failure ??= error;
      }
    }
    try {
      await client.deleteLease(leaseId);
      brokerDeleted = true;
    } catch (error: unknown) {
      failure ??= error;
    }
  }
  client?.close();
}

const report = reportSchema.parse({
  status: failure === undefined ? 'checks_passed_native_cleanup_required' : 'failed',
  phase,
  grant,
  brokerDeleted,
  launches,
  cases,
  exactOwnedFallbackExecutionIds,
  nativeCleanupVerified: false,
  ...(failure === undefined
    ? {}
    : { errorCode: failure instanceof SandboxWsError ? failure.code : 'acceptance_failed' }),
});
process.stdout.write(`${JSON.stringify(report)}\n`);
if (failure !== undefined) process.exitCode = 1;
