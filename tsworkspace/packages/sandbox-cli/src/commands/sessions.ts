import { Command } from 'commander';

import { connectClient, type GlobalOptions } from '../context.js';
import { isoFromUnixSeconds, printJson, printTable, relativeTimeFromUnixSeconds, summarizeImage } from '../output.js';

export function registerSessionsCommand(program: Command): void {
  const sessions = program.command('sessions').description('Inspect sandbox sessions');

  sessions
    .command('list')
    .description(
      'List sandbox sessions. A "session" maps 1:1 onto a lease in this backend: this is a straight alias ' +
        'over listLeases(), identical to `sandbox list`.',
    )
    .action(async (_opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const { leases } = await client.listLeases();
        if (global.json) {
          printJson(leases);
          return;
        }
        printTable(
          ['LEASE ID', 'PROVIDER', 'IMAGE', 'STATE', 'STARTED', 'TTL(s)'],
          leases.map((lease) => [
            lease.lease_id,
            lease.provider,
            summarizeImage(lease.image),
            lease.state,
            `${relativeTimeFromUnixSeconds(lease.started_at)} (${isoFromUnixSeconds(lease.started_at)})`,
            String(lease.ttl_seconds),
          ]),
        );
      } finally {
        client.close();
      }
    });
}
