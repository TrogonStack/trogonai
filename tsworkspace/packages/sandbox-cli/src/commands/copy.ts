import * as fs from 'node:fs';

import { leaseId } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';
import { parseCopyPath } from '../parsing.js';

interface CopyOptions extends GlobalOptions {
  user?: string;
}

export function registerCopyCommand(program: Command): void {
  program
    .command('copy <src> <dst>')
    .alias('cp')
    .description(
      'Copy a file between the local filesystem and a sandbox. Remote paths use <lease-id>:/path, ' +
        'e.g. `sandbox copy ./local.txt my-lease:/workspace/remote.txt`.',
    )
    .option('--user <name>', 'read/write as this sandbox user')
    .action(async (src: string, dst: string, opts: CopyOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const remoteSrc = parseCopyPath(src);
      const remoteDst = parseCopyPath(dst);

      if (remoteSrc && remoteDst) {
        throw new Error('remote-to-remote copy is not supported');
      }
      if (!remoteSrc && !remoteDst) {
        throw new Error(
          'local-to-local copy is not supported: exactly one of <src>/<dst> must be a remote <lease-id>:/path',
        );
      }

      const { client } = await connectClient(global);
      try {
        if (remoteDst) {
          const bytes = fs.readFileSync(src);
          await client.writeFiles(leaseId(remoteDst.leaseId), [{ path: remoteDst.path, content: Array.from(bytes) }], {
            asUser: opts.user,
          });
          process.stdout.write(`copied ${src} -> ${dst}\n`);
          return;
        }

        const remote = remoteSrc!;
        const response = await client.readFile(leaseId(remote.leaseId), remote.path, { asUser: opts.user });
        if (response.outcome === 'not_found') {
          throw new Error(`no such file: ${remote.path}`);
        }
        fs.writeFileSync(dst, Buffer.from(response.data));
        process.stdout.write(`copied ${src} -> ${dst}\n`);
      } finally {
        client.close();
      }
    });
}
