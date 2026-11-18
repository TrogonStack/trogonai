import { z } from 'zod';

import { SandboxWsError } from './errors.ts';
import { redactText } from './redact.ts';
import type { Sandbox } from './sandbox.ts';

/** A git invocation exited non-zero. `stderr` is already scrubbed of the token. */
export class GitCommandError extends SandboxWsError {
  readonly exitCode: number;
  readonly stderr: string;

  constructor(subcommand: string, exitCode: number, stderr: string) {
    super('git_failed', `git ${subcommand} failed with exit code ${exitCode}: ${stderr.trim()}`);
    this.name = 'GitCommandError';
    this.exitCode = exitCode;
    this.stderr = stderr;
  }
}

const noLeadingDash = (s: string): boolean => !s.startsWith('-');
const noControl = (s: string): boolean => !s.includes('\0') && !s.includes('\r') && !s.includes('\n');

const refSchema = z
  .string()
  .min(1)
  .refine(noLeadingDash, 'must not start with "-"')
  .refine(noControl, 'must not contain control characters');
const pathSchema = z.string().min(1).refine(noControl, 'must not contain control characters');
const tokenSchema = z.string().min(1).refine(noControl, 'token must not contain newlines or NUL');
const repoUrlSchema = z
  .string()
  .min(1)
  .refine(noLeadingDash, 'must not start with "-"')
  .refine(noControl, 'must not contain control characters')
  .refine((u) => !/^[a-z][a-z0-9+.-]*:\/\/[^/@]*:[^/@]*@/i.test(u), 'url must not embed credentials; pass `token`');

const common = {
  /** Working directory for the git process. */
  cwd: pathSchema.optional(),
  user: z.string().optional(),
};
const authed = {
  /** Delivered through a one-shot credential helper file, never argv or env. */
  token: tokenSchema.optional(),
  /** Username paired with the token. Defaults to `x-access-token`. */
  tokenUsername: z.string().min(1).refine(noControl).optional(),
};

export const gitCloneSchema = z.object({
  url: repoUrlSchema,
  /** Destination directory. Defaults to the name git derives from the url. */
  path: pathSchema.optional(),
  branch: refSchema.optional(),
  depth: z.number().int().positive().optional(),
  ...common,
  ...authed,
});
export const gitStatusSchema = z.object({ ...common });
export const gitDiffSchema = z.object({
  staged: z.boolean().optional(),
  /** Compare the working tree against this revision. */
  base: refSchema.optional(),
  paths: z.array(pathSchema).optional(),
  ...common,
});
export const gitCommitSchema = z.object({
  message: z
    .string()
    .min(1)
    .refine((s) => !s.includes('\0'), 'must not contain NUL'),
  /** Stage tracked changes first (`git commit -a`). */
  all: z.boolean().optional(),
  allowEmpty: z.boolean().optional(),
  author: z
    .object({ name: z.string().min(1).refine(noControl), email: z.string().min(1).refine(noControl) })
    .optional(),
  ...common,
});
export const gitPushSchema = z.object({
  remote: refSchema.optional(),
  branch: refSchema.optional(),
  setUpstream: z.boolean().optional(),
  /** Uses `--force-with-lease`, never a bare force. */
  forceWithLease: z.boolean().optional(),
  ...common,
  ...authed,
});
export const gitCheckoutSchema = z.object({
  ref: refSchema,
  /** Create the branch (`-b`) from the current HEAD or `startPoint`. */
  create: z.boolean().optional(),
  startPoint: refSchema.optional(),
  ...common,
});

export type GitCloneParams = z.input<typeof gitCloneSchema>;
export type GitStatusParams = z.input<typeof gitStatusSchema>;
export type GitDiffParams = z.input<typeof gitDiffSchema>;
export type GitCommitParams = z.input<typeof gitCommitSchema>;
export type GitPushParams = z.input<typeof gitPushSchema>;
export type GitCheckoutParams = z.input<typeof gitCheckoutSchema>;

export interface GitStatusEntry {
  path: string;
  /** Rename or copy source, when present. */
  originalPath?: string;
  /** Porcelain index column (`' '`, `M`, `A`, `D`, `R`, `?`, ...). */
  index: string;
  /** Porcelain worktree column. */
  worktree: string;
}
export interface GitStatusResult {
  /** `null` on a detached HEAD. */
  branch: string | null;
  upstream?: string;
  ahead: number;
  behind: number;
  entries: GitStatusEntry[];
  clean: boolean;
}
export interface GitCloneResult {
  url: string;
  path?: string;
}
export interface GitDiffResult {
  diff: string;
  empty: boolean;
}
export interface GitCommitResult {
  sha: string;
}
export interface GitPushResult {
  remote: string;
  branch?: string;
  /** git writes push progress to stderr. */
  output: string;
}
export interface GitCheckoutResult {
  ref: string;
  created: boolean;
}

function parse<S extends z.ZodType>(schema: S, input: unknown, what: string): z.output<S> {
  const parsed = schema.safeParse(input);
  if (!parsed.success) {
    const detail = parsed.error.issues.map((i) => `${i.path.join('.')}: ${i.message}`).join('; ');
    throw new SandboxWsError('invalid_request', `invalid git ${what} options: ${detail}`);
  }
  return parsed.data;
}

function shellQuote(s: string): string {
  return `'${s.replaceAll("'", `'\\''`)}'`;
}

/** Parses `git status --porcelain=v1 -b`. */
export function parseGitStatus(text: string): GitStatusResult {
  const result: GitStatusResult = { branch: null, ahead: 0, behind: 0, entries: [], clean: true };
  for (const line of text.split('\n')) {
    if (line.length === 0) continue;
    if (line.startsWith('## ')) {
      const head = line.slice(3);
      const trackingStart = head.endsWith(']') ? head.indexOf(' [') : -1;
      const names = trackingStart < 0 ? head : head.slice(0, trackingStart);
      const tracking = trackingStart < 0 ? '' : head.slice(trackingStart + 2, -1);
      const [local, upstream] = names.split('...');
      if (local === undefined || local.startsWith('HEAD (no branch)')) {
        result.branch = null;
      } else {
        result.branch = local.replace(/^No commits yet on /, '').replace(/^Initial commit on /, '');
      }
      if (upstream !== undefined) result.upstream = upstream;
      const ahead = /ahead (\d+)/.exec(tracking);
      const behind = /behind (\d+)/.exec(tracking);
      result.ahead = ahead ? Number(ahead[1]) : 0;
      result.behind = behind ? Number(behind[1]) : 0;
      continue;
    }
    const index = line[0] ?? ' ';
    const worktree = line[1] ?? ' ';
    const rest = line.slice(3);
    const arrow = rest.indexOf(' -> ');
    const entry: GitStatusEntry =
      (index === 'R' || index === 'C' || worktree === 'R' || worktree === 'C') && arrow >= 0
        ? { path: rest.slice(arrow + 4), originalPath: rest.slice(0, arrow), index, worktree }
        : { path: rest, index, worktree };
    result.entries.push(entry);
  }
  result.clean = result.entries.length === 0;
  return result;
}

/**
 * git helpers over `runCommand`. A token is written to a one-shot credential
 * helper script (mode 0700) with `writeFiles`, handed to git through
 * `-c credential.helper=<path>`, registered for output redaction, and the file
 * is removed in a `finally`. The token never appears in argv, env or a shell
 * command line. It does cross the wire inside the `writeFiles` payload and sits
 * in the guest file for the duration of one git call.
 */
export class SandboxGit {
  constructor(private readonly sandbox: Sandbox) {}

  private async git(
    args: string[],
    opts: { cwd?: string; user?: string; token?: string; tokenUsername?: string },
    label: string,
  ): Promise<{ stdout: string; stderr: string }> {
    let helperPath: string | undefined;
    const redactValues: string[] = [];
    const finalArgs = [...args];
    let failure: unknown;
    let output: { stdout: string; stderr: string } | undefined;
    try {
      if (opts.token !== undefined) {
        helperPath = `/tmp/.sbx-git-cred-${crypto.randomUUID()}`;
        const script =
          '#!/bin/sh\n' +
          `case "$1" in get) printf 'username=%s\\npassword=%s\\n' ${shellQuote(opts.tokenUsername ?? 'x-access-token')} ${shellQuote(opts.token)};; esac\n`;
        await this.sandbox.writeFiles([{ path: helperPath, content: script, mode: 0o700 }], { user: opts.user });
        redactValues.push(opts.token);
        finalArgs.unshift('-c', 'credential.helper=', '-c', `credential.helper=${helperPath}`);
      }
      const result = await this.sandbox.runCommand({
        cmd: 'git',
        args: finalArgs,
        cwd: opts.cwd,
        user: opts.user,
        redactValues: redactValues.length > 0 ? redactValues : undefined,
      });
      const stdout = redactText(result.stdout(), redactValues);
      const stderr = redactText(result.stderr(), redactValues);
      if (result.exitCode !== 0) {
        throw new GitCommandError(label, result.exitCode, stderr);
      }
      output = { stdout, stderr };
    } catch (err) {
      failure = err;
    } finally {
      if (helperPath !== undefined) {
        try {
          await this.sandbox.fs.rm(helperPath, { user: opts.user });
        } catch (rmErr) {
          failure ??= new SandboxWsError('git_cleanup_failed', `could not remove credential helper ${helperPath}`, {
            cause: rmErr,
          });
        }
      }
    }
    if (failure !== undefined) {
      throw failure;
    }
    return output!;
  }

  async clone(params: GitCloneParams): Promise<GitCloneResult> {
    const p = parse(gitCloneSchema, params, 'clone');
    const args = ['clone'];
    if (p.branch !== undefined) args.push('--branch', p.branch);
    if (p.depth !== undefined) args.push('--depth', String(p.depth));
    args.push('--', p.url);
    if (p.path !== undefined) args.push(p.path);
    await this.git(args, p, 'clone');
    return { url: p.url, path: p.path };
  }

  async status(params: GitStatusParams = {}): Promise<GitStatusResult> {
    const p = parse(gitStatusSchema, params, 'status');
    const { stdout } = await this.git(['status', '--porcelain=v1', '-b'], p, 'status');
    return parseGitStatus(stdout);
  }

  async diff(params: GitDiffParams = {}): Promise<GitDiffResult> {
    const p = parse(gitDiffSchema, params, 'diff');
    const args = ['diff', '--no-color'];
    if (p.staged) args.push('--staged');
    if (p.base !== undefined) args.push(p.base);
    if (p.paths !== undefined && p.paths.length > 0) args.push('--', ...p.paths);
    const { stdout } = await this.git(args, p, 'diff');
    return { diff: stdout, empty: stdout.length === 0 };
  }

  async commit(params: GitCommitParams): Promise<GitCommitResult> {
    const p = parse(gitCommitSchema, params, 'commit');
    const args: string[] = [];
    if (p.author !== undefined) args.push('-c', `user.name=${p.author.name}`, '-c', `user.email=${p.author.email}`);
    args.push('commit', '-m', p.message);
    if (p.all) args.push('-a');
    if (p.allowEmpty) args.push('--allow-empty');
    await this.git(args, p, 'commit');
    const { stdout } = await this.git(['rev-parse', 'HEAD'], p, 'rev-parse');
    return { sha: stdout.trim() };
  }

  async push(params: GitPushParams = {}): Promise<GitPushResult> {
    const p = parse(gitPushSchema, params, 'push');
    if (p.branch !== undefined && p.remote === undefined) {
      throw new SandboxWsError('invalid_request', 'invalid git push options: branch requires remote');
    }
    const args = ['push'];
    if (p.setUpstream) args.push('--set-upstream');
    if (p.forceWithLease) args.push('--force-with-lease');
    if (p.remote !== undefined) args.push(p.remote);
    if (p.branch !== undefined) args.push(p.branch);
    const { stdout, stderr } = await this.git(args, p, 'push');
    return { remote: p.remote ?? 'origin', branch: p.branch, output: `${stdout}${stderr}` };
  }

  async checkout(params: GitCheckoutParams): Promise<GitCheckoutResult> {
    const p = parse(gitCheckoutSchema, params, 'checkout');
    const args = ['checkout'];
    if (p.create) args.push('-b');
    args.push(p.ref);
    if (p.startPoint !== undefined) {
      if (!p.create) {
        throw new SandboxWsError('invalid_request', 'invalid git checkout options: startPoint requires create');
      }
      args.push(p.startPoint);
    }
    await this.git(args, p, 'checkout');
    return { ref: p.ref, created: p.create ?? false };
  }
}
