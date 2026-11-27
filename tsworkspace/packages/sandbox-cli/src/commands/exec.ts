import { leaseId, Sandbox, type UnixSignal } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, requireTenant, type GlobalOptions } from '../context.js';
import { parseImageReference, parseKeyValueFlags, validateTagCount } from '../parsing.js';

interface ExecOptions extends GlobalOptions {
  cwd?: string;
  env: string[];
  user?: string;
  detached?: boolean;
  timeout?: string;
  stream?: boolean;
}

/**
 * `remoteArgv` is everything after the first literal `--` on the original
 * command line, extracted by `main.ts` before commander ever sees it. This
 * keeps commander from trying to interpret the remote command's own flags
 * (e.g. `sandbox exec lease -- ls -la` should not have `-la` parsed by us).
 */
export function registerExecCommand(program: Command, remoteArgv: string[]): void {
  program
    .command('exec <lease-id>')
    .description('Run a command inside a sandbox lease (usage: sandbox exec <lease-id> -- <cmd> [args...])')
    .option('--cwd <dir>', 'working directory for the command')
    .option('--env <k=v>', 'environment variable (repeatable)', collectFlag, [])
    .option('--user <name>', 'run as this sandbox user')
    .option('--detached', 'print the execution id and return immediately, without waiting for completion')
    .option('--timeout <seconds>', 'command timeout in seconds')
    .option(
      '--stream',
      'publish incremental stdout/stderr chunks to the evidence stream as the command runs. Only has an ' +
        'observable effect combined with --detached (a non-detached exec already returns full output once it ' +
        "completes, there's nothing to stream to); with --detached, follow the live output afterward via " +
        '`sandbox logs --follow <lease-id> <execution-id>`.',
    )
    .action(async (leaseIdArg: string, opts: ExecOptions, command: Command) => {
      if (remoteArgv.length === 0) {
        throw new Error('missing remote command: usage `sandbox exec <lease-id> -- <cmd> [args...]`');
      }
      const [cmd, ...args] = remoteArgv;
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const env = parseKeyValueFlags(opts.env, '--env');
        const response = await client.runCommand(leaseId(leaseIdArg), [cmd, ...args], {
          cwd: opts.cwd,
          env: Object.keys(env).length > 0 ? env : undefined,
          timeoutSeconds: opts.timeout !== undefined ? Number(opts.timeout) : undefined,
          detached: opts.detached,
          streamOutput: opts.stream,
          runAsUser: opts.user,
        });

        if (response.outcome === 'started') {
          process.stdout.write(`${response.execution_id}\n`);
          return;
        }

        process.stdout.write(Buffer.from(response.stdout));
        process.stderr.write(Buffer.from(response.stderr));
        process.exitCode = response.exit_code;
      } finally {
        client.close();
      }
    });
}

function collectFlag(value: string, previous: string[]): string[] {
  return [...previous, value];
}

interface RunOptions extends GlobalOptions {
  image: string;
  name?: string;
  tag: string[];
  env: string[];
  timeout?: string;
  networkPolicy?: 'allow-all' | 'deny-all';
  user?: string;
}

export function registerRunCommand(program: Command, remoteArgv: string[]): void {
  program
    .command('run')
    .description('Create a sandbox, run a command, then stop it (usage: sandbox run --image <ref> -- <cmd> [args...])')
    .requiredOption('--image <ref>', 'OCI image reference, e.g. docker.io/library/alpine:latest')
    .option('--name <name>', 'sandbox name')
    .option('--tag <k=v>', 'sandbox tag (repeatable, max 5)', collectFlag, [])
    .option('--env <k=v>', 'environment variable (repeatable)', collectFlag, [])
    .option('--timeout <ms>', 'lease TTL in milliseconds')
    .option('--network-policy <policy>', 'allow-all or deny-all')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (opts: RunOptions, command: Command) => {
      if (remoteArgv.length === 0) {
        throw new Error('missing remote command: usage `sandbox run --image <ref> -- <cmd> [args...]`');
      }
      const [cmd, ...args] = remoteArgv;
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client, tenant } = await connectClient(global);
      try {
        const tags = parseKeyValueFlags(opts.tag, '--tag');
        validateTagCount(tags);
        const env = parseKeyValueFlags(opts.env, '--env');
        const reference = parseImageReference(opts.image);

        const sandbox = await Sandbox.create(client, {
          tenant: requireTenant(tenant),
          image: { kind: 'registry', reference },
          name: opts.name,
          tags: Object.keys(tags).length > 0 ? tags : undefined,
          env: Object.keys(env).length > 0 ? env : undefined,
          timeoutMs: opts.timeout !== undefined ? Number(opts.timeout) : undefined,
          networkPolicy: opts.networkPolicy,
        });

        let exitCode = 1;
        try {
          const result = await sandbox.runCommand({ cmd, args, user: opts.user });
          process.stdout.write(result.stdout());
          process.stderr.write(result.stderr());
          exitCode = result.exitCode;
        } finally {
          try {
            await sandbox.stop();
          } catch (err) {
            process.stderr.write(`warning: failed to stop lease ${sandbox.sandboxId}: ${errorMessage(err)}\n`);
          }
        }
        process.exitCode = exitCode;
      } finally {
        client.close();
      }
    });
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

const SIGNALS: UnixSignal[] = ['term', 'kill', 'int', 'hup', 'quit', 'usr1', 'usr2'];

function assertUnixSignal(value: string): asserts value is UnixSignal {
  if (!(SIGNALS as string[]).includes(value)) {
    throw new Error(`invalid --signal "${value}": expected one of ${SIGNALS.join(', ')}`);
  }
}

export function registerKillCommand(program: Command): void {
  program
    .command('kill <lease-id> <execution-id>')
    .description('Send a signal to a running command')
    .option('--signal <signal>', 'term, kill, int, hup, quit, usr1, or usr2', 'term')
    .action(async (leaseIdArg: string, executionId: string, opts: { signal: string }, command: Command) => {
      assertUnixSignal(opts.signal);
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const response = await client.killCommand(leaseId(leaseIdArg), executionId, opts.signal);
        process.stdout.write(`${response.outcome}\n`);
      } finally {
        client.close();
      }
    });
}

export function registerLogsCommand(program: Command): void {
  program
    .command('logs <lease-id> <execution-id>')
    .description(
      'Print stdout/stderr for a command. Without --follow this is post-hoc only: it replays the two buffered ' +
        'chunks captured at completion, there is no live streaming. --follow requires the execution to have ' +
        'been started with `sandbox exec --stream` (i.e. wire `stream_output: true`); otherwise no chunks were ' +
        'ever published to the evidence stream, and --follow silently falls back to printing the same post-hoc ' +
        'output once the command completes.',
    )
    .option('--poll-interval-ms <ms>', 'polling interval while waiting for completion', '200')
    .option(
      '--follow',
      'live-tail the command output from the evidence stream as it runs, in addition to the post-hoc summary ' +
        'printed once the command completes',
    )
    .action(
      async (
        leaseIdArg: string,
        executionId: string,
        opts: { pollIntervalMs: string; follow?: boolean },
        command: Command,
      ) => {
        const global = command.optsWithGlobals() as GlobalOptions;
        const { client } = await connectClient(global);
        try {
          if (opts.follow) {
            await followLogs(client, leaseIdArg, executionId, Number(opts.pollIntervalMs));
            return;
          }
          const intervalMs = Number(opts.pollIntervalMs);
          for (;;) {
            const response = await client.pollCommand(leaseId(leaseIdArg), executionId);
            if (response.outcome === 'completed') {
              process.stdout.write(Buffer.from(response.stdout));
              process.stderr.write(Buffer.from(response.stderr));
              process.exitCode = response.exit_code;
              return;
            }
            if (response.outcome === 'not_found') {
              throw new Error(`execution ${executionId} not found`);
            }
            await new Promise((resolve) => setTimeout(resolve, intervalMs));
          }
        } finally {
          client.close();
        }
      },
    );
}

/**
 * `--follow` cannot faithfully reconstruct a `Command.stream()` handle:
 * `Command.stream()` requires the in-process `Command` instance's own
 * `streaming` flag (set only at `runCommand()` construction time), and there
 * is no wire call to retrospectively ask "was execution X started with
 * stream_output?" for an execution id captured from a prior, possibly
 * different, CLI invocation. So this reads the lease's evidence stream
 * directly, filtering for this execution's `command.<execution_id>` chunks
 * exactly as `Command.stream()` does internally, while a concurrent poll
 * loop watches for completion via `pollCommand` and ends the tail once the
 * command finishes (printing the same post-hoc summary the non-follow path
 * prints). If the execution was not started with `--stream`, no chunks ever
 * arrive on the evidence stream; that is expected and silent, not an error.
 */
async function followLogs(
  client: Awaited<ReturnType<typeof connectClient>>['client'],
  leaseIdArg: string,
  executionId: string,
  pollIntervalMs: number,
): Promise<void> {
  process.stderr.write(
    '--follow only shows live output for executions started with `sandbox exec --stream`; ' +
      'otherwise only the post-hoc summary below will appear once the command completes.\n',
  );

  const suffix = `command.${executionId}`;
  const lease = leaseId(leaseIdArg);

  // Fire-and-forget: the caller's `finally { client.close(); }` ends the
  // underlying evidence-stream queue once we return below, which naturally
  // terminates this for-await loop without needing an explicit unsubscribe
  // (the wire protocol has none).
  void (async () => {
    for await (const event of client.evidenceStream(lease)) {
      if (event.subjectSuffix !== suffix) {
        continue;
      }
      const text = typeof event.payload === 'string' ? event.payload : new TextDecoder().decode(event.payload);
      const chunk = JSON.parse(text) as { stream: 'stdout' | 'stderr'; data: number[] };
      const bytes = Buffer.from(chunk.data);
      if (chunk.stream === 'stdout') {
        process.stdout.write(bytes);
      } else {
        process.stderr.write(bytes);
      }
    }
  })();

  for (;;) {
    const response = await client.pollCommand(lease, executionId);
    if (response.outcome === 'completed') {
      process.stdout.write(Buffer.from(response.stdout));
      process.stderr.write(Buffer.from(response.stderr));
      process.exitCode = response.exit_code;
      return;
    }
    if (response.outcome === 'not_found') {
      throw new Error(`execution ${executionId} not found`);
    }
    await new Promise((resolve) => setTimeout(resolve, pollIntervalMs));
  }
}
