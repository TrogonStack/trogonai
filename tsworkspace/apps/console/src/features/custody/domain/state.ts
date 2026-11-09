import type { KeyBackendRegistration, KeyOperationAuditRecord, PurposeCustody } from '@trogonai/proto';

import type { RevisionNote } from './backend';
import type { KeyRef, PurposeId } from './identifiers';
import type { TenantKey } from './key';
import type { Migration } from './migration';
import type { PurposeDefinition } from './purpose';

export interface BackendRecord {
  readonly registration: KeyBackendRegistration;
  readonly displayName: string;
  readonly registeredAt: Date;
  readonly revisions: readonly RevisionNote[];
}

export interface PayloadSet {
  readonly id: string;
  readonly purpose: PurposeId;
  readonly label: string;
  readonly keyRef: KeyRef | undefined;
  readonly count: number;
}

export interface TenantCustodyState {
  readonly backends: readonly BackendRecord[];
  readonly purposes: readonly PurposeDefinition[];
  readonly custody: readonly PurposeCustody[];
  readonly keys: readonly TenantKey[];
  readonly payloads: readonly PayloadSet[];
  readonly migrations: readonly Migration[];
  readonly operations: readonly KeyOperationAuditRecord[];
}
