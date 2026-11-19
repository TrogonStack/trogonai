import { z } from 'zod';

import { SandboxWsError } from './errors.ts';
import type {
  LeaseSnapshotReceipt,
  ListProcessesResponse,
  NativeSnapshotReceipt,
  NativeSnapshotRef,
  OciArtifactRef,
} from './wire.ts';

const environmentKey = z
  .string()
  .regex(/^[A-Za-z_][A-Za-z0-9_]*$/)
  .max(128);
const environmentKeys = z
  .array(environmentKey)
  .max(128)
  .refine((keys) => new Set(keys).size === keys.length);

const serverParametersSchema = z.strictObject({
  cmd: z.string().min(1).max(4096),
  args: z.array(z.string().max(8192)).max(128).optional(),
  cwd: z.string().min(1).max(4096).optional(),
  user: z.string().min(1).max(128).optional(),
  nonInteractive: z.boolean().optional(),
  outputRetention: z.enum(['buffer', 'stream_only']).optional(),
  maxOutputBytes: z.number().int().min(0).max(0xffff_ffff).optional(),
  port: z.number().int().min(1).max(65535),
  path: z
    .string()
    .max(2048)
    .regex(/^\/\S*$/)
    .optional(),
  name: z
    .string()
    .regex(/^[A-Za-z0-9._-]{1,64}$/)
    .optional(),
  preview: z.boolean().optional(),
  timeoutMs: z.number().min(0).max(2_147_483_647),
});

const serverEntrySchema = z
  .strictObject({
    id: z.string().min(1).max(128),
    executionId: z.string().min(1).max(256),
    params: serverParametersSchema,
    environmentKeys,
    overrideEnvironmentKeys: environmentKeys,
    redactedEnvironmentKeys: environmentKeys,
  })
  .refine((entry) =>
    [...entry.overrideEnvironmentKeys, ...entry.redactedEnvironmentKeys].every((key) =>
      entry.environmentKeys.includes(key),
    ),
  );

export const serverClientStateSchema = z.strictObject({
  version: z.literal(1),
  sourceSandboxId: z.string().min(1).max(256),
  servers: z
    .array(serverEntrySchema)
    .max(32)
    .refine((servers) => new Set(servers.map(({ id }) => id)).size === servers.length),
});

export type ServerRestartEntry = z.output<typeof serverEntrySchema>;
export type ServerClientState = z.output<typeof serverClientStateSchema>;
export type SandboxSnapshotReceipt = LeaseSnapshotReceipt & { clientState?: ServerClientState };
export type SandboxNativeSnapshotReceipt = NativeSnapshotReceipt & { clientState?: ServerClientState };
export type RestorableSnapshot = (LeaseSnapshotReceipt | NativeSnapshotReceipt | OciArtifactRef | NativeSnapshotRef) & {
  clientState?: ServerClientState;
};

export type SnapshotExecutionContinuity = 'disk_only' | 'memory_preserving' | 'unknown';

export function snapshotExecutionContinuity(snapshot: RestorableSnapshot): SnapshotExecutionContinuity {
  const reference = 'reference' in snapshot ? snapshot.reference : snapshot;
  if ('provider' in reference) {
    if (reference.provider === 'vercel' || reference.provider === 'modal' || reference.provider === 'fly')
      return 'disk_only';
    return reference.provider === 'e2b' ? 'memory_preserving' : 'unknown';
  }
  if (reference.artifact_type === 'sandbox_rootfs_v1' || reference.artifact_type === 'kubevirt_disk_v1')
    return 'disk_only';
  return reference.artifact_type === 'firecracker_snapshot_v1' ? 'memory_preserving' : 'unknown';
}

const processIdentity = {
  execution_id: z.string().min(1).max(256),
  name: z
    .string()
    .regex(/^[A-Za-z0-9._-]{1,64}$/)
    .optional(),
  readiness: z
    .discriminatedUnion('kind', [
      z.object({ kind: z.literal('tcp_port'), port: z.number().int().min(1).max(65535) }),
      z.object({
        kind: z.literal('output_contains'),
        needle: z
          .string()
          .min(1)
          .max(4096)
          .refine((value) => new TextEncoder().encode(value).byteLength <= 4096),
      }),
    ])
    .optional(),
};

const processInventorySchema = z.discriminatedUnion('outcome', [
  z.object({ outcome: z.literal('unsupported') }),
  z
    .object({
      outcome: z.literal('processes'),
      processes: z
        .array(
          z.discriminatedUnion('state', [
            z.object({ ...processIdentity, state: z.literal('running') }),
            z.object({
              ...processIdentity,
              state: z.literal('exited'),
              exit_code: z.number().int().min(-0x8000_0000).max(0x7fff_ffff),
            }),
          ]),
        )
        .max(20_000),
    })
    .refine(({ processes }) => {
      const runningNames = processes
        .filter((process) => process.state === 'running' && process.name !== undefined)
        .map(({ name }) => name);
      return (
        new Set(processes.map(({ execution_id }) => execution_id)).size === processes.length &&
        new Set(runningNames).size === runningNames.length
      );
    }),
]);

export function parseServerProcessInventory(value: unknown): ListProcessesResponse {
  const parsed = processInventorySchema.safeParse(value);
  if (!parsed.success)
    throw new SandboxWsError('invalid_response', 'server process discovery returned an invalid or ambiguous inventory');
  return parsed.data;
}

export interface ServerConnectionOptions {
  clientState?: ServerClientState;
  /** Replacement values stay in memory and are never included in exported state. */
  env?: Record<string, string>;
}

export function parseServerClientState(value: unknown): ServerClientState {
  const parsed = serverClientStateSchema.safeParse(value);
  if (!parsed.success || new TextEncoder().encode(JSON.stringify(parsed.data)).byteLength > 64 * 1024) {
    throw new SandboxWsError('invalid_request', 'invalid server restart state: expected a bounded version 1 plan');
  }
  return parsed.data;
}

export function snapshotClientState(snapshot: RestorableSnapshot | undefined): ServerClientState | undefined {
  return snapshot?.clientState === undefined ? undefined : parseServerClientState(snapshot.clientState);
}

export function requireServerEnvironment(
  state: ServerClientState | undefined,
  env: Record<string, string>,
  availableNames: ReadonlySet<string>,
  sameSandbox = false,
): void {
  if (state === undefined) return;
  for (const entry of state.servers) {
    const missing =
      entry.environmentKeys.some((key) => !availableNames.has(key) && !sameSandbox) ||
      entry.overrideEnvironmentKeys.some((key) => !Object.hasOwn(env, key)) ||
      entry.redactedEnvironmentKeys.some((key) => !Object.hasOwn(env, key));
    if (missing) throw new SandboxWsError('invalid_request', 'server restoration needs replacement environment values');
  }
}
