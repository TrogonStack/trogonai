import {
  CommandFinished,
  EditRejectedError,
  MemorySandboxClient,
  Sandbox,
  SandboxWsError,
  tenantId,
} from '@trogonai/sandbox-ws-client';
import type { CommandHandler } from '@trogonai/sandbox-ws-client';
import { describe, expect, it } from 'vitest';

import { toAiSdkTools } from '../src/adapters/ai-sdk.js';
import { toMastraTools } from '../src/adapters/mastra.js';
import { toOpenAiAgentsTools } from '../src/adapters/openai-agents.js';
import { TOOL_NAMES, clip, createSandboxTools, nativeFileOps, toolList, toolSchemas } from '../src/index.js';
import type { NativeFileOps, ToolSandbox } from '../src/index.js';

/** Minimal `sh -c` for the fake: the script's first word picks a registered handler. */
const sh: CommandHandler = async (ctx) => {
  const script = ctx.args[1] ?? '';
  if (script === 'fail') {
    return { exitCode: 3, stdout: 'partial\n', stderr: 'boom\n' };
  }
  if (script.startsWith('echo ')) {
    return { exitCode: 0, stdout: `${script.slice(5)}\n` };
  }
  if (script === 'pwd') {
    return { exitCode: 0, stdout: `${ctx.cwd}\n` };
  }
  if (script === 'serve') {
    await new Promise((resolve) => setTimeout(resolve, 60));
    return { exitCode: 0, stdout: 'server stopped\n' };
  }
  return { exitCode: 127, stderr: `${script}: not found\n` };
};

const grep: CommandHandler = ({ args, fs, cwd }) => {
  const [, ...rest] = args;
  const pattern = rest[rest.indexOf('--') + 1];
  const root = rest.at(-1) ?? '.';
  const lines: string[] = [];
  for (const name of fs.list(root, cwd)) {
    const path = `${root.replace(/\/$/, '')}/${name}`;
    new TextDecoder()
      .decode(fs.read(path, cwd))
      .split('\n')
      .forEach((line, i) => {
        if (line.includes(pattern)) lines.push(`${path}:${i + 1}:${line}`);
      });
  }
  return { exitCode: lines.length === 0 ? 1 : 0, stdout: lines.map((l) => `${l}\n`).join('') };
};

async function setup(options: Parameters<typeof createSandboxTools>[1] = {}) {
  const client = new MemorySandboxClient({ commands: { sh, grep } });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
  const tools = createSandboxTools(sandbox, options);
  const run = async (name: (typeof TOOL_NAMES)[number], input: unknown) => {
    const tool = tools[name];
    if (tool === undefined) throw new Error(`tool ${name} not offered`);
    return tool.execute(input);
  };
  return { client, sandbox, tools, run };
}

describe('createSandboxTools', () => {
  it('offers every tool with descriptions that say when not to use them', async () => {
    const { tools } = await setup();
    expect(toolList(tools).map((t) => t.name)).toEqual([...TOOL_NAMES]);
    for (const tool of toolList(tools)) {
      expect(tool.description).toMatch(/Do not use|do not use|do not guess/);
    }
  });

  it('filters to the allowed subset', async () => {
    const { tools } = await setup({ allow: ['read_file', 'find_files'] });
    expect(toolList(tools).map((t) => t.name)).toEqual(['read_file', 'find_files']);
    expect(tools.run_command).toBeUndefined();
  });

  it('drops denied tools, and deny wins over allow', async () => {
    const { tools } = await setup({ allow: ['read_file', 'run_command'], deny: ['run_command', 'write_file'] });
    expect(toolList(tools).map((t) => t.name)).toEqual(['read_file']);
  });

  it('lets beforeRun refuse a command before it reaches the sandbox', async () => {
    const seen: (string | undefined)[] = [];
    const { run } = await setup({
      beforeRun: ({ tool, command }) => {
        seen.push(command);
        return tool === 'run_command' && command?.startsWith('rm ')
          ? { deny: 'destructive commands are blocked' }
          : undefined;
      },
    });
    const refused = await run('run_command', { command: 'rm -rf /' });
    expect(refused).toMatch(/^Verdict: refused, destructive commands are blocked/);
    expect(await run('run_command', { command: 'echo hi' })).toContain('hi');
    expect(seen).toEqual(['rm -rf /', 'echo hi']);
  });

  it('lets beforeRun rewrite the command, and treats a throwing hook as a refusal', async () => {
    const { run } = await setup({
      beforeRun: ({ command }) => (command === 'echo a' ? { command: 'echo b' } : undefined),
    });
    expect(await run('run_command', { command: 'echo a' })).toContain('b');
    const { run: runThrowing } = await setup({
      beforeRun: () => {
        throw new Error('policy offline');
      },
    });
    expect(await runThrowing('read_file', { path: '/x' })).toMatch(
      /^Verdict: refused, policy hook failed: policy offline/,
    );
  });

  it('keeps inputs flat, with no lease id or base64', () => {
    for (const schema of Object.values(toolSchemas)) {
      const keys = Object.keys(schema.shape);
      expect(keys).not.toContain('lease_id');
      expect(keys).not.toContain('content_base64');
    }
  });

  describe('run_command', () => {
    it('reports an ok verdict first, then stdout', async () => {
      const { run } = await setup();
      const out = await run('run_command', { command: 'echo hello' });
      expect(out.split('\n')[0]).toBe('Verdict: ok (exit 0)');
      expect(out).toContain('stdout:\nhello');
    });

    it('reports a failing exit code with both streams', async () => {
      const { run } = await setup();
      const out = await run('run_command', { command: 'fail' });
      expect(out.split('\n')[0]).toBe('Verdict: failed (exit 3)');
      expect(out).toContain('stdout:\npartial');
      expect(out).toContain('stderr:\nboom');
    });

    it('runs in the configured working directory', async () => {
      const { run } = await setup({ cwd: '/work' });
      expect(await run('run_command', { command: 'pwd' })).toContain('/work');
    });

    it('returns an error verdict instead of throwing on bad input', async () => {
      const { run } = await setup();
      const out = await run('run_command', { command: '' });
      expect(out.split('\n')[0]).toMatch(/^Verdict: error, invalid input/);
    });

    it('asks the stack for an output cap, flags timeouts and flags truncation', async () => {
      const seen: unknown[] = [];
      const finished = (flags: { timedOut?: boolean; truncated?: boolean }, code: number) =>
        new CommandFinished('l' as never, '', code, new TextEncoder().encode('x'.repeat(100)), new Uint8Array(), flags);
      const fake: ToolSandbox = {
        sandboxId: 'l' as never,
        runCommand: (async (params: { timeoutSeconds?: number; maxOutputBytes?: number }) => {
          seen.push(params);
          return params.timeoutSeconds === 5 ? finished({ timedOut: true }, -1) : finished({ truncated: true }, 0);
        }) as unknown as ToolSandbox['runCommand'],
        readFileToBuffer: async () => null,
        writeFiles: async () => undefined,
        domain: async () => 'x:1',
      };
      const tools = createSandboxTools(fake, { maxOutputBytes: 1024, maxOutputChars: 40 });
      const timedOut = await tools.run_command!.execute({ command: 'sleep 99', timeout_seconds: 5 });
      expect(timedOut.split('\n')[0]).toBe('Verdict: timed out after 5s and was killed');
      const truncated = await tools.run_command!.execute({ command: 'yes' });
      expect(truncated).toContain('Note: output exceeded the retention cap');
      expect(truncated).toContain('characters omitted');
      expect(seen[0]).toMatchObject({ maxOutputBytes: 1024 });
    });

    it('passes host-set redact values through to run_command', async () => {
      const seen: unknown[] = [];
      const fake = {
        sandboxId: 'l',
        runCommand: async (params: unknown) => {
          seen.push(params);
          return new CommandFinished('l' as never, '', 0, new Uint8Array(), new Uint8Array());
        },
      } as unknown as ToolSandbox;
      await createSandboxTools(fake, { redactValues: ['s3cret-value'] }).run_command!.execute({ command: 'env' });
      expect(seen[0]).toMatchObject({ redactValues: ['s3cret-value'] });
      await createSandboxTools(fake).run_command!.execute({ command: 'env' });
      expect((seen[1] as { redactValues?: unknown }).redactValues).toBeUndefined();
    });

    it('clamps a requested timeout to the maximum', async () => {
      const seen: number[] = [];
      const fake = {
        sandboxId: 'l',
        runCommand: async (params: { timeoutSeconds?: number }) => {
          seen.push(params.timeoutSeconds ?? -1);
          return new CommandFinished('l' as never, '', 0, new Uint8Array(), new Uint8Array());
        },
      } as unknown as ToolSandbox;
      const tools = createSandboxTools(fake, { maxTimeoutSeconds: 100 });
      await tools.run_command!.execute({ command: 'x', timeout_seconds: 9999 });
      await tools.run_command!.execute({ command: 'x' });
      expect(seen).toEqual([100, 60]);
    });
  });

  describe('files', () => {
    it('writes with parents, then reads numbered lines and pages them', async () => {
      const { run, client, sandbox } = await setup({ cwd: '/work' });
      const wrote = await run('write_file', { path: 'src/a.txt', content: 'one\ntwo\nthree\n' });
      expect(wrote).toBe('Verdict: ok, wrote 14 bytes to /work/src/a.txt');
      expect(new TextDecoder().decode(client.filesystem(sandbox.sandboxId).read('/work/src/a.txt'))).toBe(
        'one\ntwo\nthree\n',
      );

      const page = await run('read_file', { path: 'src/a.txt', limit: 2 });
      expect(page.split('\n')[0]).toBe('Verdict: ok, /work/src/a.txt lines 1-2 of 3');
      expect(page).toContain('1\tone\n2\ttwo');
      expect(page).toContain('call again with offset 3');
      expect(await run('read_file', { path: 'src/a.txt', offset: 3 })).toContain('3\tthree');
    });

    it('says so when a file is missing, binary or the offset is past the end', async () => {
      const { run, client, sandbox } = await setup();
      expect(await run('read_file', { path: '/nope' })).toBe('Verdict: error, no such file: /nope');
      client.filesystem(sandbox.sandboxId).write('/bin.dat', new Uint8Array([1, 0, 2]));
      expect(await run('read_file', { path: '/bin.dat' })).toMatch(/looks binary/);
      await run('write_file', { path: '/a', content: 'x\n' });
      expect(await run('read_file', { path: '/a', offset: 5 })).toMatch(/past the end/);
    });

    it('edits exact text and refuses, writing nothing, on a count mismatch', async () => {
      const { run } = await setup();
      await run('write_file', { path: '/a.txt', content: 'cat dog cat' });
      const refused = await run('edit_file', { path: '/a.txt', old_text: 'cat', new_text: 'bird' });
      expect(refused).toBe('Verdict: refused, old_text occurs 2 time(s) but 1 expected; nothing was written');
      expect(await run('read_file', { path: '/a.txt' })).toContain('cat dog cat');
      const ok = await run('edit_file', { path: '/a.txt', old_text: 'cat', new_text: 'bird', expected_occurrences: 2 });
      expect(ok).toBe('Verdict: ok, replaced 2 occurrence(s) in /a.txt');
      expect(await run('read_file', { path: '/a.txt' })).toContain('bird dog bird');
      expect(await run('edit_file', { path: '/missing', old_text: 'a', new_text: 'b' })).toMatch(/no such file/);
    });

    it('lists a directory through the shell fallback', async () => {
      const { run } = await setup();
      await run('write_file', { path: '/d/a.txt', content: 'a' });
      await run('write_file', { path: '/d/b.txt', content: 'b' });
      const out = await run('find_files', { path: '/d' });
      expect(out.split('\n')[0]).toBe('Verdict: ok, 2 entr(ies) in /d');
      expect(out).toContain('a.txt\nb.txt');
      expect((await run('find_files', { path: '/missing' })).split('\n')[0]).toMatch(/^Verdict: error, could not list/);
    });

    it('searches contents through the shell fallback and caps results', async () => {
      const { run } = await setup();
      await run('write_file', { path: '/s/a.txt', content: 'TODO one\nok\nTODO two\n' });
      const out = await run('find_files', { path: '/s', pattern: 'TODO' });
      expect(out.split('\n')[0]).toBe('Verdict: ok, 2 match(es) under /s');
      expect(out).toContain('/s/a.txt:1:TODO one');
      const capped = await run('find_files', { path: '/s', pattern: 'TODO', max_results: 1 });
      expect(capped).toContain('results capped at 1');
      expect(await run('find_files', { path: '/s', pattern: 'absent' })).toBe('Verdict: ok, no matches under /s');
    });
  });

  describe('native file ops', () => {
    const calls: string[] = [];
    const native: NativeFileOps = {
      async edit(path, _f, _r, expected) {
        calls.push(`edit ${path} ${expected}`);
        if (path === '/rejected') throw new EditRejectedError(3, expected);
        return { occurrences: expected };
      },
      async search(path, o) {
        calls.push(`search ${path} ${o.pattern} ${o.glob ?? ''}`);
        return { matches: [{ path: '/x.ts', line: 4, text: 'needle' }], truncated: true };
      },
      async list(path, o) {
        calls.push(`list ${path} ${o.recursive}`);
        return {
          entries: [
            { path: '/x', kind: 'directory' },
            { path: '/x/y.ts', kind: 'file' },
          ],
          truncated: false,
        };
      },
    };

    it('prefers them over the fallbacks', async () => {
      const { run } = await setup({ nativeFileOps: native });
      expect(await run('edit_file', { path: '/f', old_text: 'a', new_text: 'b' })).toBe(
        'Verdict: ok, replaced 1 occurrence(s) in /f',
      );
      expect(await run('edit_file', { path: '/rejected', old_text: 'a', new_text: 'b' })).toBe(
        'Verdict: refused, old_text occurs 3 time(s) but 1 expected; nothing was written',
      );
      const search = await run('find_files', { path: '/', pattern: 'needle', glob: '*.ts' });
      expect(search).toContain('/x.ts:4: needle');
      expect(search).toContain('results capped');
      expect(await run('find_files', { path: '/', recursive: true })).toContain('/x/\n/x/y.ts');
      expect(calls).toContain('search / needle *.ts');
      expect(calls).toContain('list / true');
    });

    it('binds the gateway client helpers to a lease', async () => {
      const seen: unknown[][] = [];
      const ops = nativeFileOps(
        {
          editFile: async (...a: unknown[]) => (seen.push(a), { occurrences: 1, size: 1 }),
          searchFiles: async (...a: unknown[]) => (seen.push(a), { matches: [], truncated: false, filesScanned: 0 }),
          listDir: async (...a: unknown[]) => (seen.push(a), { entries: [], truncated: false }),
        } as never,
        'lease-1' as never,
      );
      await ops.edit('/p', 'a', 'b', 2);
      await ops.search('/p', { pattern: 'q', maxResults: 5 });
      await ops.list('/p', { recursive: true, maxEntries: 9 });
      expect(seen[0]).toEqual(['lease-1', '/p', 'a', 'b', { expectedOccurrences: 2 }]);
      expect(seen[1]?.[0]).toBe('lease-1');
      expect(seen[2]).toEqual(['lease-1', '/p', { recursive: true, maxEntries: 9 }]);
    });
  });

  describe('background_process', () => {
    it('starts, reports running, reports the exit with output, and rejects unknown names', async () => {
      const { run } = await setup();
      expect(await run('background_process', { action: 'start', name: 'web', command: 'serve' })).toMatch(
        /^Verdict: ok, started web/,
      );
      expect(await run('background_process', { action: 'status', name: 'web' })).toBe('Verdict: ok, web is running');
      expect(await run('background_process', { action: 'start', name: 'web', command: 'serve' })).toMatch(
        /already running/,
      );
      await new Promise((resolve) => setTimeout(resolve, 150));
      const done = await run('background_process', { action: 'status', name: 'web' });
      expect(done.split('\n')[0]).toBe('Verdict: web exited (exit 0)');
      expect(done).toContain('server stopped');
      expect(await run('background_process', { action: 'status', name: 'nope' })).toMatch(/no process named nope/);
      expect(await run('background_process', { action: 'start', name: 'x' })).toMatch(/`command` is required/);
    });
  });

  describe('start_server', () => {
    const wire = (client: MemorySandboxClient) => {
      client.waitSandboxReady = (async () => ({ outcome: 'ready' })) as typeof client.waitSandboxReady;
      client.getSandboxDomain = async () => ({
        outcome: 'available',
        endpoint: 'h:3000',
        url: 'https://gw/v1/preview/l/3000/tok/',
        expires_at: 1_700_000_000,
      });
    };

    it('reports the preview url and keeps the server addressable by background_process', async () => {
      const { client, run } = await setup();
      wire(client);
      expect(await run('start_server', { name: 'web', command: 'serve', port: 3000 })).toBe(
        'Verdict: ok, web is ready (port 3000 answers)\nhttps://gw/v1/preview/l/3000/tok/',
      );
      expect(await run('background_process', { action: 'status', name: 'web' })).toBe('Verdict: ok, web is running');
      expect(await run('start_server', { name: 'web', command: 'serve', port: 3000 })).toMatch(/already running/);
    });

    it('follows the server across a pause and resume, and does not restart one that was stopped', async () => {
      const { client, run, sandbox } = await setup();
      wire(client);
      await run('start_server', { name: 'web', command: 'serve', port: 3000 });
      await run('start_server', { name: 'api', command: 'serve', port: 3001 });
      await sandbox.pause();
      await run('background_process', { action: 'stop', name: 'api' });
      await new Promise((resolve) => setTimeout(resolve, 100));
      expect((await run('background_process', { action: 'status', name: 'web' })).split('\n')[0]).toMatch(
        /exited|finished/i,
      );
      await sandbox.resume();
      expect(await run('background_process', { action: 'status', name: 'web' })).toBe('Verdict: ok, web is running');
      expect((await run('background_process', { action: 'status', name: 'api' })).split('\n')[0]).not.toBe(
        'Verdict: ok, api is running',
      );
    });

    it('returns the process output when it exits before the port opens', async () => {
      const { client, run } = await setup();
      wire(client);
      client.waitSandboxReady = (async () => ({
        outcome: 'not_ready',
        pending_ports: [3000],
      })) as typeof client.waitSandboxReady;
      const out = await run('start_server', { name: 'web', command: 'nope', port: 3000 });
      expect(out.split('\n')[0]).toMatch(/^Verdict: error, command_failed: server exited with code 127/);
      expect(out).toContain('nope: not found');
    });

    it('applies the beforeRun policy to the command', async () => {
      const { client, run } = await setup({
        beforeRun: ({ tool }) => (tool === 'start_server' ? { deny: 'no servers' } : undefined),
      });
      wire(client);
      expect(await run('start_server', { name: 'web', command: 'serve', port: 3000 })).toBe(
        'Verdict: refused, no servers',
      );
    });
  });

  describe('preview_url', () => {
    it('adds a scheme to a bare endpoint and reports unsupported providers as text', async () => {
      const { sandbox } = await setup();
      const withDomain = createSandboxTools(
        {
          ...sandbox,
          sandboxId: sandbox.sandboxId,
          domain: async (p: number) => `host.example:${p}`,
        } as unknown as ToolSandbox,
        { previewScheme: 'https' },
      );
      expect(await withDomain.preview_url!.execute({ port: 3000 })).toBe(
        'Verdict: ok, port 3000 is reachable\nhttps://host.example:3000',
      );
      const { run } = await setup();
      expect((await run('preview_url', { port: 3000 })).split('\n')[0]).toMatch(/^Verdict: error, /);
    });
  });

  describe('change tracking and watching', () => {
    it('checkpoints, reports exact changes since it, and since the start', async () => {
      const { run } = await setup();
      await run('write_file', { path: '/w/keep.txt', content: 'k' });
      await run('write_file', { path: '/w/gone.txt', content: 'g' });
      await run('write_file', { path: '/w/edit.txt', content: 'one' });
      const cp = await run('checkpoint_files', { path: '/w', hash: true });
      expect(cp.split('\n')[0]).toMatch(/^Verdict: ok, recorded \d+ entr\(ies\) under \/w$/);
      const id = /checkpoint_id: (\S+)/.exec(cp)![1]!;

      await run('write_file', { path: '/w/edit.txt', content: 'two' });
      await run('write_file', { path: '/w/new.txt', content: 'n' });
      const sandboxChanges = await run('file_changes', { since: id });
      expect(sandboxChanges.split('\n')[0]).toMatch(/^Verdict: ok, 2 change\(s\) since /);
      expect(sandboxChanges).toContain('added /w/new.txt');
      expect(sandboxChanges).toContain('modified /w/edit.txt');

      const fromStart = await run('file_changes', {});
      expect(fromStart).toContain('since start');
      expect(fromStart).toContain('added /w/keep.txt');
      expect((await run('file_changes', { since: 'nope' })).split('\n')[0]).toMatch(
        /^Verdict: error, invalid_request: /,
      );
    });

    it('reports a sandbox without the verbs as text', async () => {
      const { sandbox } = await setup();
      const bare = createSandboxTools({
        sandboxId: sandbox.sandboxId,
        runCommand: sandbox.runCommand.bind(sandbox),
        readFileToBuffer: sandbox.readFileToBuffer.bind(sandbox),
        writeFiles: sandbox.writeFiles.bind(sandbox),
        domain: sandbox.domain.bind(sandbox),
      } as ToolSandbox);
      for (const [name, input] of [
        ['checkpoint_files', {}],
        ['file_changes', {}],
        ['watch_files', { path: '/w' }],
      ] as const) {
        expect((await bare[name]!.execute(input)).split('\n')[0]).toMatch(/^Verdict: error, this sandbox cannot /);
      }
    });

    it('watches for a bounded time or event count and stops the watch', async () => {
      const { sandbox, tools } = await setup({ maxWatchSeconds: 1 });
      await sandbox.fs.mkdir('/w');
      const quiet = await tools.watch_files!.execute({ path: '/w', duration_seconds: 600 });
      expect(quiet).toBe('Verdict: ok, no events under /w after 1s');

      const pending = tools.watch_files!.execute({ path: '/w', max_events: 2, duration_seconds: 5 });
      await new Promise((resolve) => setTimeout(resolve, 30));
      await sandbox.writeFiles([{ path: '/w/a.txt', content: 'a' }]);
      await sandbox.writeFiles([{ path: '/w/b.txt', content: 'b' }]);
      const seen = await pending;
      expect(seen.split('\n')[0]).toBe('Verdict: ok, 2 event(s) under /w at the 2 event limit');
      expect(seen).toContain('created /w/a.txt');
      expect(seen).toContain('created /w/b.txt');
    });
  });

  describe('streamed run_command', () => {
    /** A sandbox whose detached command feeds chunks through a followed log. */
    function streamingSandbox(
      chunks: Array<{ stream: 'stdout' | 'stderr'; data: string }>,
      followFails: false | 'error' | 'unsupported' = false,
      requests: Array<Record<string, unknown>> = [],
    ) {
      const text = (stream: 'stdout' | 'stderr') =>
        new TextEncoder().encode(
          chunks
            .filter((c) => c.stream === stream)
            .map((c) => c.data)
            .join(''),
        );
      const finished = new CommandFinished('l' as never, 'c1', 0, text('stdout'), text('stderr'), { durationMs: 1 });
      let exit: () => void = () => undefined;
      const exited = new Promise<void>((resolve) => {
        exit = resolve;
      });
      const command = {
        async *logs() {
          if (followFails === 'error') throw new Error('feed dropped');
          if (followFails === 'unsupported') throw new SandboxWsError('unsupported', 'no durable logs');
          for (const c of chunks) yield { ...c, bytes: new Uint8Array(), cursor: 1 };
        },
        // The live feed never ends on its own; the command's exit stops the read.
        async *stream() {
          for (const c of chunks) yield { stream: c.stream, data: new TextEncoder().encode(c.data) };
          exit();
          await new Promise(() => undefined);
        },
        wait: async () => {
          if (followFails === 'unsupported') await exited;
          return finished;
        },
      };
      return {
        runCommand: async (params: Record<string, unknown>) => {
          requests.push(params);
          return command;
        },
        sandboxId: 'lease-x',
      } as unknown as ToolSandbox;
    }

    it('asks the stack to stream the command output', async () => {
      const requests: Array<Record<string, unknown>> = [];
      const tools = createSandboxTools(streamingSandbox([{ stream: 'stdout', data: 'x' }], false, requests));
      for await (const _ of tools.run_command!.stream!({ command: 'x' })) {
        // drain
      }
      expect(requests[0]).toMatchObject({ detached: true, streamOutput: true });
    });

    it('reads the live feed until the command exits when the stack keeps no durable logs', async () => {
      const tools = createSandboxTools(
        streamingSandbox(
          [
            { stream: 'stdout', data: 'one\n' },
            { stream: 'stdout', data: 'two\n' },
          ],
          'unsupported',
        ),
        { streamIntervalMs: 0 },
      );
      const seen: string[] = [];
      for await (const text of tools.run_command!.stream!({ command: 'build' })) seen.push(text);
      expect(seen).toHaveLength(3);
      expect(seen[0]).toBe('Verdict: running\nstdout so far:\none\n');
      expect(seen[1]).toBe('Verdict: running\nstdout so far:\none\ntwo\n');
      expect(seen[2]!.split('\n')[0]).toBe('Verdict: ok (exit 0)');
    });

    it('yields progress snapshots, then the same final text execute would return', async () => {
      const tools = createSandboxTools(
        streamingSandbox([
          { stream: 'stdout', data: 'one\n' },
          { stream: 'stderr', data: 'warn\n' },
          { stream: 'stdout', data: 'two\n' },
        ]),
        { streamIntervalMs: 0 },
      );
      const seen: string[] = [];
      for await (const text of tools.run_command!.stream!({ command: 'build' })) seen.push(text);
      expect(seen.length).toBe(4);
      expect(seen[0]).toBe('Verdict: running\nstdout so far:\none\n');
      expect(seen[2]).toContain('stderr so far:\nwarn');
      expect(seen[3]!.split('\n')[0]).toBe('Verdict: ok (exit 0)');
      expect(seen[3]).toContain('one\ntwo');
    });

    it('throttles snapshots and still ends with the final result', async () => {
      const tools = createSandboxTools(
        streamingSandbox([
          { stream: 'stdout', data: 'a' },
          { stream: 'stdout', data: 'b' },
        ]),
        {
          streamIntervalMs: 60_000,
        },
      );
      const seen: string[] = [];
      for await (const text of tools.run_command!.stream!({ command: 'x' })) seen.push(text);
      expect(seen.length).toBe(2);
      expect(seen.at(-1)!.split('\n')[0]).toBe('Verdict: ok (exit 0)');
    });

    it('falls back to the final result when the log feed fails, and refuses bad input as one value', async () => {
      const tools = createSandboxTools(streamingSandbox([{ stream: 'stdout', data: 'hi\n' }], 'error'));
      const seen: string[] = [];
      for await (const text of tools.run_command!.stream!({ command: 'x' })) seen.push(text);
      expect(seen).toHaveLength(1);
      expect(seen[0]).toContain('hi');
      const bad: string[] = [];
      for await (const text of tools.run_command!.stream!({})) bad.push(text);
      expect(bad).toHaveLength(1);
      expect(bad[0]).toMatch(/^Verdict: error, invalid input/);
    });

    it('the AI SDK adapter streams only when asked', async () => {
      const tools = createSandboxTools(streamingSandbox([{ stream: 'stdout', data: 'x\n' }]), { streamIntervalMs: 0 });
      const plain = toAiSdkTools(tools).run_command!.execute({ command: 'x' });
      expect(typeof (plain as Promise<string>).then).toBe('function');
      await plain;
      const streamed = toAiSdkTools(tools, { streamOutput: true }).run_command!.execute({ command: 'x' });
      const out: string[] = [];
      for await (const text of streamed as AsyncIterable<string>) out.push(text);
      expect(out.at(-1)!.split('\n')[0]).toBe('Verdict: ok (exit 0)');
      const other = toAiSdkTools(tools, { streamOutput: true }).read_file!.execute({ path: '/x' });
      expect(typeof (other as Promise<string>).then).toBe('function');
      await other;
    });

    it('the Mastra adapter writes snapshots to the tool writer only when asked and given one', async () => {
      const tools = createSandboxTools(streamingSandbox([{ stream: 'stdout', data: 'x\n' }]), { streamIntervalMs: 0 });
      const written: unknown[] = [];
      const writer = { write: async (chunk: unknown) => void written.push(chunk) };
      const streamed = toMastraTools(tools, { streamOutput: true })[0]!;
      const final = await streamed.execute({ command: 'x' }, { writer });
      expect(written.length).toBeGreaterThan(1);
      expect(final).toBe(written.at(-1));
      expect(final.split('\n')[0]).toBe('Verdict: ok (exit 0)');

      written.length = 0;
      await streamed.execute({ command: 'x' });
      await toMastraTools(tools)[0]!.execute({ command: 'x' }, { writer });
      expect(written).toHaveLength(0);
    });
  });

  it('clip keeps head and tail', () => {
    const { text, clipped } = clip('a'.repeat(50) + 'b'.repeat(50), 20);
    expect(clipped).toBe(true);
    expect(text.startsWith('a'.repeat(12))).toBe(true);
    expect(text.endsWith('b'.repeat(8))).toBe(true);
    expect(clip('short', 20)).toEqual({ text: 'short', clipped: false });
  });
});

describe('adapters', () => {
  it('map the bundle to each framework shape with no framework dependency', async () => {
    const { tools } = await setup({ allow: ['run_command', 'read_file'] });
    const ai = toAiSdkTools(tools);
    expect(Object.keys(ai)).toEqual(['run_command', 'read_file']);
    expect(await ai.run_command!.execute({ command: 'echo hi' })).toContain('hi');

    const mastra = toMastraTools(tools);
    expect(mastra.map((t) => t.id)).toEqual(['run_command', 'read_file']);
    expect(await mastra[0]!.execute({ context: { command: 'echo wrapped' } })).toContain('wrapped');
    expect(await mastra[0]!.execute({ command: 'echo bare' })).toContain('bare');

    const openai = toOpenAiAgentsTools(tools);
    expect(openai.map((t) => t.name)).toEqual(['run_command', 'read_file']);
    expect(openai[0]!.parameters).toBe(toolSchemas.run_command);
    expect(await openai[0]!.execute({ command: 'echo oa' })).toContain('oa');
  });
});
