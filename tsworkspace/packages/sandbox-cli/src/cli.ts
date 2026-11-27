import { SandboxWsError } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { registerConfigCommand, registerLoginCommand, registerLogoutCommand } from './commands/auth.js';
import { registerConnectCommand } from './commands/connect.js';
import { registerCopyCommand } from './commands/copy.js';
import { registerDrivesCommand } from './commands/drives.js';
import { registerExecCommand, registerKillCommand, registerLogsCommand, registerRunCommand } from './commands/exec.js';
import { registerFsCommand } from './commands/fs.js';
import { registerGroupCommand, registerUserCommand } from './commands/identity.js';
import {
  registerCreateCommand,
  registerDomainCommand,
  registerExtendCommand,
  registerForkCommand,
  registerGetCommand,
  registerListCommand,
  registerRemoveCommand,
  registerStopCommand,
} from './commands/lease.js';
import { registerSessionsCommand } from './commands/sessions.js';
import { registerSnapshotsCommand, registerTagsCommand } from './commands/snapshots.js';
import { DEFAULT_PROFILE_NAME } from './config.js';

function splitOnDoubleDash(argv: string[]): { before: string[]; after: string[] } {
  const index = argv.indexOf('--');
  if (index === -1) {
    return { before: argv, after: [] };
  }
  return { before: argv.slice(0, index), after: argv.slice(index + 1) };
}

function formatError(err: unknown): string {
  if (err instanceof SandboxWsError) {
    return err.message;
  }
  if (err instanceof Error) {
    return err.message;
  }
  return String(err);
}

/**
 * Builds and runs the command tree for a given argv (in Node's
 * `process.argv` shape: [execPath, scriptPath, ...args]) and resolves to the
 * process exit code, instead of calling `process.exit` directly. Kept
 * separate from `main.ts` so tests can drive the whole CLI (arg parsing,
 * command dispatch, SDK calls) without spawning a subprocess.
 */
export async function runCli(argv: string[]): Promise<number> {
  const { before, after } = splitOnDoubleDash(argv);

  const program = new Command();
  program
    .name('sandbox')
    .description('Command-line interface for trogon-sandbox-ws-gateway')
    .option('--profile <name>', 'named config profile to use', DEFAULT_PROFILE_NAME)
    .option('--gateway-http <url>', 'override the profile gateway HTTP URL for this invocation')
    .option('--gateway-ws <url>', 'override the profile gateway WS URL for this invocation')
    .option('--api-key <key>', 'override the profile API key for this invocation')
    .option('--tenant <tenant>', 'override the profile tenant for this invocation')
    .option('--json', 'print machine-readable JSON wherever table output exists')
    .exitOverride()
    .configureOutput({
      writeErr: (str) => process.stderr.write(str),
    });

  registerLoginCommand(program);
  registerLogoutCommand(program);
  registerConfigCommand(program);
  registerCreateCommand(program);
  registerListCommand(program);
  registerForkCommand(program);
  registerStopCommand(program);
  registerRemoveCommand(program);
  registerExtendCommand(program);
  registerDomainCommand(program);
  registerGetCommand(program);
  registerExecCommand(program, after);
  registerRunCommand(program, after);
  registerKillCommand(program);
  registerLogsCommand(program);
  registerCopyCommand(program);
  registerDrivesCommand(program);
  registerSnapshotsCommand(program);
  registerTagsCommand(program);
  registerSessionsCommand(program);
  registerUserCommand(program);
  registerGroupCommand(program);
  registerFsCommand(program);
  registerConnectCommand(program);

  process.exitCode = 0;
  try {
    await program.parseAsync(before);
    return process.exitCode ?? 0;
  } catch (err) {
    if (
      err &&
      typeof err === 'object' &&
      'code' in err &&
      (err as { code: unknown }).code === 'commander.helpDisplayed'
    ) {
      return 0;
    }
    if (
      err &&
      typeof err === 'object' &&
      'exitCode' in err &&
      typeof (err as { exitCode: unknown }).exitCode === 'number'
    ) {
      return (err as { exitCode: number }).exitCode;
    }
    process.stderr.write(`Error: ${formatError(err)}\n`);
    return 1;
  }
}
