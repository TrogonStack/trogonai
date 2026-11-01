import type { KeyOperationAuditRecord } from '@trogonai/proto';

import type { KeyBackendId, KeyRef } from '../domain/identifiers';
import { keyBackendIdOf } from '../domain/identifiers';
import type { BackendRecord, TenantCustodyState } from '../domain/state';

export interface InternalCustodyState extends Omit<TenantCustodyState, 'backends' | 'operations'> {
  readonly backends: readonly BackendRecord[];
  readonly operations: readonly KeyOperationAuditRecord[];
  readonly placements: ReadonlyMap<KeyRef, KeyBackendId>;
}

export function isPlatformOwned(record: BackendRecord): boolean {
  return record.registration.configuration?.owner?.owner.case === 'platform';
}

export function projectForTenant(state: InternalCustodyState): TenantCustodyState {
  const backends = state.backends.filter((record) => !isPlatformOwned(record));
  const visible = new Set(backends.map((record) => keyBackendIdOf(record.registration.keyBackendId)));
  return {
    backends,
    purposes: state.purposes,
    custody: state.custody,
    keys: state.keys,
    payloads: state.payloads,
    migrations: state.migrations,
    operations: state.operations.filter((record) => visible.has(keyBackendIdOf(record.keyBackendId))),
  };
}
