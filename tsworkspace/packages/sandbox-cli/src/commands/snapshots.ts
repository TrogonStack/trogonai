import { digest } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';
import { isoFromUnixSeconds, isoFromWireSystemTime, printJson, printTable } from '../output.js';

export function registerSnapshotsCommand(program: Command): void {
  const snapshots = program.command('snapshots').description('Inspect published snapshots');

  snapshots
    .command('list')
    .description('List snapshots')
    .action(async (_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { snapshots: rows } = await client.listSnapshots();
        if (global.json) {
          printJson(rows);
          return;
        }
        printTable(
          ['DIGEST', 'TYPE', 'TENANT', 'PUBLISHED', 'BYTES', 'REPOSITORY'],
          rows.map((row) => [
            row.digest,
            row.artifact_type,
            row.tenant,
            row.published_at !== null ? isoFromUnixSeconds(row.published_at) : '',
            row.bytes !== null ? String(row.bytes) : '',
            `${row.registry}/${row.repository}`,
          ]),
        );
      } finally {
        client.close();
      }
    });

  snapshots
    .command('get <digest>')
    .description('Fetch a single snapshot by digest')
    .action(async (digestArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const response = await client.snapshotGet(digest(digestArg));
        if (response.outcome === 'not_found') {
          throw new Error(`snapshot "${digestArg}" not found`);
        }
        const row = response.row;
        if (global.json) {
          printJson(row);
          return;
        }
        process.stdout.write(`digest: ${row.digest}\n`);
        process.stdout.write(`tenant: ${row.tenant}\n`);
        process.stdout.write(`artifact_type: ${row.artifact_type}\n`);
        process.stdout.write(`created_at: ${isoFromWireSystemTime(row.created_at)}\n`);
        if (row.parent) {
          process.stdout.write(`parent: ${row.parent}\n`);
        }
      } finally {
        client.close();
      }
    });

  snapshots
    .command('tree <digest>')
    .description(
      'Show the ancestry of a snapshot. Note: lineage may currently be flat (a single node) pending ' +
        'backend/builder support for real parent-chain construction; the wire response shape supports ' +
        'multi-node ancestry once that lands.',
    )
    .action(async (digestArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { nodes } = await client.snapshotTree(digest(digestArg));
        if (global.json) {
          printJson(nodes);
          return;
        }
        printTable(
          ['DIGEST', 'PARENT', 'TYPE', 'CREATED'],
          nodes.map((node) => [
            node.digest,
            node.parent ?? '',
            node.artifact_type,
            isoFromWireSystemTime(node.created_at),
          ]),
        );
      } finally {
        client.close();
      }
    });
}

export function registerTagsCommand(program: Command): void {
  const tags = program.command('tags').description('Inspect snapshot tags');

  tags
    .command('list')
    .description('List snapshot tags')
    .action(async (_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { tags: rows } = await client.listTags();
        if (global.json) {
          printJson(rows);
          return;
        }
        printTable(
          ['NAME', 'TENANT', 'DIGEST', 'TYPE', 'SET AT'],
          rows.map((row) => [row.name, row.tenant, row.digest, row.artifact_type, isoFromUnixSeconds(row.set_at)]),
        );
      } finally {
        client.close();
      }
    });

  tags
    .command('set <name> <digest>')
    .description('Point a named snapshot tag at a digest')
    .action(async (name: string, digestArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const response = await client.setSnapshotTag(name, digest(digestArg));
        if (response.outcome === 'denied') {
          throw new Error(`setting tag "${name}" to ${digestArg} was denied`);
        }
        process.stdout.write(`tag "${name}" set to ${digestArg}\n`);
      } finally {
        client.close();
      }
    });

  tags
    .command('delete <name>')
    .alias('rm')
    .description('Delete a named snapshot tag')
    .action(async (name: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const response = await client.deleteSnapshotTag(name);
        if (response.outcome === 'not_found') {
          throw new Error(`tag "${name}" not found`);
        }
        if (response.outcome === 'denied') {
          throw new Error(`deleting tag "${name}" was denied`);
        }
        process.stdout.write(`tag "${name}" deleted\n`);
      } finally {
        client.close();
      }
    });

  tags
    .command('history <name>')
    .description('Show the audit history of a named snapshot tag')
    .action(async (name: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { entries } = await client.snapshotTagHistory(name);
        if (global.json) {
          printJson(entries);
          return;
        }
        printTable(
          ['NAME', 'DIGEST', 'SET AT'],
          entries.map((entry) => [entry.name, entry.digest, isoFromWireSystemTime(entry.set_at)]),
        );
      } finally {
        client.close();
      }
    });
}
