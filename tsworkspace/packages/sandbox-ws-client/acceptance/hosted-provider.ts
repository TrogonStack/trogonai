import { setTimeout as delay } from 'node:timers/promises';

import {
  CommandCancellationError,
  Sandbox,
  SandboxWsClient,
  SandboxWsError,
  nativeSnapshotReferenceSchema,
  providerKindSchema,
  tenantId,
} from '@trogonai/sandbox-ws-client';
import type { LeaseId, NativeSnapshotRef, Terminal, TerminalChunk } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const configurationSchema = z.object({
  SANDBOX_HOSTED_ACCEPTANCE: z.literal('1'),
  SANDBOX_ACCEPTANCE_PROVIDER: z.enum(['e2b', 'daytona', 'vercel']),
  SANDBOX_ACCEPTANCE_IMAGE: z.string().min(1),
  SANDBOX_ACCEPTANCE_IMAGE_REGISTRY: z.string().default(''),
  SANDBOX_ACCEPTANCE_IMAGE_TAG: z.string().min(1).optional(),
  SANDBOX_ACCEPTANCE_TENANT: z.string().min(1),
  SANDBOX_GATEWAY_HTTP_URL: z.url(),
  SANDBOX_GATEWAY_WS_URL: z.string().regex(/^wss?:\/\//),
  SANDBOX_API_KEY: z.string().min(1),
  SANDBOX_ACCEPTANCE_LONG_IDLE_SECONDS: z.coerce.number().int().min(70).max(600).default(90),
});
const grantSchema = z.looseObject({
  lease_id: z.string().min(1),
  provider: providerKindSchema,
  sandbox_id: z.string().min(1),
  created: z.boolean().optional(),
});
const recordedGrantSchema = z.object({
  leaseId: z.string().min(1),
  provider: providerKindSchema,
  nativeId: z.string().min(1),
  created: z.boolean(),
  brokerDeleted: z.boolean(),
});
const overflowEvidenceSchema = z.object({
  watch_id: z.string().min(1),
  dropped_through: z.number().int().positive(),
});
const trackingBoundEvidenceSchema = z.object({
  leaseId: z.string().min(1),
  path: z.string().startsWith('/home/user/provider-acceptance-large-'),
  entriesObserved: z.literal(100_001),
  maxEntries: z.literal(100_000),
  maxManifestBytes: z.literal(16 * 1024 * 1024),
  refusal: z.literal('too_large'),
});
const reportSchema = z.object({
  status: z.enum(['checks_passed_native_cleanup_required', 'failed']),
  phase: z.string(),
  checks: z.array(z.string()),
  grants: z.array(recordedGrantSchema),
  nativeSnapshots: z.array(nativeSnapshotReferenceSchema),
  sdkRetiredSnapshots: z.array(nativeSnapshotReferenceSchema),
  overflowEvidence: z.array(overflowEvidenceSchema.extend({ leaseId: z.string().min(1) })),
  trackingBoundEvidence: z.array(trackingBoundEvidenceSchema),
  nativeCleanupVerified: z.literal(false),
  errorCode: z.string().optional(),
});

function requireThat(value: unknown): asserts value {
  if (!value) throw new Error('acceptance assertion failed');
}

function sameBytes(actual: Uint8Array | null, expected: Uint8Array): boolean {
  return (
    actual !== null && actual.length === expected.length && actual.every((value, index) => value === expected[index])
  );
}

async function bounded<T>(work: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      work,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('acceptance bound exceeded')), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function terminalText(reader: AsyncIterator<TerminalChunk>, needles: readonly string[]): Promise<void> {
  const deadline = Date.now() + 20_000;
  const decoder = new TextDecoder();
  let text = '';
  while (!needles.every((needle) => text.includes(needle))) {
    const next = await bounded(reader.next(), Math.max(1, deadline - Date.now()));
    requireThat(!next.done && next.value.type === 'data');
    text = (text + decoder.decode(next.value.bytes, { stream: true })).slice(-32_768);
    requireThat(Date.now() <= deadline);
  }
}

const grants: z.output<typeof recordedGrantSchema>[] = [];
const nativeSnapshots: NativeSnapshotRef[] = [];
const sdkRetiredSnapshots: NativeSnapshotRef[] = [];
const overflowEvidence: Array<z.output<typeof overflowEvidenceSchema> & { leaseId: string }> = [];
const trackingBoundEvidence: z.output<typeof trackingBoundEvidenceSchema>[] = [];
const checks: string[] = [];
const leases = new Map<string, LeaseId>();
let phase = 'configuration';
let client: SandboxWsClient | undefined;
let failure: unknown;

async function requireTrackingBound(sandbox: Sandbox, path: string): Promise<void> {
  const activeClient = client;
  requireThat(activeClient !== undefined);
  const count = await sandbox.runCommand({
    cmd: 'node',
    args: ['-e', `process.stdout.write(String(require('node:fs').readdirSync('${path}').length));`],
  });
  count.throwIfFailed();
  requireThat(count.stdout() === '100001');
  const refused = await activeClient.fileOp(sandbox.sandboxId, { op: 'checkpoint', path, hash: false });
  requireThat(
    refused.outcome === 'failed' &&
      refused.kind === 'too_large' &&
      refused.message === `${path} holds more than 100000 entries or 16777216 bytes of listing to track`,
  );
  trackingBoundEvidence.push(
    trackingBoundEvidenceSchema.parse({
      leaseId: sandbox.sandboxId,
      path,
      entriesObserved: 100_001,
      maxEntries: 100_000,
      maxManifestBytes: 16 * 1024 * 1024,
      refusal: refused.kind,
    }),
  );
}

try {
  const config = configurationSchema.parse(process.env);
  phase = 'session_authentication';
  client = await SandboxWsClient.connect({
    gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
    gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
    apiKey: config.SANDBOX_API_KEY,
    requestTimeoutMs: 60_000,
    retry: false,
  });
  const activeClient = client;
  let invalidTokenAccepted = false;
  try {
    const invalid = await bounded(
      SandboxWsClient.connectWithToken({
        gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
        token: 'invalid-acceptance-token',
        requestTimeoutMs: 5_000,
        retry: false,
      }),
      10_000,
    );
    invalidTokenAccepted = true;
    invalid.close();
  } catch {
    // The authenticated connection remains the positive control.
  }
  requireThat(!invalidTokenAccepted);
  await client.listCapabilities();
  checks.push(phase);

  const allocate = client.lease.bind(client);
  client.lease = async (...args: Parameters<SandboxWsClient['lease']>) => {
    const response = await allocate(...args);
    const grant = grantSchema.parse(response);
    if (grant.created === true) leases.set(grant.lease_id, response.lease_id);
    grants.push({
      leaseId: grant.lease_id,
      provider: grant.provider,
      nativeId: grant.sandbox_id,
      created: grant.created === true,
      brokerDeleted: false,
    });
    return response;
  };
  phase = 'create';
  const createOptions = {
    tenant: tenantId(config.SANDBOX_ACCEPTANCE_TENANT),
    providerPref: [config.SANDBOX_ACCEPTANCE_PROVIDER],
    timeoutMs: 600_000,
    ports: [3333],
    persistent: true,
  };
  const sandbox = await Sandbox.create(client, {
    ...createOptions,
    image: {
      kind: 'registry',
      reference: {
        registry: config.SANDBOX_ACCEPTANCE_IMAGE_REGISTRY,
        repository: config.SANDBOX_ACCEPTANCE_IMAGE,
        ...(config.SANDBOX_ACCEPTANCE_IMAGE_TAG === undefined ? {} : { tag: config.SANDBOX_ACCEPTANCE_IMAGE_TAG }),
      },
    },
  });
  requireThat(sandbox.provider === config.SANDBOX_ACCEPTANCE_PROVIDER);
  requireThat(grants.at(-1)?.created);
  checks.push(phase);
  const nonce = crypto.randomUUID().replaceAll('-', '');
  const root = `/tmp/provider-acceptance-${nonce}`;
  let largeHome: string | undefined;

  phase = 'command_contract';
  const status = await sandbox.runCommand({
    cmd: 'sh',
    args: ['-c', 'printf acceptance-stdout; printf acceptance-stderr >&2; exit 7'],
  });
  requireThat(
    status.exitCode === 7 && status.stdout() === 'acceptance-stdout' && status.stderr() === 'acceptance-stderr',
  );
  const deadline = await sandbox.runCommand({ cmd: 'sh', args: ['-c', 'sleep 60'], timeoutSeconds: 1 });
  requireThat(deadline.timedOut && deadline.exitCode !== 0);
  checks.push(phase);

  phase = 'binary_files_and_cwd';
  (await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', root] })).throwIfFailed();
  const bytes = Uint8Array.from({ length: 4096 }, (_, index) => index % 256);
  await sandbox.writeFiles([{ path: `${root}/binary`, content: bytes }]);
  const read = await sandbox.readFileToBuffer({ path: `${root}/binary` });
  requireThat(sameBytes(read, bytes));
  const cwd = (await sandbox.runCommand({ cmd: 'pwd', cwd: root })).text();
  requireThat(cwd === root);
  if (sandbox.provider === 'vercel') {
    requireThat((await sandbox.runCommand({ cmd: 'pwd' })).text() === '/vercel/sandbox');
  }
  checks.push(phase);

  if (sandbox.provider === 'daytona') {
    phase = 'interactive_stdin';
    requireThat(sandbox.supports('write_stdin'));
    const interactive = await sandbox.runCommand({
      cmd: 'sh',
      args: ['-c', 'IFS= read -r line; printf "input:%s" "$line"'],
      detached: true,
      stdin: 'interactive',
      deadline: 'lease',
    });
    await interactive.writeStdin('ordered-');
    await interactive.writeStdin('input\n');
    await interactive.closeStdin();
    requireThat((await interactive.wait()).stdout() === 'input:ordered-input');
    checks.push(phase);
  }

  phase = 'signed_preview';
  requireThat(sandbox.supports('port_preview'));
  const server = await sandbox.runCommand({
    cmd: 'node',
    args: ['-e', `require('node:http').createServer((req,res)=>res.end('${nonce}')).listen(3333,'0.0.0.0')`],
    detached: true,
    deadline: 'lease',
  });
  const preview = await sandbox.preview(3333, { waitMs: 30_000 });
  const page = await fetch(preview.url, { signal: AbortSignal.timeout(10_000) });
  requireThat(page.status === 200 && (await page.text()) === nonce);
  checks.push(phase);

  phase = 'pause_resume';
  requireThat(sandbox.supports('pause_resume'));
  await sandbox.pause({ maxPausedMs: 120_000 });
  await sandbox.resume();
  requireThat(sameBytes(await sandbox.readFileToBuffer({ path: `${root}/binary` }), bytes));
  (await sandbox.runCommand({ cmd: 'true' })).throwIfFailed();
  if (sandbox.provider === 'e2b') {
    requireThat((await client.pollCommand(sandbox.sandboxId, server.cmdId)).outcome === 'running');
  }
  checks.push(phase);

  if (sandbox.provider !== 'vercel') {
    phase = sandbox.provider === 'e2b' ? 'long_idle_terminal' : 'terminal_pty';
    requireThat(sandbox.supports('terminal'));
    let terminal: Terminal | undefined;
    try {
      terminal = await sandbox.terminal({ cmd: ['/bin/sh'] });
      const reader = terminal[Symbol.asyncIterator]();
      await terminal.write(`printf 'terminal-%s\\n' 'before-${nonce}'\r`);
      await terminalText(reader, [`terminal-before-${nonce}`]);
      if (sandbox.provider === 'e2b') {
        await delay(config.SANDBOX_ACCEPTANCE_LONG_IDLE_SECONDS * 1000);
      }
      await terminal.resize(91, 29);
      await terminal.write(`stty size; printf 'terminal-%s\\n' 'after-${nonce}'\r`);
      await terminalText(reader, ['29 91', `terminal-after-${nonce}`]);
    } finally {
      await terminal?.close();
    }
    checks.push(phase);
  }

  if (sandbox.provider === 'e2b') {
    phase = 'watch_overflow_and_evidence';
    requireThat(sandbox.supports('file_watch') && sandbox.supports('evidence_streaming'));
    const watchPath = `${root}/watch`;
    (await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', watchPath] })).throwIfFailed();
    const watching = await client.fileOp(sandbox.sandboxId, { op: 'watch_start', path: watchPath, recursive: true });
    requireThat(watching.outcome === 'watch_started');
    const evidence = client.subscribeEvidence(sandbox.sandboxId);
    try {
      await evidence.ready;
      (
        await sandbox.runCommand({
          cmd: 'node',
          args: [
            '-e',
            `const fs=require('node:fs');for(let i=0;i<8193;i++)fs.closeSync(fs.openSync('${watchPath}/event-'+i,'w'));`,
          ],
          timeoutSeconds: 120,
        })
      ).throwIfFailed();
      const watchDeadline = Date.now() + 30_000;
      while (true) {
        const events = await client.fileOp(sandbox.sandboxId, {
          op: 'watch_poll',
          watch_id: watching.watch_id,
          after_seq: 0,
          wait_ms: 1000,
        });
        requireThat(events.outcome === 'watch_events');
        requireThat(
          events.events.every(
            (event, index) =>
              event.path.startsWith(`${watchPath}/`) && (index === 0 || event.seq > events.events[index - 1]!.seq),
          ),
        );
        if (events.overflowed) {
          requireThat(events.events.length > 0 && events.next_seq > 0);
          break;
        }
        requireThat(Date.now() < watchDeadline);
      }
      const iterator = evidence.events[Symbol.asyncIterator]();
      const evidenceDeadline = Date.now() + 10_000;
      while (true) {
        const event = await bounded(iterator.next(), Math.max(1, evidenceDeadline - Date.now()));
        requireThat(!event.done);
        if (event.value.subjectSuffix !== 'file-watch-overflow') continue;
        const text =
          typeof event.value.payload === 'string' ? event.value.payload : new TextDecoder().decode(event.value.payload);
        const receipt = overflowEvidenceSchema.parse(JSON.parse(text));
        if (receipt.watch_id !== watching.watch_id) continue;
        overflowEvidence.push({ leaseId: sandbox.sandboxId, ...receipt });
        break;
      }
    } finally {
      evidence.close();
      const stopped = await client.fileOp(sandbox.sandboxId, { op: 'watch_stop', watch_id: watching.watch_id });
      requireThat(stopped.outcome === 'watch_stopped' && stopped.existed);
    }
    checks.push(phase);

    phase = 'lost_start_reply_cancellation';
    requireThat(sandbox.supports('command_launch_cancellation') && sandbox.supports('command_cancellation'));
    const sentinel = await sandbox.runCommand({ cmd: 'sleep', args: ['300'], detached: true, deadline: 'lease' });
    const dispatch = client.runCommand.bind(client);
    let observedId: string | undefined;
    let acknowledge!: () => void;
    const acknowledged = new Promise<void>((resolve) => {
      acknowledge = resolve;
    });
    client.runCommand = async (...args: Parameters<SandboxWsClient['runCommand']>) => {
      const response = await dispatch(...args);
      if (response.outcome === 'started') {
        observedId = response.execution_id;
        acknowledge();
        return new Promise<never>(() => {});
      }
      return response;
    };
    try {
      const controller = new AbortController();
      const waiting = sandbox
        .runCommand({
          cmd: 'sh',
          args: ['-c', 'sleep 300 & wait'],
          signal: controller.signal,
          cancellationTimeoutMs: 30_000,
        })
        .catch((error: unknown) => error);
      await bounded(
        Promise.race([
          acknowledged,
          waiting.then(() => {
            throw new Error('the launch ended before its missing-reply fixture');
          }),
        ]),
        60_000,
      );
      controller.abort(new Error('acceptance lost-reply cancellation'));
      const error = await bounded(waiting, 35_000);
      requireThat(error instanceof CommandCancellationError && !error.cleanupPending && Boolean(error.launchKey));
      requireThat(observedId !== undefined);
      requireThat((await activeClient.pollCommand(sandbox.sandboxId, observedId)).outcome === 'completed');
      requireThat((await activeClient.pollCommand(sandbox.sandboxId, sentinel.cmdId)).outcome === 'running');
    } finally {
      client.runCommand = dispatch;
      await sentinel.kill('kill');
      if (observedId !== undefined) await client.killCommand(sandbox.sandboxId, observedId, 'kill');
    }
    checks.push(phase);

    phase = 'large_home_tracking_bounds';
    requireThat(sandbox.supports('change_tracking'));
    largeHome = `/home/user/provider-acceptance-large-${nonce}`;
    (
      await sandbox.runCommand({
        cmd: 'node',
        args: [
          '-e',
          `const fs=require('node:fs');fs.mkdirSync('${largeHome}');for(let i=0;i<100001;i++)fs.closeSync(fs.openSync('${largeHome}/entry-'+i,'w'));`,
        ],
        timeoutSeconds: 120,
      })
    ).throwIfFailed();
    await requireTrackingBound(sandbox, largeHome);
    (await sandbox.runCommand({ cmd: 'mkdir', args: ['-p', `${root}/small`] })).throwIfFailed();
    await sandbox.writeFiles([{ path: `${root}/small/control`, content: nonce }]);
    const control = await client.checkpointFiles(sandbox.sandboxId, `${root}/small`, { hash: true });
    const unchanged = await client.fileChanges(sandbox.sandboxId, { since: control.checkpointId });
    requireThat(
      !unchanged.truncated &&
        unchanged.added.length === 0 &&
        unchanged.modified.length === 0 &&
        unchanged.deleted.length === 0,
    );
    checks.push(phase);
  }

  phase = 'native_snapshot_restore';
  const receipt = await sandbox.snapshot({ acceptNative: true });
  requireThat('reference' in receipt);
  nativeSnapshots.push(nativeSnapshotReferenceSchema.parse(receipt.reference));
  const restored = await Sandbox.create(client, { ...createOptions, fromSnapshot: receipt });
  requireThat(restored.provider === sandbox.provider && grants.at(-1)?.created);
  const restoredBytes = await restored.readFileToBuffer({ path: `${root}/binary` });
  requireThat(sameBytes(restoredBytes, bytes));
  (await restored.runCommand({ cmd: 'true' })).throwIfFailed();
  if (largeHome !== undefined) {
    const defaultChanges = await client.fileOp(restored.sandboxId, { op: 'changes' });
    requireThat(defaultChanges.outcome === 'failed' && defaultChanges.kind === 'invalid_request');
    await requireTrackingBound(restored, largeHome);
    const control = await client.checkpointFiles(restored.sandboxId, `${root}/small`, { hash: true });
    const unchanged = await client.fileChanges(restored.sandboxId, { since: control.checkpointId });
    requireThat(
      !unchanged.truncated &&
        unchanged.added.length === 0 &&
        unchanged.modified.length === 0 &&
        unchanged.deleted.length === 0,
    );
    checks.push('restored_large_home_baseline_refusal');
  }
  (await sandbox.runCommand({ cmd: 'true' })).throwIfFailed();
  await sandbox.delete();
  for (const grant of grants) if (grant.leaseId === sandbox.sandboxId) grant.brokerDeleted = true;
  requireThat(sameBytes(await restored.readFileToBuffer({ path: `${root}/binary` }), bytes));
  if (sandbox.provider === 'e2b') {
    requireThat((await client.pollCommand(restored.sandboxId, server.cmdId)).outcome === 'running');
    const restoredPreview = await restored.preview(3333, { waitMs: 30_000 });
    const response = await fetch(restoredPreview.url, { signal: AbortSignal.timeout(10_000) });
    requireThat(response.status === 200 && (await response.text()) === nonce);
  }
  checks.push(phase);
} catch (error: unknown) {
  failure = error;
} finally {
  if (client !== undefined) {
    for (const [id, leaseId] of leases) {
      try {
        await client.deleteLease(leaseId);
        for (const grant of grants) if (grant.leaseId === id) grant.brokerDeleted = true;
      } catch (error: unknown) {
        failure ??= error;
      }
    }
    for (const reference of nativeSnapshots) {
      if (reference.provider !== 'e2b') continue;
      try {
        await Sandbox.deleteNativeSnapshot(client, reference);
        sdkRetiredSnapshots.push(reference);
      } catch (error: unknown) {
        failure ??= error;
      }
    }
    client.close();
  }
}

const report = reportSchema.parse({
  status: failure === undefined ? 'checks_passed_native_cleanup_required' : 'failed',
  phase,
  checks,
  grants,
  nativeSnapshots,
  sdkRetiredSnapshots,
  overflowEvidence,
  trackingBoundEvidence,
  nativeCleanupVerified: false,
  ...(failure === undefined
    ? {}
    : {
        errorCode: failure instanceof SandboxWsError ? failure.code : 'acceptance_failed',
      }),
});
process.stdout.write(`${JSON.stringify(report)}\n`);
if (failure !== undefined) process.exitCode = 1;
