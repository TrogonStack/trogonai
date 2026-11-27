import assert from 'node:assert/strict';
import { test } from 'node:test';

import { z } from 'zod';

import type { LeaseId } from '../../src/index.ts';

const { Command, Sandbox, SandboxWsClient, leaseId, tenantId }: typeof import('../../src/index.ts') = await import(
  new URL('../../dist/index.js', import.meta.url).href
);

const imageSchema = z.object({ registry: z.string(), repository: z.string().min(1), tag: z.string().min(1) });
const commandSchema = z.strictObject({
  code: z.string().min(1),
  cwd: z.string().min(1).optional(),
  env: z.record(z.string(), z.string()).optional(),
  timeoutSeconds: z.number().int().min(1).max(15).default(5),
  maxOutputBytes: z.number().int().positive().optional(),
});
const resultSchema = z.object({
  exitCode: z.number().int(),
  stdout: z.string(),
  stderr: z.string(),
  timedOut: z.boolean(),
  truncated: z.boolean(),
  ok: z.boolean(),
});
const processIdentitySchema = z.object({ pid: z.number().int().positive(), start: z.string().regex(/^\d+$/) });
const processGroupSchema = z.object({ parent: processIdentitySchema, child: processIdentitySchema });
const stoppedGroupSchema = z.array(z.enum(['absent', 'zombie', 'replaced', 'alive'])).length(2);

test(
  'real local Docker command contracts preserve options, output and process ownership',
  { timeout: 120_000 },
  async (context) => {
    const config = z
      .object({
        SANDBOX_GATEWAY_HTTP_URL: z.string().url(),
        SANDBOX_GATEWAY_WS_URL: z.string().url(),
        SANDBOX_API_KEY: z.string().min(1),
        SANDBOX_TENANT: z.string().min(1),
        SANDBOX_PROVIDER: z.literal('local_docker').default('local_docker'),
        SANDBOX_COMMAND_IMAGE_JSON: z
          .string()
          .transform((value) => imageSchema.parse(JSON.parse(value)))
          .prefault(JSON.stringify({ registry: 'docker.io', repository: 'library/node', tag: '22-bookworm-slim' })),
      })
      .parse(process.env);
    const client = await SandboxWsClient.connect({
      gatewayHttpUrl: config.SANDBOX_GATEWAY_HTTP_URL,
      gatewayWsUrl: config.SANDBOX_GATEWAY_WS_URL,
      apiKey: config.SANDBOX_API_KEY,
    });
    const owned = new Set<LeaseId>();
    const leasing = client.lease.bind(client);
    context.mock.method(client, 'lease', async (...args: Parameters<typeof leasing>) => {
      const grant = await leasing(...args);
      if (grant.created === true) owned.add(grant.lease_id);
      return grant;
    });
    context.after(async () => {
      try {
        const present = (await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id)));
        const cleanup = await Promise.allSettled(present.map((row) => client.deleteLease(leaseId(row.lease_id))));
        assert.ok(
          cleanup.every((entry) => entry.status === 'fulfilled'),
          'delete every observed newly-created command lease',
        );
        assert.equal((await client.listLeases()).leases.filter((row) => owned.has(leaseId(row.lease_id))).length, 0);
      } finally {
        client.close();
      }
    });

    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(config.SANDBOX_TENANT),
      providerPref: ['local_docker'],
      timeoutMs: 180_000,
      spec: {
        image: { kind: 'registry', reference: config.SANDBOX_COMMAND_IMAGE_JSON },
        workspace: { empty: {} },
        env: {},
        cpu_seconds: 60,
        memory_mib: 256,
        boot_mode: 'bare',
        working_dir: '/tmp',
        network_policy: 'deny_all',
      },
    });
    assert.equal(sandbox.created, true, 'the fixture must prove ownership of its lease');
    const directoryName = 'commands-' + crypto.randomUUID();
    const directory = '/tmp/' + directoryName;

    async function node(code: string, options: Omit<z.input<typeof commandSchema>, 'code'> = {}) {
      const { code: script, ...params } = commandSchema.parse({ code, ...options });
      const result = await sandbox.runCommand({ cmd: 'node', args: ['-e', script], ...params });
      return resultSchema.parse({
        exitCode: result.exitCode,
        stdout: result.stdout(),
        stderr: result.stderr(),
        timedOut: result.timedOut,
        truncated: result.truncated,
        ok: result.ok,
      });
    }

    await context.test('Node runtime and configured command base are real', async () => {
      const response = await node(
        'process.stdout.write(JSON.stringify({ node:process.versions.node, cwd:process.cwd() }))',
      );
      assert.equal(response.exitCode, 0);
      const runtime = z.object({ node: z.string(), cwd: z.literal('/tmp') }).parse(JSON.parse(response.stdout));
      const [major, minor] = runtime.node.split('.').map(Number);
      assert.ok(major! > 22 || (major === 22 && minor! >= 7));
      assert.equal(
        (await node(`require('node:fs').mkdirSync(${JSON.stringify(directory + '/nested')},{recursive:true})`))
          .exitCode,
        0,
      );
    });

    await context.test('absolute and relative cwd resolve against the configured base', async () => {
      const absolute = await node('process.stdout.write(process.cwd())', { cwd: directory });
      assert.equal(absolute.exitCode, 0);
      assert.equal(absolute.stdout, directory);
      const relative = await node('process.stdout.write(process.cwd())', { cwd: './' + directoryName + '/nested' });
      assert.equal(relative.exitCode, 0);
      assert.equal(relative.stdout, directory + '/nested');
      assert.equal((await node('process.stdout.write(process.cwd())')).stdout, '/tmp');
    });

    await context.test('per-call environment is honored and cannot leak into the next command', async () => {
      const code = 'process.stdout.write(JSON.stringify(process.env.COMMAND_ENV_PROOF ?? null))';
      const scoped = await node(code, { env: { COMMAND_ENV_PROOF: 'per-call-marker' } });
      assert.equal(scoped.exitCode, 0);
      assert.equal(z.literal('per-call-marker').parse(JSON.parse(scoped.stdout)), 'per-call-marker');
      const subsequent = await node(code);
      assert.equal(subsequent.exitCode, 0);
      assert.equal(z.null().parse(JSON.parse(subsequent.stdout)), null);
    });

    await context.test('stdin reaches EOF by default', async () => {
      const result = await node(
        "let bytes=0;process.stdin.on('data',chunk=>bytes+=chunk.length);process.stdin.on('end',()=>process.stdout.write(String(bytes)))",
        { timeoutSeconds: 3 },
      );
      assert.equal(result.exitCode, 0);
      assert.equal(result.timedOut, false);
      assert.equal(result.stdout, '0');
    });

    await context.test('stdout and stderr stay separate and exit42 remains a result', async () => {
      const result = await node(
        "process.stdout.write('stdout-42');process.stderr.write('stderr-42');process.exitCode=42",
      );
      assert.equal(result.exitCode, 42);
      assert.equal(result.ok, false);
      assert.equal(result.timedOut, false);
      assert.equal(result.stdout, 'stdout-42');
      assert.equal(result.stderr, 'stderr-42');
    });

    await context.test('bounded output retains each stream head and tail', async () => {
      const result = await node(
        "process.stdout.write('OUT_HEAD\\n'+'o'.repeat(65536)+'\\nOUT_TAIL\\n');process.stderr.write('ERR_HEAD\\n'+'e'.repeat(65536)+'\\nERR_TAIL\\n')",
        { maxOutputBytes: 1024 },
      );
      assert.equal(result.exitCode, 0);
      assert.equal(result.truncated, true);
      for (const stream of [result.stdout, result.stderr]) {
        const markers = [...stream.matchAll(/\n\[\.\.\. (\d+) bytes truncated \.\.\.\]\n/g)];
        assert.equal(markers.length, 1, 'each truncated stream carries one bounded marker');
        assert.ok(Number(markers[0]![1]) > 0);
        assert.ok(Buffer.byteLength(stream.replace(markers[0]![0], '')) <= 1024);
      }
      assert.match(result.stdout, /^OUT_HEAD\n/);
      assert.match(result.stdout, /\nOUT_TAIL\n$/);
      assert.match(result.stderr, /^ERR_HEAD\n/);
      assert.match(result.stderr, /\nERR_TAIL\n$/);
    });

    await context.test('shell working directory and exports do not carry into another command', async () => {
      const stateful = await sandbox.runCommand({
        cmd: 'sh',
        args: ['-c', `cd ${directory}; export COMMAND_SHELL_PROOF=only-here; printf changed`],
        timeoutSeconds: 5,
      });
      assert.equal(stateful.exitCode, 0);
      assert.equal(stateful.stdout(), 'changed');
      const fresh = await node(
        'process.stdout.write(JSON.stringify({cwd:process.cwd(),state:process.env.COMMAND_SHELL_PROOF??null}))',
      );
      assert.equal(fresh.exitCode, 0);
      assert.deepEqual(z.object({ cwd: z.literal('/tmp'), state: z.null() }).parse(JSON.parse(fresh.stdout)), {
        cwd: '/tmp',
        state: null,
      });
    });

    await context.test('timeout reports timedOut and terminates the command process group', async () => {
      const marker = directory + '/process-group.json';
      const code = `const fs=require('node:fs');const{spawn}=require('node:child_process');const identity=pid=>{const value=fs.readFileSync('/proc/'+pid+'/stat','utf8');return{pid,start:value.slice(value.lastIndexOf(')')+2).trim().split(/\\s+/)[19]}};const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});child.on('spawn',()=>{fs.writeFileSync(${JSON.stringify(marker)},JSON.stringify({parent:identity(process.pid),child:identity(child.pid)}));setInterval(()=>{},1000)})`;
      const result = await node(code, { timeoutSeconds: 1 });
      assert.equal(result.timedOut, true);
      assert.equal(result.exitCode, -1);
      assert.equal(result.ok, false);
      const raw = await sandbox.readFile({ path: marker });
      assert.ok(raw, 'the timed command must first record its actual process identities');
      const processes = processGroupSchema.parse(JSON.parse(raw));
      const check = `const fs=require('node:fs');const expected=${JSON.stringify(processes)};process.stdout.write(JSON.stringify(Object.values(expected).map(({pid,start})=>{try{const value=fs.readFileSync('/proc/'+pid+'/stat','utf8');const fields=value.slice(value.lastIndexOf(')')+2).trim().split(/\\s+/);return fields[19]!==start?'replaced':fields[0]==='Z'?'zombie':'alive'}catch(error){if(error.code==='ENOENT')return'absent';throw error}})))`;
      const deadline = Date.now() + 10_000;
      let observed: z.output<typeof stoppedGroupSchema>;
      do {
        const status = await node(check, { timeoutSeconds: 3 });
        assert.equal(status.exitCode, 0);
        observed = stoppedGroupSchema.parse(JSON.parse(status.stdout));
        if (observed.every((state) => state !== 'alive')) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      } while (Date.now() < deadline);
      assert.ok(
        observed.every((state) => state !== 'alive'),
        'timeout must stop both original process identities, including the child',
      );
    });

    await context.test('local Docker returns a real detached command that can be stopped explicitly', async () => {
      const command = await sandbox.runCommand({
        cmd: 'node',
        args: ['-e', 'setInterval(()=>{},1000)'],
        detached: true,
        deadline: 'lease',
      });
      assert.ok(command instanceof Command, 'local Docker must honor detached execution');
      await command.stop({ graceMs: 0 });
      const finished = await command.wait({ pollIntervalMs: 50 });
      assert.equal(finished.ok, false);
      assert.equal(finished.timedOut, false);
    });
  },
);
