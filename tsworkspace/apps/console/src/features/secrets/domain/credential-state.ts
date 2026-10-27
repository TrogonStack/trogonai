import { timestampDate } from '@bufbuild/protobuf/wkt';
import {
  CredentialLifecycle,
  SecretVersionState,
  type CredentialEvent,
  type CredentialWriteKind,
  type CredentialPolicyReason,
  type MaterialVerificationOutcome,
  type SecretAuthorizationEndReason,
  type SecretFailureReason,
  type SecretMaterialKind,
  type SecretMaterialOrigin,
  type SecretPurpose,
  type SecretUseAction,
  type SecretUseDirection,
  type SecretUseMode,
  type SecretUseResult,
  type SecretLeaseEndReason,
} from '@trogonai/proto';

import { environmentIdFromStored, type EnvironmentId } from '@/features/environments/domain/environment-id';

import type { LeaseRefId, SecretRefId } from './secret-ref';

export type WriteStepKey =
  | 'RESERVED'
  | 'CANDIDATE_WRITE_RESERVED'
  | 'CANDIDATE_OBSERVED'
  | 'VERIFICATION'
  | 'ACTIVATED';

export type WriteStatus = 'IN_FLIGHT' | 'ACTIVATED' | 'REJECTED';

export interface EventPointer {
  readonly revision: number;
  readonly eventId: string;
}

export interface VersionEntry {
  readonly version: number;
  readonly state: SecretVersionState;
  readonly observedAt: Date;
  readonly activated: boolean;
  readonly activatedAt?: Date;
  readonly retiredAt?: Date;
  readonly destroyedAt?: Date;
}

export interface OverlapWindow {
  readonly version: number;
  readonly startsAt: Date;
  readonly expiresAt: Date;
}

export interface WriteSaga {
  readonly provisioningId: string;
  readonly kind: CredentialWriteKind;
  readonly stepId: string;
  readonly ownershipToken: string;
  readonly origin: SecretMaterialOrigin;
  readonly status: WriteStatus;
  readonly candidateVersion: number;
  readonly priorActiveVersion?: number;
  readonly verificationRequired: boolean;
  readonly requirementId?: string;
  readonly previousWebhookExpiresAt?: Date;
  readonly writeReserved: EventPointer;
  readonly candidateWriteReservedAt: Date;
  readonly candidateObservedAt?: Date;
  readonly verification?: {
    readonly outcome: MaterialVerificationOutcome;
    readonly at: Date;
    readonly evidenceId: string;
    readonly pointer: EventPointer;
  };
  readonly activatedAt?: Date;
  readonly rejection?: {
    readonly reason: SecretFailureReason;
    readonly at: Date;
    readonly failedStep: WriteStepKey;
  };
}

export interface UseRecord {
  readonly id: string;
  readonly workloadPrincipalId: string;
  readonly operationId: string;
  readonly purpose: SecretPurpose;
  readonly action: SecretUseAction;
  readonly direction: SecretUseDirection;
  readonly mode: SecretUseMode;
  readonly audience: string;
  readonly bindingId: string;
  readonly bindingRevision: number;
  readonly issuedAt: Date;
  readonly expiresAt: Date;
  readonly version?: number;
  readonly result?: SecretUseResult;
  readonly failureReason?: SecretFailureReason;
  readonly completedAt?: Date;
  readonly endedReason?: SecretAuthorizationEndReason;
  readonly endedAt?: Date;
}

export interface LeaseRecord {
  readonly id: LeaseRefId;
  readonly workloadPrincipalId: string;
  readonly operationId: string;
  readonly audience: string;
  readonly bindingId: string;
  readonly bindingRevision: number;
  readonly reservedAt: Date;
  readonly issuedAt?: Date;
  readonly expiresAt?: Date;
  readonly providerExpiresAt?: Date;
  readonly endedAt?: Date;
  readonly endReason?: SecretLeaseEndReason;
  readonly cleanupConfirmedAt?: Date;
  readonly replaces?: string;
}

export interface CredentialState {
  readonly secret: SecretRefId;
  readonly projectId: string;
  readonly vaultId: string;
  readonly displayName: string;
  readonly purpose: SecretPurpose;
  readonly kind: SecretMaterialKind;
  readonly environment: EnvironmentId;
  readonly origin?: SecretMaterialOrigin;
  readonly lifecycle: CredentialLifecycle;
  readonly lifecycleReason?: CredentialPolicyReason;
  readonly lifecycleChangedAt: Date;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly revision: number;
  readonly activeVersion?: number;
  readonly lastRotatedAt?: Date;
  readonly versions: readonly VersionEntry[];
  readonly overlap?: OverlapWindow;
  readonly write?: WriteSaga;
  readonly destruction?: {
    readonly executeAfter: Date;
    readonly reason: CredentialPolicyReason;
    readonly scheduledAt: Date;
  };
  readonly destroyedAt?: Date;
  readonly uses: readonly UseRecord[];
  readonly leases: readonly LeaseRecord[];
}

export function nextVersion(state: CredentialState): number {
  return state.versions.reduce((highest, entry) => Math.max(highest, entry.version), 0) + 1;
}

export function isLive(state: CredentialState): boolean {
  return (
    state.lifecycle !== CredentialLifecycle.DESTROYED && state.lifecycle !== CredentialLifecycle.PROVISIONING_ABORTED
  );
}

export function hasOutstandingWrite(state: CredentialState): boolean {
  return state.write?.status === 'IN_FLIGHT';
}

export function liveLeases(state: CredentialState): readonly LeaseRecord[] {
  return state.leases.filter((lease) => lease.issuedAt !== undefined && lease.endedAt === undefined);
}

function dateOf(event: CredentialEvent): Date {
  return event.occurredAt ? timestampDate(event.occurredAt) : new Date(0);
}

function mapVersion(
  versions: readonly VersionEntry[],
  version: number,
  update: (entry: VersionEntry) => VersionEntry,
): readonly VersionEntry[] {
  return versions.map((entry) => (entry.version === version ? update(entry) : entry));
}

function failedStep(saga: WriteSaga): WriteStepKey {
  if (saga.candidateObservedAt) return saga.verificationRequired ? 'VERIFICATION' : 'ACTIVATED';
  return 'CANDIDATE_OBSERVED';
}

function updateUse(
  uses: readonly UseRecord[],
  id: string,
  update: (use: UseRecord) => UseRecord,
): readonly UseRecord[] {
  return uses.map((use) => (use.id === id ? update(use) : use));
}

function updateLease(
  leases: readonly LeaseRecord[],
  id: string,
  update: (lease: LeaseRecord) => LeaseRecord,
): readonly LeaseRecord[] {
  return leases.map((lease) => (lease.id === id ? update(lease) : lease));
}

export function evolveCredential(
  state: CredentialState | undefined,
  event: CredentialEvent,
): CredentialState | undefined {
  const at = dateOf(event);
  const revision = Number(event.revision);
  const fact = event.fact;

  if (fact.case === 'reserved') {
    const policy = fact.value.policy!;
    return {
      secret: event.secret!.id as SecretRefId,
      projectId: event.projectId,
      vaultId: fact.value.vaultId,
      displayName: fact.value.displayName,
      purpose: policy.purpose,
      kind: policy.materialKind,
      environment: environmentIdFromStored(policy.environment),
      lifecycle: CredentialLifecycle.PROVISIONING,
      lifecycleChangedAt: at,
      createdAt: at,
      updatedAt: at,
      revision,
      versions: [],
      uses: [],
      leases: [],
    };
  }
  if (!state) return state;

  const base: CredentialState = { ...state, updatedAt: at, revision };

  switch (fact.case) {
    case 'metadataUpdated':
      return { ...base, displayName: fact.value.displayName };
    case 'candidateWriteReserved': {
      const required = fact.value.requiredVerifications;
      return {
        ...base,
        write: {
          provisioningId: fact.value.provisioning!.id,
          kind: fact.value.kind,
          stepId: fact.value.stepId,
          ownershipToken: fact.value.ownershipToken,
          origin: fact.value.origin,
          status: 'IN_FLIGHT',
          candidateVersion: nextVersion(state),
          priorActiveVersion: fact.value.priorActiveVersion ? Number(fact.value.priorActiveVersion.version) : undefined,
          verificationRequired: required.length > 0,
          requirementId: required[0]?.id,
          previousWebhookExpiresAt: fact.value.previousWebhookExpiresAt
            ? timestampDate(fact.value.previousWebhookExpiresAt)
            : undefined,
          writeReserved: { revision, eventId: event.eventId },
          candidateWriteReservedAt: at,
        },
      };
    }
    case 'candidateObserved': {
      const candidate = fact.value.candidate!;
      const version = Number(candidate.reference!.version);
      return {
        ...base,
        versions: [...state.versions, { version, state: candidate.state, observedAt: at, activated: false }],
        write: state.write && { ...state.write, candidateVersion: version, candidateObservedAt: at },
      };
    }
    case 'materialVerificationRecorded': {
      const verification = fact.value.verification!;
      return {
        ...base,
        write: state.write && {
          ...state.write,
          verification: {
            outcome: verification.outcome,
            at,
            evidenceId: verification.evidenceId,
            pointer: { revision, eventId: event.eventId },
          },
        },
      };
    }
    case 'versionActivated': {
      const activated = Number(fact.value.version!.version);
      const retired = fact.value.retiredVersion ? Number(fact.value.retiredVersion.version) : undefined;
      const previous = fact.value.previousWebhookVersion;
      let versions = mapVersion(state.versions, activated, (entry) => ({
        ...entry,
        state: SecretVersionState.AVAILABLE,
        activated: true,
        activatedAt: at,
      }));
      if (retired !== undefined) {
        versions = mapVersion(versions, retired, (entry) => ({
          ...entry,
          state: SecretVersionState.SOFT_DELETED,
          retiredAt: at,
        }));
      }
      return {
        ...base,
        lifecycle: CredentialLifecycle.ACTIVE,
        lifecycleChangedAt: state.lifecycle === CredentialLifecycle.ACTIVE ? state.lifecycleChangedAt : at,
        activeVersion: activated,
        lastRotatedAt: at,
        origin: state.write?.origin ?? state.origin,
        versions,
        overlap: previous
          ? {
              version: Number(previous.reference!.version),
              startsAt: timestampDate(previous.startsAt!),
              expiresAt: timestampDate(previous.expiresAt!),
            }
          : state.overlap,
        write: state.write && { ...state.write, status: 'ACTIVATED', activatedAt: at },
      };
    }
    case 'writeRejected': {
      const saga = state.write;
      const candidate = saga?.candidateObservedAt ? saga.candidateVersion : undefined;
      return {
        ...base,
        lifecycle:
          state.lifecycle === CredentialLifecycle.PROVISIONING
            ? CredentialLifecycle.PROVISIONING_ABORTED
            : state.lifecycle,
        lifecycleChangedAt: state.lifecycle === CredentialLifecycle.PROVISIONING ? at : state.lifecycleChangedAt,
        versions:
          candidate === undefined
            ? state.versions
            : mapVersion(state.versions, candidate, (entry) => ({
                ...entry,
                state: SecretVersionState.SOFT_DELETED,
                retiredAt: at,
              })),
        write: saga && {
          ...saga,
          status: 'REJECTED',
          rejection: { reason: fact.value.reason, at, failedStep: failedStep(saga) },
        },
      };
    }
    case 'lifecycleChanged':
      return {
        ...base,
        lifecycle: fact.value.lifecycle,
        lifecycleReason: fact.value.reason,
        lifecycleChangedAt: at,
      };
    case 'destructionScheduled':
      return {
        ...base,
        lifecycle: CredentialLifecycle.DESTROY_PENDING,
        lifecycleReason: fact.value.reason,
        lifecycleChangedAt: at,
        destruction: {
          executeAfter: timestampDate(fact.value.executeAfter!),
          reason: fact.value.reason,
          scheduledAt: at,
        },
      };
    case 'versionDestructionObserved': {
      const version = Number(fact.value.version!.version);
      return {
        ...base,
        lifecycle: CredentialLifecycle.CLEANUP_PENDING,
        lifecycleChangedAt: at,
        versions: mapVersion(state.versions, version, (entry) => ({
          ...entry,
          state: SecretVersionState.DESTROYED,
          destroyedAt: at,
        })),
      };
    }
    case 'destroyed':
      return {
        ...base,
        lifecycle: CredentialLifecycle.DESTROYED,
        lifecycleChangedAt: at,
        destroyedAt: at,
        overlap: undefined,
      };
    case 'webhookRotationOverlapEnded': {
      const version = Number(fact.value.previousVersion!.version);
      return {
        ...base,
        overlap: undefined,
        versions: mapVersion(state.versions, version, (entry) => ({
          ...entry,
          state: SecretVersionState.SOFT_DELETED,
          retiredAt: at,
        })),
      };
    }
    case 'audit':
      return evolveAudit(base, fact.value.fact, at);
    case 'lease':
      return evolveLease(base, fact.value.fact, at);
    default:
      return base;
  }
}

type AuditFact = Extract<CredentialEvent['fact'], { case: 'audit' }>['value']['fact'];
type LeaseFact = Extract<CredentialEvent['fact'], { case: 'lease' }>['value']['fact'];

function evolveAudit(state: CredentialState, fact: AuditFact, at: Date): CredentialState {
  switch (fact.case) {
    case 'accepted': {
      const authorization = fact.value.authorization!;
      const material = authorization.material;
      return {
        ...state,
        uses: [
          ...state.uses,
          {
            id: authorization.id,
            workloadPrincipalId: authorization.workloadPrincipalId,
            operationId: authorization.operationId,
            purpose: authorization.purpose,
            action: authorization.action,
            direction: authorization.direction,
            mode: authorization.mode,
            audience: authorization.audience,
            bindingId: authorization.bindingId,
            bindingRevision: Number(authorization.bindingRevision),
            issuedAt: timestampDate(authorization.issuedAt!),
            expiresAt: timestampDate(authorization.expiresAt!),
            version: material.case === 'activeVersion' ? Number(material.value.version) : undefined,
          },
        ],
      };
    }
    case 'completed':
      return {
        ...state,
        uses: updateUse(state.uses, fact.value.authorizationId, (use) => ({
          ...use,
          result: fact.value.result,
          failureReason: fact.value.failureReason,
          completedAt: at,
        })),
      };
    case 'authorizationEnded':
      return {
        ...state,
        uses: updateUse(state.uses, fact.value.authorizationId, (use) => ({
          ...use,
          endedReason: fact.value.reason,
          endedAt: at,
        })),
      };
    default:
      return state;
  }
}

function evolveLease(state: CredentialState, fact: LeaseFact, at: Date): CredentialState {
  switch (fact.case) {
    case 'reserved':
      return {
        ...state,
        leases: [
          ...state.leases,
          {
            id: fact.value.lease!.id as LeaseRefId,
            workloadPrincipalId: fact.value.workloadPrincipalId,
            operationId: fact.value.operationId,
            audience: fact.value.audience,
            bindingId: fact.value.bindingId,
            bindingRevision: Number(fact.value.bindingRevision),
            reservedAt: at,
            replaces: fact.value.replaces?.id,
          },
        ],
      };
    case 'issued':
      return {
        ...state,
        leases: updateLease(state.leases, fact.value.lease!.id, (lease) => ({
          ...lease,
          issuedAt: timestampDate(fact.value.issuedAt!),
          expiresAt: timestampDate(fact.value.expiresAt!),
          providerExpiresAt: timestampDate(fact.value.providerExpiresAt!),
        })),
      };
    case 'useEnded':
      return {
        ...state,
        leases: updateLease(state.leases, fact.value.lease!.id, (lease) => ({
          ...lease,
          endedAt: at,
          endReason: fact.value.reason,
        })),
      };
    case 'cleanupConfirmed':
      return {
        ...state,
        leases: updateLease(state.leases, fact.value.lease!.id, (lease) => ({ ...lease, cleanupConfirmedAt: at })),
      };
    default:
      return state;
  }
}

export function foldCredential(events: readonly CredentialEvent[]): CredentialState | undefined {
  return events.reduce<CredentialState | undefined>(evolveCredential, undefined);
}
