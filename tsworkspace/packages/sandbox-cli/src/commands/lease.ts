import { leaseId, Sandbox, wireSystemTimeToDate, type ProviderKind } from '@trogonai/sandbox-ws-client';
import { Command } from 'commander';

import { connectClient, requireTenant, type GlobalOptions } from '../context.js';
import {
  isoFromWireSystemTime,
  isoFromUnixSeconds,
  printJson,
  printLeaseGranted,
  printTable,
  relativeTimeFromUnixSeconds,
  summarizeImage,
} from '../output.js';
import { parseImageReference, parseKeyValueFlags, validateTagCount } from '../parsing.js';

const PROVIDER_KINDS: ProviderKind[] = [
  'k8s_micro_vm',
  'subprocess',
  'local_docker',
  'daytona',
  'modal',
  'e2b',
  'vercel',
  'space',
  'hyperlight',
  'v8_wasm',
  'kube_virt',
  'azure_container_apps',
  'substrate',
  'firecracker',
  'mock',
];

function assertProviderKind(value: string): asserts value is ProviderKind {
  if (!(PROVIDER_KINDS as string[]).includes(value)) {
    throw new Error(`invalid --provider "${value}": expected one of ${PROVIDER_KINDS.join(', ')}`);
  }
}

interface CreateOptions extends GlobalOptions {
  image: string;
  name?: string;
  tag: string[];
  env: string[];
  timeout?: string;
  networkPolicy?: 'allow-all' | 'deny-all';
  cpuSeconds?: string;
  memoryMib?: string;
  provider: string[];
  /** Commander sets these false for `--no-monitoring` / `--no-snapshots`. */
  monitoring: boolean;
  snapshots: boolean;
  detachJson?: boolean;
}

export function registerCreateCommand(program: Command): void {
  program
    .command('create')
    .description('Create a new sandbox lease')
    .requiredOption('--image <ref>', 'OCI image reference, e.g. docker.io/library/alpine:latest')
    .option('--name <name>', 'sandbox name (persistent identity across forks/restarts)')
    .option('--tag <k=v>', 'sandbox tag (repeatable, max 5)', collectFlag, [])
    .option('--env <k=v>', 'environment variable (repeatable)', collectFlag, [])
    .option('--timeout <ms>', 'lease TTL in milliseconds')
    .option('--network-policy <policy>', 'allow-all or deny-all')
    .option('--cpu-seconds <n>', 'CPU seconds quota')
    .option('--memory-mib <n>', 'memory quota in MiB')
    .option('--provider <kind>', 'preferred provider kind (repeatable)', collectFlag, [])
    .option('--no-monitoring', 'purge the lease command history when it ends')
    .option('--no-snapshots', 'refuse every snapshot, fork or persistence capture of the lease')
    .option('--detach-json', 'print the full LeaseGranted JSON')
    .action(async (opts: CreateOptions, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client, tenant } = await connectClient(global);
      try {
        const tags = parseKeyValueFlags(opts.tag, '--tag');
        validateTagCount(tags);
        const env = parseKeyValueFlags(opts.env, '--env');
        for (const provider of opts.provider) {
          assertProviderKind(provider);
        }

        const reference = parseImageReference(opts.image);
        const resolvedEnv = Object.keys(env).length > 0 ? env : {};

        // Sandbox.create()'s CreateSandboxOptions has no direct cpuSeconds/
        // memoryMib passthrough, only its `spec` escape hatch. We only build
        // a full SandboxSpec (falling back to the same 3600s/512MiB defaults
        // sandbox.ts uses internally) when the caller actually asked for a
        // non-default quota, so the common case still goes through the plain
        // `image`/`env` path.
        const spec =
          opts.cpuSeconds !== undefined || opts.memoryMib !== undefined
            ? {
                image: { kind: 'registry' as const, reference },
                workspace: { empty: {} },
                env: resolvedEnv,
                cpu_seconds: opts.cpuSeconds !== undefined ? Number(opts.cpuSeconds) : 3600,
                memory_mib: opts.memoryMib !== undefined ? Number(opts.memoryMib) : 512,
              }
            : undefined;

        const sandbox = await Sandbox.create(client, {
          tenant: requireTenant(tenant),
          spec,
          image: spec ? undefined : { kind: 'registry', reference },
          name: opts.name,
          tags: Object.keys(tags).length > 0 ? tags : undefined,
          env: spec ? undefined : Object.keys(env).length > 0 ? env : undefined,
          timeoutMs: opts.timeout !== undefined ? Number(opts.timeout) : undefined,
          networkPolicy: opts.networkPolicy,
          providerPref: opts.provider.length > 0 ? (opts.provider as ProviderKind[]) : undefined,
          monitoring: opts.monitoring,
          snapshots: opts.snapshots,
        });

        const granted = {
          lease_id: sandbox.sandboxId,
          provider: sandbox.provider,
          sandbox_id: sandbox.sandboxId,
          evidence_subject: '',
          deadline: { secs_since_epoch: Math.floor(sandbox.deadline.getTime() / 1000), nanos_since_epoch: 0 },
        };

        if (opts.detachJson || global.json) {
          printJson(granted);
        } else {
          printLeaseGranted(granted);
        }
      } finally {
        client.close();
      }
    });
}

function collectFlag(value: string, previous: string[]): string[] {
  return [...previous, value];
}

export function registerListCommand(program: Command): void {
  program
    .command('list')
    .alias('ls')
    .description('List sandbox leases')
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

export function registerForkCommand(program: Command): void {
  program
    .command('fork <lease-id>')
    .description('Fork an existing sandbox lease into a new one')
    .option('--timeout <ms>', 'new lease TTL in milliseconds')
    .action(async (leaseIdArg: string, opts: { timeout?: string }, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const sandbox = await Sandbox.fork(client, {
          sourceSandbox: leaseId(leaseIdArg),
          timeoutMs: opts.timeout !== undefined ? Number(opts.timeout) : undefined,
        });
        const granted = {
          lease_id: sandbox.sandboxId,
          provider: sandbox.provider,
          sandbox_id: sandbox.sandboxId,
          evidence_subject: '',
          deadline: { secs_since_epoch: Math.floor(sandbox.deadline.getTime() / 1000), nanos_since_epoch: 0 },
        };
        if (global.json) {
          printJson(granted);
        } else {
          printLeaseGranted(granted);
        }
      } finally {
        client.close();
      }
    });
}

export function registerStopCommand(program: Command): void {
  program
    .command('stop <lease-id>')
    .description('Stop a sandbox lease')
    .action(async (leaseIdArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await client.stopLease(leaseId(leaseIdArg));
        process.stdout.write(`stopped ${leaseIdArg}\n`);
      } finally {
        client.close();
      }
    });
}

export function registerRemoveCommand(program: Command): void {
  program
    .command('remove <lease-id>')
    .alias('rm')
    .description('Delete a sandbox lease')
    .action(async (leaseIdArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        await client.deleteLease(leaseId(leaseIdArg));
        process.stdout.write(`removed ${leaseIdArg}\n`);
      } finally {
        client.close();
      }
    });
}

export function registerExtendCommand(program: Command): void {
  program
    .command('extend <lease-id>')
    .description('Extend a sandbox lease timeout')
    .requiredOption('--by <ms>', 'additional time in milliseconds')
    .action(async (leaseIdArg: string, opts: { by: string }, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const additionalSeconds = Math.ceil(Number(opts.by) / 1000);
        const response = await client.extendLeaseTimeout(leaseId(leaseIdArg), additionalSeconds);
        const deadline = wireSystemTimeToDate(response.new_deadline);
        process.stdout.write(`new deadline: ${deadline.toISOString()}\n`);
      } finally {
        client.close();
      }
    });
}

export function registerDomainCommand(program: Command): void {
  program
    .command('domain <lease-id> <port>')
    .description('Resolve a reachable host:port endpoint for a port exposed by a sandbox lease')
    .action(async (leaseIdArg: string, portArg: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const port = Number(portArg);
        const response = await client.getSandboxDomain(leaseId(leaseIdArg), port);
        if (response.outcome === 'available') {
          if (global.json) {
            printJson(response);
          } else {
            process.stdout.write(`${response.endpoint}\n`);
          }
          return;
        }
        if (response.outcome === 'port_not_exposed') {
          throw new Error(`port ${port} is not exposed on sandbox ${leaseIdArg}`);
        }
        if (response.outcome === 'unsupported') {
          throw new Error(`sandbox ${leaseIdArg}'s provider does not support sandbox domain resolution`);
        }
        throw new Error(`domain resolution for port ${port} denied on sandbox ${leaseIdArg}`);
      } finally {
        client.close();
      }
    });
}

export function registerGetCommand(program: Command): void {
  program
    .command('get <name>')
    .description('Look up a sandbox lease by its tenant-scoped --name')
    .action(async (name: string, _opts: unknown, command: Command) => {
      const global = command.optsWithGlobals() as GlobalOptions;
      const { client } = await connectClient(global);
      try {
        const response = await client.getLeaseByName(name);
        if (response.outcome === 'not_found') {
          throw new Error(`sandbox with name "${name}" not found`);
        }
        const row = response.row;
        if (global.json) {
          printJson(row);
          return;
        }
        process.stdout.write(`lease: ${row.lease_id}\n`);
        process.stdout.write(`tenant: ${row.tenant}\n`);
        process.stdout.write(`provider: ${row.provider}\n`);
        process.stdout.write(`status: ${row.status}\n`);
        process.stdout.write(`created_at: ${isoFromWireSystemTime(row.created_at)}\n`);
        process.stdout.write(`deadline: ${isoFromWireSystemTime(row.deadline)}\n`);
        if (row.name) {
          process.stdout.write(`name: ${row.name}\n`);
        }
      } finally {
        client.close();
      }
    });
}
