import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';
import { isoFromWireSystemTime, printJson, printTable } from '../output.js';

export function registerDrivesCommand(program: Command): void {
  const drives = program.command('drives').description('Manage sandbox drives');

  drives
    .command('list')
    .alias('ls')
    .description('List drives')
    .action(async (_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const records = await client.listDrives();
        if (global.json) {
          printJson(records);
          return;
        }
        printTable(
          ['NAME', 'TENANT', 'SIZE(GiB)', 'STORAGE', 'CREATED', 'ATTACHED LEASE'],
          records.map((r) => [
            r.id.name,
            r.tenant,
            String(r.size_gib),
            r.bucket ?? (r.provider_volume ? `${r.provider_volume.provider}:${r.provider_volume.volume_id}` : ''),
            isoFromWireSystemTime(r.created_at),
            r.attached_lease ?? '',
          ]),
        );
      } finally {
        client.close();
      }
    });

  drives
    .command('create')
    .description('Create a drive')
    .requiredOption('--name <name>', 'drive name')
    .requiredOption('--bucket <bucket>', 'backing bucket')
    .option('--size-gib <n>', 'size in GiB')
    .action(async (opts: { name: string; bucket: string; sizeGib?: string }, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        try {
          const record = await client.createDrive(opts.name, opts.bucket, {
            sizeGib: opts.sizeGib !== undefined ? Number(opts.sizeGib) : undefined,
          });
          if (global.json) {
            printJson(record);
          } else {
            process.stdout.write(`created drive "${record.id.name}"\n`);
          }
        } catch (err) {
          // Get-or-create fallback: the wire protocol/errors.ts expose no
          // dedicated "drive already exists" error code (checked wire.ts's
          // CreateDriveResponse and errors.ts's SandboxWsError subclasses;
          // neither has one). On any create failure we fall back to looking
          // the drive up by name via listDrives() and, if found, treat that
          // as success. This is a best-effort heuristic, not a guarantee: a
          // transient failure unrelated to "already exists" would also match
          // this path if a same-named drive happens to already exist.
          const records = await client.listDrives();
          const existing = records.find((r) => r.id.name === opts.name);
          if (!existing) {
            throw err;
          }
          if (global.json) {
            printJson(existing);
          } else {
            process.stdout.write(`drive "${opts.name}" already exists\n`);
          }
        }
      } finally {
        client.close();
      }
    });

  drives
    .command('delete <name>')
    .alias('rm')
    .description('Delete a drive')
    .action(async (name: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await client.deleteDrive(name);
        process.stdout.write(`deleted drive "${name}"\n`);
      } finally {
        client.close();
      }
    });
}
