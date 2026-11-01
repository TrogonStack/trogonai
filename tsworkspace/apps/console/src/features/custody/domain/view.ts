import { timestampDate } from '@bufbuild/protobuf/wkt';
import {
  KeyBackendAdmission,
  KeyBackendLifecycle,
  KeyProvider,
  KeyState,
  KeyUseDenialReason,
  type KeyBackendRegistration,
  type KeyCapability,
} from '@trogonai/proto';

import { canPause, canResume, keyUseDenial } from './admission';
import {
  accessSummary,
  connectionStatusOf,
  expiryWarnings,
  providerOfAccess,
  type ConnectionStatus,
  type ExpiryWarning,
  type RevisionNote,
} from './backend';
import { keyRefOfCustody, purposeOfCustody, tierOfCustody, type CustodyTier } from './custody';
import { denialReasonMessages, keyFaultMessages } from './faults';
import { keyBackendIdOf, type KeyBackendId, type KeyRef, type PurposeId } from './identifiers';
import type { DailyUsage, TenantKey } from './key';
import { canBeDefault } from './key-state';
import { locatorText } from './locator';
import {
  endpointKeyRef,
  isInFlight,
  migrationDirection,
  remaining,
  type CustodyEndpoint,
  type Migration,
  type MigrationDirection,
} from './migration';
import { purposeLabel, type PurposeDefinition } from './purpose';
import type { BackendRecord, PayloadSet, TenantCustodyState } from './state';
import { toUsageRecordView, type UsageRecordView } from './usage';

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

const yes: ActionAvailability = { available: true };
const no = (reason: string): ActionAvailability => ({ available: false, reason });

export interface BackendReference {
  readonly id: KeyBackendId;
  readonly displayName: string;
  readonly provider: KeyProvider;
  readonly admission: KeyBackendAdmission;
}

export interface CustomerBindingView {
  readonly backend: BackendReference;
  readonly locator: string;
}

export interface KeyView {
  readonly keyRef: KeyRef;
  readonly purpose: PurposeId;
  readonly purposeLabel: string;
  readonly tier: Exclude<CustodyTier, 'managed'>;
  readonly state: KeyState;
  readonly observedAt: Date;
  readonly capabilities: readonly KeyCapability[];
  readonly boundAt: Date;
  readonly customer: CustomerBindingView | undefined;
  readonly isDefault: boolean;
  readonly payloads: readonly PayloadSet[];
  readonly payloadCount: number;
  readonly inbound: number;
  readonly outbound: number;
  readonly usage: readonly DailyUsage[];
  readonly wraps7d: number;
  readonly unwraps7d: number;
  readonly records: readonly UsageRecordView[];
  readonly wrapDenial: KeyUseDenialReason | undefined;
  readonly unwrapDenial: KeyUseDenialReason | undefined;
  readonly blockedByPause: boolean;
  readonly makeDefault: ActionAvailability;
}

export interface BackendView {
  readonly id: KeyBackendId;
  readonly registration: KeyBackendRegistration;
  readonly displayName: string;
  readonly provider: KeyProvider;
  readonly revision: number;
  readonly lifecycle: KeyBackendLifecycle;
  readonly admission: KeyBackendAdmission;
  readonly paused: boolean;
  readonly connection: ConnectionStatus;
  readonly expiry: readonly ExpiryWarning[];
  readonly profileSummary: string;
  readonly registeredAt: Date;
  readonly revisions: readonly RevisionNote[];
  readonly keys: readonly KeyView[];
  readonly pause: ActionAvailability;
  readonly resume: ActionAvailability;
  readonly validate: ActionAvailability;
}

export interface MigrationEnd {
  readonly tier: CustodyTier;
  readonly key: KeyView | undefined;
}

export interface MigrationView {
  readonly migration: Migration;
  readonly purpose: PurposeId;
  readonly purposeLabel: string;
  readonly direction: MigrationDirection;
  readonly from: MigrationEnd;
  readonly to: MigrationEnd;
  readonly remaining: number;
  readonly percent: number;
  readonly inFlight: boolean;
  readonly blocked: string | undefined;
}

export interface PurposeCustodyView {
  readonly purpose: PurposeDefinition;
  readonly tier: CustodyTier;
  readonly key: KeyView | undefined;
  readonly payloadCount: number;
  readonly migration: MigrationView | undefined;
  readonly denial: KeyUseDenialReason | undefined;
}

export interface CustodyView {
  readonly catalogue: readonly PurposeDefinition[];
  readonly backends: readonly BackendView[];
  readonly keys: readonly KeyView[];
  readonly migrations: readonly MigrationView[];
  readonly purposes: readonly PurposeCustodyView[];
  readonly records: readonly UsageRecordView[];
  readonly stats: {
    readonly backends: number;
    readonly pausedBackends: number;
    readonly keys: number;
    readonly deniedKeys: number;
    readonly migrationsInFlight: number;
  };
}

function sum(values: readonly number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

function referenceOf(record: BackendRecord): BackendReference {
  return {
    id: keyBackendIdOf(record.registration.keyBackendId),
    displayName: record.displayName,
    provider: providerOfAccess(record.registration.configuration?.access),
    admission: record.registration.admission,
  };
}

function toKeyView(
  state: TenantCustodyState,
  catalogue: readonly PurposeDefinition[],
  key: TenantKey,
  defaults: ReadonlyMap<PurposeId, KeyRef | undefined>,
): KeyView {
  const owner = key.binding.kind === 'customer_managed' ? keyBackendIdOf(key.binding.detail.keyBackendId) : undefined;
  const backend = owner
    ? state.backends.find((record) => keyBackendIdOf(record.registration.keyBackendId) === owner)
    : undefined;
  const reference = backend ? referenceOf(backend) : undefined;
  const admission = reference?.admission;
  const wrapDenial = keyUseDenial(admission, key.state.state, 'wrap');
  const unwrapDenial = keyUseDenial(admission, key.state.state, 'unwrap');
  const payloads = state.payloads.filter((set) => set.keyRef === key.keyRef);
  const flights = state.migrations.filter(isInFlight);
  const outbound = sum(flights.filter((m) => endpointKeyRef(m.from) === key.keyRef).map((m) => m.rewrapped));
  const inbound = sum(flights.filter((m) => endpointKeyRef(m.to) === key.keyRef).map((m) => m.rewrapped));
  const payloadCount = Math.max(0, sum(payloads.map((set) => set.count)) - outbound) + inbound;
  const isDefault = defaults.get(key.purpose) === key.keyRef;
  const migrationInFlight = flights.some((migration) => migration.purpose === key.purpose);
  const makeDefault: ActionAvailability = migrationInFlight
    ? no(keyFaultMessages.MIGRATION_IN_FLIGHT)
    : isDefault
      ? no('This key is already the default for its purpose.')
      : wrapDenial === KeyUseDenialReason.BACKEND_PAUSED
        ? no(denialReasonMessages[KeyUseDenialReason.BACKEND_PAUSED])
        : !canBeDefault(key.state.state)
          ? no('Only an active key can become the default, because the default wraps new payloads.')
          : yes;
  const usage = key.usage;
  return {
    keyRef: key.keyRef,
    purpose: key.purpose,
    purposeLabel: purposeLabel(catalogue, key.purpose),
    tier: key.binding.kind,
    state: key.state.state,
    observedAt: key.state.observedAt ? timestampDate(key.state.observedAt) : key.boundAt,
    capabilities: key.capabilities,
    boundAt: key.boundAt,
    customer:
      key.binding.kind === 'customer_managed' && reference
        ? { backend: reference, locator: locatorText(key.binding.detail.locator) }
        : undefined,
    isDefault,
    payloads,
    payloadCount,
    inbound,
    outbound,
    usage,
    wraps7d: sum(usage.map((day) => day.wraps)),
    unwraps7d: sum(usage.map((day) => day.unwraps)),
    records: state.operations
      .filter((record) => record.keyRef?.value === key.keyRef)
      .map(toUsageRecordView)
      .sort((a, b) => b.occurredAt.getTime() - a.occurredAt.getTime()),
    wrapDenial,
    unwrapDenial,
    blockedByPause: unwrapDenial === KeyUseDenialReason.BACKEND_PAUSED,
    makeDefault,
  };
}

function toBackendView(record: BackendRecord, keys: readonly KeyView[], now: Date): BackendView {
  const registration = record.registration;
  const id = keyBackendIdOf(registration.keyBackendId);
  const { lifecycle, admission } = registration;
  const paused = admission === KeyBackendAdmission.PAUSED;
  return {
    id,
    registration,
    displayName: record.displayName,
    provider: providerOfAccess(registration.configuration?.access),
    revision: Number(registration.revision?.value ?? 1),
    lifecycle,
    admission,
    paused,
    connection: connectionStatusOf(registration.health),
    expiry: expiryWarnings(registration.health?.trustExpiry, now),
    profileSummary: accessSummary(registration.configuration?.access),
    registeredAt: record.registeredAt,
    revisions: record.revisions,
    keys: keys.filter((key) => key.customer?.backend.id === id),
    pause: canPause(lifecycle, admission)
      ? yes
      : no(paused ? 'The registration is already paused.' : 'Only an active registration can be paused.'),
    resume: canResume(lifecycle, admission) ? yes : no('Only a paused registration can be resumed.'),
    validate: paused
      ? no('The platform makes no call to your provider while the registration is paused. Resume it to validate.')
      : lifecycle === KeyBackendLifecycle.RETIRED
        ? no('The registration is retired.')
        : yes,
  };
}

function endOf(endpoint: CustodyEndpoint, keys: ReadonlyMap<KeyRef, KeyView>): MigrationEnd {
  const ref = endpointKeyRef(endpoint);
  return { tier: endpoint.tier, key: ref ? keys.get(ref) : undefined };
}

function toMigrationView(
  migration: Migration,
  keys: ReadonlyMap<KeyRef, KeyView>,
  catalogue: readonly PurposeDefinition[],
): MigrationView {
  const from = endOf(migration.from, keys);
  const to = endOf(migration.to, keys);
  const inFlight = isInFlight(migration);
  const sourceDenial = from.key?.unwrapDenial;
  const targetDenial = to.key?.wrapDenial;
  const blocked = !inFlight
    ? undefined
    : sourceDenial !== undefined
      ? `The source cannot be read. ${denialReasonMessages[sourceDenial]}`
      : targetDenial !== undefined
        ? `The target cannot be written. ${denialReasonMessages[targetDenial]}`
        : undefined;
  return {
    migration,
    purpose: migration.purpose,
    purposeLabel: purposeLabel(catalogue, migration.purpose),
    direction: migrationDirection(migration),
    from,
    to,
    remaining: remaining(migration),
    percent: migration.total === 0 ? 100 : Math.round((migration.rewrapped / migration.total) * 100),
    inFlight,
    blocked,
  };
}

export function toCustodyView(state: TenantCustodyState, now: Date = new Date()): CustodyView {
  const defaults = new Map<PurposeId, KeyRef | undefined>(
    state.custody.map((custody) => [purposeOfCustody(custody), keyRefOfCustody(custody)] as const),
  );
  const keys = state.keys.map((key) => toKeyView(state, state.purposes, key, defaults));
  const byRef = new Map(keys.map((key) => [key.keyRef, key] as const));
  const migrations = [...state.migrations]
    .sort((a, b) => b.startedAt.getTime() - a.startedAt.getTime())
    .map((migration) => toMigrationView(migration, byRef, state.purposes));
  const backends = state.backends.map((record) => toBackendView(record, keys, now));

  const purposes = state.purposes.map((purpose): PurposeCustodyView => {
    const custody = state.custody.find((candidate) => purposeOfCustody(candidate) === purpose.id);
    const ref = custody ? keyRefOfCustody(custody) : undefined;
    const key = ref ? byRef.get(ref) : undefined;
    return {
      purpose,
      tier: custody ? tierOfCustody(custody) : 'managed',
      key,
      payloadCount: sum(state.payloads.filter((set) => set.purpose === purpose.id).map((set) => set.count)),
      migration: migrations.find((view) => view.purpose === purpose.id && view.inFlight),
      denial: key?.unwrapDenial,
    };
  });

  return {
    catalogue: state.purposes,
    backends,
    keys,
    migrations,
    purposes,
    records: state.operations.map(toUsageRecordView).sort((a, b) => b.occurredAt.getTime() - a.occurredAt.getTime()),
    stats: {
      backends: backends.length,
      pausedBackends: backends.filter((backend) => backend.paused).length,
      keys: keys.length,
      deniedKeys: keys.filter((key) => key.unwrapDenial !== undefined).length,
      migrationsInFlight: migrations.filter((view) => view.inFlight).length,
    },
  };
}
