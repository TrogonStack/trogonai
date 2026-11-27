import { MemorySandboxClient } from '@trogonai/sandbox-ws-client';
import type { CommandHandler, MemoryFileSystem } from '@trogonai/sandbox-ws-client';

/**
 * What the in-memory backend fakes so the agent can run with no network.
 *
 * - `git clone` copies a small built-in repository into the virtual filesystem and
 *   ignores the url except for the directory name. Nothing is fetched.
 * - `git status`, `git diff` and `git add` compare the working tree with the files as
 *   they were at clone time. They are not git: whole-file hunks trimmed to three lines
 *   of context, no renames, no ignore rules, no index.
 * - `sh -c` understands only `cat`, `ls` and `echo` with plain arguments.
 */
const SEED_FILES: Record<string, string> = {
  'README.md': '# demo\n\nA tiny project used to exercise the coding agent.\n',
  'greeting.txt': 'Hello, there!\n',
};

const decoder = new TextDecoder();

function snapshot(fs: MemoryFileSystem, root: string): Map<string, string> {
  const out = new Map<string, string>();
  for (const file of fs.tree(root).files) {
    out.set(file.path, decoder.decode(file.data));
  }
  return out;
}

/** Whole-file unified diff: the common head and tail are trimmed to three lines of context. */
function unifiedDiff(path: string, before: string | undefined, after: string | undefined): string {
  const a = before === undefined || before === '' ? [] : before.replace(/\n$/, '').split('\n');
  const b = after === undefined || after === '' ? [] : after.replace(/\n$/, '').split('\n');
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head += 1;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a.at(-1 - tail) === b.at(-1 - tail)) tail += 1;
  const ctxBefore = Math.min(3, head);
  const ctxAfter = Math.min(3, tail);
  const start = head - ctxBefore;
  const removed = a.slice(head, a.length - tail);
  const added = b.slice(head, b.length - tail);
  const body = [
    ...a.slice(start, head).map((l) => ` ${l}`),
    ...removed.map((l) => `-${l}`),
    ...added.map((l) => `+${l}`),
    ...a.slice(a.length - tail, a.length - tail + ctxAfter).map((l) => ` ${l}`),
  ];
  const oldLen = ctxBefore + removed.length + ctxAfter;
  const newLen = ctxBefore + added.length + ctxAfter;
  const header = [
    `diff --git a/${path} b/${path}`,
    ...(before === undefined ? ['new file mode 100644'] : []),
    ...(after === undefined ? ['deleted file mode 100644'] : []),
    `--- ${before === undefined ? '/dev/null' : `a/${path}`}`,
    `+++ ${after === undefined ? '/dev/null' : `b/${path}`}`,
    `@@ -${oldLen === 0 ? start : start + 1},${oldLen} +${newLen === 0 ? start : start + 1},${newLen} @@`,
  ];
  return `${[...header, ...body].join('\n')}\n`;
}

/** Registers the fake `git` and `sh` commands on a memory client. Returns the same client. */
export function withFakeRepo(client: MemorySandboxClient): MemorySandboxClient {
  let root: string | undefined;
  let baseline = new Map<string, string>();

  const git: CommandHandler = ({ args, cwd, fs }) => {
    const [sub, ...rest] = args;
    if (sub === 'clone') {
      const operands = rest.slice(rest.indexOf('--') + 1);
      const [url, dest] = operands;
      const name =
        (url ?? 'repo')
          .replace(/\.git$/, '')
          .split('/')
          .at(-1) || 'repo';
      root = fs.resolve(dest ?? name, cwd);
      for (const [path, content] of Object.entries(SEED_FILES)) {
        fs.write(`${root}/${path}`, content);
      }
      baseline = snapshot(fs, root);
      return { exitCode: 0, stderr: `Cloning into '${root}' (seeded, nothing fetched)\n` };
    }
    if (root === undefined) {
      return { exitCode: 128, stderr: 'fatal: not a git repository\n' };
    }
    if (sub === 'add') {
      return { exitCode: 0 };
    }
    const now = snapshot(fs, root);
    const paths = [...new Set([...baseline.keys(), ...now.keys()])].sort();
    const changed = paths.filter((p) => baseline.get(p) !== now.get(p));
    if (sub === 'status') {
      const lines = changed.map((p) => `${baseline.has(p) ? (now.has(p) ? ' M' : ' D') : '??'} ${p}`);
      return { exitCode: 0, stdout: `## main\n${lines.map((l) => `${l}\n`).join('')}` };
    }
    if (sub === 'diff') {
      return { exitCode: 0, stdout: changed.map((p) => unifiedDiff(p, baseline.get(p), now.get(p))).join('') };
    }
    return { exitCode: 1, stderr: `git ${sub ?? ''}: not supported by the in-memory backend\n` };
  };

  const sh: CommandHandler = ({ args, cwd, fs }) => {
    const [flag, script = ''] = args;
    if (flag !== '-c') {
      return { exitCode: 2, stderr: 'sh: only -c is supported by the in-memory backend\n' };
    }
    const [cmd, ...operands] = script.trim().split(/\s+/);
    const paths = operands.filter((o) => !o.startsWith('-'));
    try {
      if (cmd === 'echo') return { exitCode: 0, stdout: `${operands.join(' ')}\n` };
      if (cmd === 'cat') return { exitCode: 0, stdout: paths.map((p) => decoder.decode(fs.read(p, cwd))).join('') };
      if (cmd === 'ls')
        return {
          exitCode: 0,
          stdout: fs
            .list(paths[0] ?? '.', cwd)
            .map((n) => `${n}\n`)
            .join(''),
        };
    } catch {
      return { exitCode: 1, stderr: `${script}: No such file or directory\n` };
    }
    return { exitCode: 127, stderr: `sh: ${cmd}: not supported by the in-memory backend\n` };
  };

  client.registerCommand('git', git);
  client.registerCommand('sh', sh);
  return client;
}
