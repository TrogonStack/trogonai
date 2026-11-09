import type { CustodyTier } from './custody';
import type { KeyRef, MigrationId, PurposeId } from './identifiers';

export type MigrationStatus = 'running' | 'paused' | 'completed';

export type CustodyEndpoint =
  | { readonly tier: 'managed' }
  | { readonly tier: 'dedicated'; readonly keyRef: KeyRef }
  | { readonly tier: 'customer_managed'; readonly keyRef: KeyRef };

export type CustodyTarget =
  | { readonly tier: 'managed' }
  | { readonly tier: 'dedicated' }
  | { readonly tier: 'customer_managed'; readonly keyRef: KeyRef };

export interface Migration {
  readonly id: MigrationId;
  readonly operationId: string;
  readonly purpose: PurposeId;
  readonly from: CustodyEndpoint;
  readonly to: CustodyEndpoint;
  readonly status: MigrationStatus;
  readonly total: number;
  readonly rewrapped: number;
  readonly ratePerMinute: number;
  readonly startedAt: Date;
  readonly updatedAt: Date;
  readonly completedAt?: Date;
}

export const migrationStatusLabels: Record<MigrationStatus, string> = {
  running: 'Running',
  paused: 'Paused',
  completed: 'Completed',
};

export function endpointTier(endpoint: CustodyEndpoint): CustodyTier {
  return endpoint.tier;
}

export function endpointKeyRef(endpoint: CustodyEndpoint): KeyRef | undefined {
  return endpoint.tier === 'managed' ? undefined : endpoint.keyRef;
}

export function sameEndpoint(a: CustodyEndpoint, b: CustodyEndpoint): boolean {
  return a.tier === b.tier && endpointKeyRef(a) === endpointKeyRef(b);
}

const CONFLICT_INTERVAL = 9;

export function conflictsRetried(migration: Migration): number {
  return Math.floor(migration.rewrapped / CONFLICT_INTERVAL);
}

export function remaining(migration: Migration): number {
  return migration.total - migration.rewrapped;
}

export function isInFlight(migration: Migration): boolean {
  return migration.status !== 'completed';
}

export type MigrationDirection =
  | 'into_customer_managed'
  | 'out_of_customer_managed'
  | 'between_customer_managed'
  | 'within_platform';

export const migrationDirectionLabels: Record<MigrationDirection, string> = {
  into_customer_managed: 'Into customer managed custody',
  out_of_customer_managed: 'Out of customer managed custody',
  between_customer_managed: 'Between customer managed keys',
  within_platform: 'Within platform custody',
};

export function migrationDirection(migration: Migration): MigrationDirection {
  const from = migration.from.tier === 'customer_managed';
  const to = migration.to.tier === 'customer_managed';
  if (from && to) return 'between_customer_managed';
  if (to) return 'into_customer_managed';
  if (from) return 'out_of_customer_managed';
  return 'within_platform';
}

export function advanceMigration(migration: Migration, now: Date): Migration {
  if (migration.status !== 'running') return migration;
  const elapsedMinutes = (now.getTime() - migration.updatedAt.getTime()) / 60_000;
  const advanced = Math.floor(elapsedMinutes * migration.ratePerMinute);
  if (advanced <= 0) return migration;
  const rewrapped = Math.min(migration.total, migration.rewrapped + advanced);
  if (rewrapped >= migration.total) {
    return { ...migration, rewrapped: migration.total, status: 'completed', updatedAt: now, completedAt: now };
  }
  const consumedMs = (advanced / migration.ratePerMinute) * 60_000;
  return { ...migration, rewrapped, updatedAt: new Date(migration.updatedAt.getTime() + consumedMs) };
}
