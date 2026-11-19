import { describe, expect, it } from 'vitest';

import {
  GitCommandError,
  MemorySandboxClient,
  REDACTION_MARKER,
  Sandbox,
  SandboxWsError,
  parseGitStatus,
  tenantId,
} from '../src/index.ts';
import type { CommandContext } from '../src/index.ts';

const TOKEN = 'ghp_s3cr3tTokenValue123';

interface Call {
  args: string[];
  env: Record<string, string>;
  helperBody?: string;
  helperPresent: boolean;
}

type Reply = { exitCode: number; stdout?: string; stderr?: string };

async function setup(respond: (args: string[], ctx: CommandContext) => Reply) {
  const calls: Call[] = [];
  const client = new MemorySandboxClient();
  client.registerCommand('git', (ctx) => {
    const helper = ctx.args.find((a) => a.startsWith('credential.helper=/'))?.slice('credential.helper='.length);
    const present = helper !== undefined && ctx.fs.exists(helper);
    calls.push({
      args: ctx.args,
      env: ctx.env,
      helperPresent: present,
      helperBody: present ? new TextDecoder().decode(ctx.fs.read(helper)) : undefined,
    });
    return respond(ctx.args, ctx);
  });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
  return { client, sandbox, calls };
}

const leftoverHelpers = (client: MemorySandboxClient, sandbox: Sandbox) => {
  const fs = client.filesystem(sandbox.sandboxId);
  return fs.exists('/tmp') ? fs.list('/tmp').filter((n) => n.startsWith('.sbx-git-cred-')) : [];
};

describe('sandbox.git token handling', () => {
  it('delivers the token through a helper file, never argv or env, and removes it', async () => {
    const { client, sandbox, calls } = await setup(() => ({ exitCode: 0 }));
    await sandbox.git.clone({ url: 'https://example.com/o/r.git', path: '/work/r', token: TOKEN });
    expect(calls).toHaveLength(1);
    const call = calls[0]!;
    expect(JSON.stringify(call.args)).not.toContain(TOKEN);
    expect(JSON.stringify(call.env)).not.toContain(TOKEN);
    expect(call.helperPresent).toBe(true);
    expect(call.helperBody).toContain(TOKEN);
    expect(call.args.slice(0, 4)).toEqual([
      '-c',
      'credential.helper=',
      '-c',
      expect.stringMatching(/^credential\.helper=\/tmp\/\.sbx-git-cred-/),
    ]);
    expect(leftoverHelpers(client, sandbox)).toEqual([]);
  });

  it('removes the helper and redacts the token when git fails', async () => {
    const { client, sandbox } = await setup(() => ({ exitCode: 128, stderr: `fatal: auth failed for ${TOKEN}` }));
    const err = await sandbox.git.push({ remote: 'origin', branch: 'main', token: TOKEN }).catch((e) => e);
    expect(err).toBeInstanceOf(GitCommandError);
    expect(err.exitCode).toBe(128);
    expect(err.message).not.toContain(TOKEN);
    expect(err.stderr).toContain(REDACTION_MARKER);
    expect(leftoverHelpers(client, sandbox)).toEqual([]);
  });

  it('removes the helper when the handler throws', async () => {
    const { client, sandbox } = await setup(() => {
      throw new Error('boom');
    });
    await expect(sandbox.git.push({ token: TOKEN })).rejects.toBeDefined();
    expect(leftoverHelpers(client, sandbox)).toEqual([]);
  });

  it('writes no helper when no token is given', async () => {
    const { sandbox, calls } = await setup(() => ({ exitCode: 0 }));
    await sandbox.git.push({ remote: 'origin', branch: 'main' });
    expect(calls[0]!.args).toEqual(['push', 'origin', 'main']);
  });

  it('redacts the token from successful push output', async () => {
    const { sandbox } = await setup(() => ({ exitCode: 0, stderr: `remote: ${TOKEN}\n` }));
    const result = await sandbox.git.push({ token: TOKEN });
    expect(result.output).not.toContain(TOKEN);
  });

  it('quotes tokens containing single quotes in the helper', async () => {
    const { sandbox, calls } = await setup(() => ({ exitCode: 0 }));
    await sandbox.git.push({ token: "tok'en;rm -rf" });
    expect(calls[0]!.helperBody).toContain(`'tok'\\''en;rm -rf'`);
  });
});

describe('sandbox.git validation', () => {
  it('rejects urls with embedded credentials and dash-prefixed refs', async () => {
    const { sandbox, calls } = await setup(() => ({ exitCode: 0 }));
    await expect(sandbox.git.clone({ url: 'https://u:pw@example.com/r.git' })).rejects.toBeInstanceOf(SandboxWsError);
    await expect(sandbox.git.clone({ url: '--upload-pack=x' })).rejects.toBeInstanceOf(SandboxWsError);
    await expect(sandbox.git.checkout({ ref: '-f' })).rejects.toBeInstanceOf(SandboxWsError);
    await expect(sandbox.git.push({ token: 'a\nb' })).rejects.toBeInstanceOf(SandboxWsError);
    expect(calls).toHaveLength(0);
  });
});

describe('sandbox.git verbs', () => {
  it('parses status', async () => {
    const out = '## main...origin/main [ahead 2, behind 1]\n M src/a.ts\n?? new.txt\nR  old.ts -> new.ts\n';
    const { sandbox, calls } = await setup(() => ({ exitCode: 0, stdout: out }));
    const status = await sandbox.git.status({ cwd: '/work' });
    expect(calls[0]!.args).toEqual(['status', '--porcelain=v1', '-b']);
    expect(status).toMatchObject({ branch: 'main', upstream: 'origin/main', ahead: 2, behind: 1, clean: false });
    expect(status.entries).toEqual([
      { path: 'src/a.ts', index: ' ', worktree: 'M' },
      { path: 'new.txt', index: '?', worktree: '?' },
      { path: 'new.ts', originalPath: 'old.ts', index: 'R', worktree: ' ' },
    ]);
    expect(parseGitStatus('## HEAD (no branch)\n').branch).toBeNull();
    expect(parseGitStatus('## main\n').clean).toBe(true);
  });

  it('builds diff, commit and checkout argv', async () => {
    const { sandbox, calls } = await setup((args) =>
      args.includes('rev-parse') ? { exitCode: 0, stdout: 'abc123\n' } : { exitCode: 0, stdout: 'diff --git a b\n' },
    );
    const diff = await sandbox.git.diff({ staged: true, base: 'HEAD~1', paths: ['a.ts'] });
    expect(diff).toEqual({ diff: 'diff --git a b\n', empty: false });
    expect(calls[0]!.args).toEqual(['diff', '--no-color', '--staged', 'HEAD~1', '--', 'a.ts']);
    const commit = await sandbox.git.commit({ message: 'feat: x', all: true, author: { name: 'A', email: 'a@x.io' } });
    expect(commit.sha).toBe('abc123');
    expect(calls[1]!.args).toEqual(['-c', 'user.name=A', '-c', 'user.email=a@x.io', 'commit', '-m', 'feat: x', '-a']);
    const co = await sandbox.git.checkout({ ref: 'topic', create: true, startPoint: 'main' });
    expect(co).toEqual({ ref: 'topic', created: true });
    expect(calls[3]!.args).toEqual(['checkout', '-b', 'topic', 'main']);
  });
});
