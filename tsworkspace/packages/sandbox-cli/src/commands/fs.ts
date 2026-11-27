import { leaseId, Sandbox, SandboxWsClient } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';
import { printJson } from '../output.js';

interface FsOptions extends GlobalOptions {
  user?: string;
}

// FileSystem operations need a Sandbox handle, but the CLI only ever has a
// bare lease-id string (no LeaseGranted/LeaseByNameRow to build a full
// Sandbox from). Sandbox's constructor is public and unvalidated (no
// provider/deadline check against the wire), so this lightweight wrapper
// exists purely to reach `.fs`; `provider`/`deadline` are never read by any
// FileSystem method.
function sandboxHandle(client: SandboxWsClient, leaseIdArg: string): Sandbox {
  return new Sandbox(client, { sandboxId: leaseId(leaseIdArg), provider: 'mock', deadline: new Date() });
}

function parseOctalMode(mode: string): number {
  const parsed = Number.parseInt(mode, 8);
  if (!Number.isFinite(parsed)) {
    throw new Error(`invalid mode "${mode}": expected an octal string like "755"`);
  }
  return parsed;
}

export function registerFsCommand(program: Command): void {
  const fsCommand = program
    .command('fs')
    .description('Filesystem operations inside a sandbox lease (run_command sugar, not a dedicated wire verb)');

  fsCommand
    .command('stat <lease-id> <path>')
    .description('Print file metadata (size, mode, type, mtime)')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, path: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const stat = await sandboxHandle(client, leaseIdArg).fs.stat(path, { user: opts.user });
        if (global.json) {
          printJson(stat);
          return;
        }
        process.stdout.write(`size: ${stat.size}\n`);
        process.stdout.write(`mode: ${stat.mode.toString(8)}\n`);
        process.stdout.write(`isDir: ${stat.isDir}\n`);
        process.stdout.write(`isFile: ${stat.isFile}\n`);
        process.stdout.write(`isSymlink: ${stat.isSymlink}\n`);
        process.stdout.write(`mtimeMs: ${stat.mtimeMs}\n`);
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('ls <lease-id> <path>')
    .description('List directory entries')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, path: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const entries = await sandboxHandle(client, leaseIdArg).fs.ls(path, { user: opts.user });
        if (global.json) {
          printJson(entries);
          return;
        }
        for (const entry of entries) {
          process.stdout.write(`${entry}\n`);
        }
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('exists <lease-id> <path>')
    .description('Check whether a path exists; exits 0 if it does, 1 if it does not (scriptable like `test -e`)')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, path: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const exists = await sandboxHandle(client, leaseIdArg).fs.exists(path, { user: opts.user });
        process.stdout.write(`${exists}\n`);
        process.exitCode = exists ? 0 : 1;
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('rm <lease-id> <path>')
    .description('Remove a file or directory')
    .option('--recursive', 'remove directories recursively')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, path: string, opts: FsOptions & { recursive?: boolean }, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await sandboxHandle(client, leaseIdArg).fs.rm(path, { recursive: opts.recursive, user: opts.user });
        process.stdout.write(`removed ${path}\n`);
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('mkdir <lease-id> <path>')
    .description('Create a directory (and parents as needed)')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, path: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await sandboxHandle(client, leaseIdArg).fs.mkdir(path, { user: opts.user });
        process.stdout.write(`created ${path}\n`);
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('cp <lease-id> <src> <dst>')
    .description('Copy a file or directory within a sandbox')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, src: string, dst: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await sandboxHandle(client, leaseIdArg).fs.cp(src, dst, { user: opts.user });
        process.stdout.write(`copied ${src} -> ${dst}\n`);
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('mv <lease-id> <src> <dst>')
    .description('Move/rename a file or directory within a sandbox')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, src: string, dst: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await sandboxHandle(client, leaseIdArg).fs.mv(src, dst, { user: opts.user });
        process.stdout.write(`moved ${src} -> ${dst}\n`);
      } finally {
        client.close();
      }
    });

  fsCommand
    .command('chmod <lease-id> <mode> <path>')
    .description('Change file permissions (mode is an octal string, e.g. 755)')
    .option('--user <name>', 'run as this sandbox user')
    .action(async (leaseIdArg: string, mode: string, path: string, opts: FsOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const parsedMode = parseOctalMode(mode);
        await sandboxHandle(client, leaseIdArg).fs.chmod(path, parsedMode, { user: opts.user });
        process.stdout.write(`chmod ${mode} ${path}\n`);
      } finally {
        client.close();
      }
    });
}
