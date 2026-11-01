import {
  CredentialLifecycle,
  CredentialWriteKind,
  SecretVersionState,
  VaultLifecycle,
  type CredentialPolicyReason,
  type SecretFailureReason,
  type SecretMaterialKind,
  type SecretMaterialOrigin,
  type SecretPurpose,
} from '@trogonai/proto';

import type { CustodyTier } from '@/features/custody/domain/custody';
import type { EnvironmentId } from '@/features/environments/domain/environment-id';

import {
  hasOutstandingWrite,
  isLive,
  liveLeases,
  type CredentialState,
  type LeaseRecord,
  type UseRecord,
  type VersionEntry,
  type WriteStepKey,
} from './credential-state';
import { verificationOutcomeLabels } from './labels';
import { isWebhookPurpose } from './policy';
import type { SecretRefId } from './secret-ref';
import type { VaultState } from './vault';

export type VersionRole = 'CURRENT' | 'OVERLAP' | 'CANDIDATE' | 'RETIRED' | 'REJECTED' | 'DESTROYED';

export interface VersionView {
  readonly version: number;
  readonly role: VersionRole;
  readonly state: SecretVersionState;
  readonly observedAt: Date;
  readonly activatedAt?: Date;
  readonly retiredAt?: Date;
  readonly destroyedAt?: Date;
  readonly overlapExpiresAt?: Date;
}

export type StepStatus = 'done' | 'current' | 'pending' | 'failed' | 'skipped';

export interface SagaStepView {
  readonly key: WriteStepKey;
  readonly label: string;
  readonly status: StepStatus;
  readonly at?: Date;
  readonly detail?: string;
  readonly failure?: SecretFailureReason;
}

export interface SagaView {
  readonly kind: CredentialWriteKind;
  readonly origin: SecretMaterialOrigin;
  readonly status: 'IN_FLIGHT' | 'ACTIVATED' | 'REJECTED';
  readonly candidateVersion: number;
  readonly steps: readonly SagaStepView[];
  readonly awaitingVerification: boolean;
  readonly rejection?: { readonly reason: SecretFailureReason; readonly at: Date };
}

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

export interface CredentialActions {
  readonly rotate: ActionAvailability;
  readonly disable: ActionAvailability;
  readonly enable: ActionAvailability;
  readonly revoke: ActionAvailability;
  readonly scheduleDestruction: ActionAvailability;
  readonly confirmDestruction: ActionAvailability;
  readonly completeVerification: ActionAvailability;
  readonly endOverlap: ActionAvailability;
  readonly rename: ActionAvailability;
}

export interface DestructionView {
  readonly executeAfter: Date;
  readonly reason: CredentialPolicyReason;
  readonly scheduledAt: Date;
  readonly due: boolean;
}

export interface CredentialView {
  readonly secret: SecretRefId;
  readonly revision: number;
  readonly displayName: string;
  readonly vaultId: string;
  readonly purpose: SecretPurpose;
  readonly kind: SecretMaterialKind;
  readonly environment: EnvironmentId;
  readonly origin?: SecretMaterialOrigin;
  readonly lifecycle: CredentialLifecycle;
  readonly lifecycleReason?: CredentialPolicyReason;
  readonly activeVersion?: number;
  readonly lastRotatedAt?: Date;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly versions: readonly VersionView[];
  readonly overlap?: { readonly version: number; readonly startsAt: Date; readonly expiresAt: Date };
  readonly saga?: SagaView;
  readonly destruction?: DestructionView;
  readonly uses: readonly UseRecord[];
  readonly leases: readonly LeaseRecord[];
  readonly liveLeaseCount: number;
  readonly needsAttention: boolean;
  readonly attentionReason?: string;
  readonly actions: CredentialActions;
}

export interface VaultView {
  readonly vaultId: string;
  readonly revision: number;
  readonly displayName: string;
  readonly environment: EnvironmentId;
  readonly custody: CustodyTier;
  readonly lifecycle: VaultLifecycle;
  readonly createdAt: Date;
  readonly credentialCount: number;
  readonly liveCredentialCount: number;
  readonly byLifecycle: ReadonlyMap<CredentialLifecycle, number>;
  readonly actions: {
    readonly rename: ActionAvailability;
    readonly addCredential: ActionAvailability;
    readonly disable: ActionAvailability;
    readonly enable: ActionAvailability;
    readonly scheduleDestruction: ActionAvailability;
    readonly confirmDestroyed: ActionAvailability;
  };
}

const yes: ActionAvailability = { available: true };
const no = (reason: string): ActionAvailability => ({ available: false, reason });
const gate = (condition: boolean, reason: string): ActionAvailability => (condition ? yes : no(reason));

function sagaSteps(credential: CredentialState): readonly SagaStepView[] {
  const write = credential.write;
  if (!write) return [];
  const rejection = write.rejection;
  const failedAt = rejection?.failedStep;
  const verificationOutcome = write.verification?.outcome;
  const order: readonly WriteStepKey[] = [
    'RESERVED',
    'CANDIDATE_WRITE_RESERVED',
    'CANDIDATE_OBSERVED',
    'VERIFICATION',
    'ACTIVATED',
  ];
  const labels: Record<WriteStepKey, string> = {
    RESERVED: 'Reserved',
    CANDIDATE_WRITE_RESERVED: 'Candidate write reserved',
    CANDIDATE_OBSERVED: 'Candidate observed',
    VERIFICATION: 'Verification',
    ACTIVATED: 'Activated',
  };
  const doneAt = (key: WriteStepKey): Date | undefined => {
    switch (key) {
      case 'RESERVED':
        return write.candidateWriteReservedAt;
      case 'CANDIDATE_WRITE_RESERVED':
        return write.candidateWriteReservedAt;
      case 'CANDIDATE_OBSERVED':
        return write.candidateObservedAt;
      case 'VERIFICATION':
        return write.verification?.at;
      case 'ACTIVATED':
        return write.activatedAt;
    }
  };
  let currentAssigned = false;
  return order.map((key): SagaStepView => {
    const at = doneAt(key);
    if (key === 'VERIFICATION' && !write.verificationRequired) {
      return { key, label: labels[key], status: 'skipped', detail: 'Generated locally, no external verification.' };
    }
    if (rejection && failedAt === key) {
      return {
        key,
        label: labels[key],
        status: 'failed',
        at: rejection.at,
        failure: rejection.reason,
        detail:
          key === 'VERIFICATION' && verificationOutcome !== undefined
            ? verificationOutcomeLabels[verificationOutcome]
            : undefined,
      };
    }
    if (at !== undefined) return { key, label: labels[key], status: 'done', at };
    if (rejection || currentAssigned) return { key, label: labels[key], status: 'pending' };
    currentAssigned = true;
    return { key, label: labels[key], status: 'current' };
  });
}

function sagaView(credential: CredentialState): SagaView | undefined {
  const write = credential.write;
  if (!write) return undefined;
  return {
    kind: write.kind ?? CredentialWriteKind.INITIAL,
    origin: write.origin,
    status: write.status,
    candidateVersion: write.candidateVersion,
    steps: sagaSteps(credential),
    awaitingVerification:
      write.status === 'IN_FLIGHT' &&
      write.verificationRequired &&
      write.candidateObservedAt !== undefined &&
      write.verification === undefined,
    rejection: write.rejection && { reason: write.rejection.reason, at: write.rejection.at },
  };
}

function versionRole(credential: CredentialState, entry: VersionEntry): VersionRole {
  if (entry.state === SecretVersionState.DESTROYED) return 'DESTROYED';
  if (credential.activeVersion === entry.version) return 'CURRENT';
  if (credential.overlap?.version === entry.version) return 'OVERLAP';
  if (!entry.activated) {
    return credential.write?.status === 'IN_FLIGHT' && credential.write.candidateVersion === entry.version
      ? 'CANDIDATE'
      : 'REJECTED';
  }
  return 'RETIRED';
}

function versionViews(credential: CredentialState): readonly VersionView[] {
  return [...credential.versions]
    .sort((left, right) => right.version - left.version)
    .map((entry) => ({
      version: entry.version,
      role: versionRole(credential, entry),
      state: entry.state,
      observedAt: entry.observedAt,
      activatedAt: entry.activatedAt,
      retiredAt: entry.retiredAt,
      destroyedAt: entry.destroyedAt,
      overlapExpiresAt: credential.overlap?.version === entry.version ? credential.overlap.expiresAt : undefined,
    }));
}

function attention(
  credential: CredentialState,
  now: Date,
  saga: SagaView | undefined,
): { needsAttention: boolean; attentionReason?: string } {
  if (credential.lifecycle === CredentialLifecycle.RECONNECT_REQUIRED) {
    return { needsAttention: true, attentionReason: 'The provider authorization is gone. Reconnect it.' };
  }
  if (credential.lifecycle === CredentialLifecycle.PROVISIONING_ABORTED) {
    return { needsAttention: true, attentionReason: 'Provisioning was aborted. Schedule destruction.' };
  }
  if (saga?.awaitingVerification) {
    return { needsAttention: true, attentionReason: 'A candidate is waiting for verification.' };
  }
  if (saga?.status === 'REJECTED' && credential.lifecycle === CredentialLifecycle.ACTIVE) {
    return { needsAttention: true, attentionReason: 'The last write was rejected.' };
  }
  if (
    credential.lifecycle === CredentialLifecycle.DESTROY_PENDING &&
    credential.destruction &&
    credential.destruction.executeAfter <= now
  ) {
    return { needsAttention: true, attentionReason: 'Destruction is due. Confirm it.' };
  }
  return { needsAttention: false };
}

function credentialActions(credential: CredentialState, now: Date, saga: SagaView | undefined): CredentialActions {
  const lifecycle = credential.lifecycle;
  const destroyed = lifecycle === CredentialLifecycle.DESTROYED;
  const inFlight = hasOutstandingWrite(credential);
  const dead = (reason: string) => (destroyed ? no('The credential is destroyed.') : no(reason));
  const rotatable = lifecycle === CredentialLifecycle.ACTIVE || lifecycle === CredentialLifecycle.RECONNECT_REQUIRED;
  const revocable = rotatable || lifecycle === CredentialLifecycle.DISABLED;
  const destroyable =
    lifecycle === CredentialLifecycle.DISABLED ||
    lifecycle === CredentialLifecycle.REVOKED ||
    lifecycle === CredentialLifecycle.PROVISIONING_ABORTED;
  const due = credential.destruction ? credential.destruction.executeAfter <= now : false;
  return {
    rotate: !rotatable
      ? dead('Only an active credential, or one that needs a reconnect, can be rotated.')
      : inFlight
        ? no('Finish the outstanding write first.')
        : credential.overlap
          ? no('End the open webhook overlap before rotating again.')
          : yes,
    disable:
      lifecycle !== CredentialLifecycle.ACTIVE
        ? dead('Only an active credential can be disabled.')
        : gate(!inFlight, 'Finish the outstanding write first.'),
    enable:
      lifecycle === CredentialLifecycle.DISABLED
        ? yes
        : lifecycle === CredentialLifecycle.REVOKED
          ? no('A revoked credential is never re-enabled.')
          : dead('Only a disabled credential can be enabled.'),
    revoke: revocable ? yes : dead('This credential can no longer be revoked.'),
    scheduleDestruction: destroyable
      ? yes
      : dead('Disable or revoke the credential before scheduling its destruction.'),
    confirmDestruction:
      lifecycle !== CredentialLifecycle.DESTROY_PENDING
        ? dead('Schedule the destruction first.')
        : !due
          ? no('Destruction is not due yet.')
          : liveLeases(credential).length > 0
            ? no('A lease is still live.')
            : yes,
    completeVerification: gate(saga?.awaitingVerification === true, 'No candidate is waiting for verification.'),
    endOverlap: gate(credential.overlap !== undefined, 'No webhook overlap window is open.'),
    rename: gate(!destroyed, 'A destroyed credential cannot be renamed.'),
  };
}

export function toCredentialView(credential: CredentialState, now: Date = new Date()): CredentialView {
  const saga = sagaView(credential);
  const live = liveLeases(credential).filter((lease) => !lease.expiresAt || lease.expiresAt > now);
  return {
    secret: credential.secret,
    revision: credential.revision,
    displayName: credential.displayName,
    vaultId: credential.vaultId,
    purpose: credential.purpose,
    kind: credential.kind,
    environment: credential.environment,
    origin: credential.origin ?? credential.write?.origin,
    lifecycle: credential.lifecycle,
    lifecycleReason: credential.lifecycleReason,
    activeVersion: credential.activeVersion,
    lastRotatedAt: credential.lastRotatedAt,
    createdAt: credential.createdAt,
    updatedAt: credential.updatedAt,
    versions: versionViews(credential),
    overlap: credential.overlap,
    saga,
    destruction: credential.destruction && {
      ...credential.destruction,
      due: credential.destruction.executeAfter <= now,
    },
    uses: [...credential.uses].sort((left, right) => right.issuedAt.getTime() - left.issuedAt.getTime()),
    leases: [...credential.leases].sort((left, right) => right.reservedAt.getTime() - left.reservedAt.getTime()),
    liveLeaseCount: live.length,
    ...attention(credential, now, saga),
    actions: credentialActions(credential, now, saga),
  };
}

export function toVaultView(vault: VaultState, credentials: readonly CredentialState[]): VaultView {
  const members = credentials.filter((credential) => credential.vaultId === vault.vaultId);
  const byLifecycle = new Map<CredentialLifecycle, number>();
  for (const member of members) byLifecycle.set(member.lifecycle, (byLifecycle.get(member.lifecycle) ?? 0) + 1);
  const liveCount = members.filter(isLive).length;
  const lifecycle = vault.lifecycle;
  const blocked = liveCount > 0;
  const blockedReason = `${liveCount} live ${liveCount === 1 ? 'credential' : 'credentials'} must be destroyed first.`;
  const destroyedVault = lifecycle === VaultLifecycle.DESTROYED;
  return {
    vaultId: vault.vaultId,
    revision: vault.revision,
    displayName: vault.displayName,
    environment: vault.environment,
    custody: vault.custody,
    lifecycle,
    createdAt: vault.createdAt,
    credentialCount: members.length,
    liveCredentialCount: liveCount,
    byLifecycle,
    actions: {
      rename: gate(!destroyedVault, 'A destroyed vault cannot be renamed.'),
      addCredential: gate(lifecycle === VaultLifecycle.ACTIVE, 'Only an active vault accepts new credentials.'),
      disable: gate(lifecycle === VaultLifecycle.ACTIVE, 'Only an active vault can be disabled.'),
      enable: gate(lifecycle === VaultLifecycle.DISABLED, 'Only a disabled vault can be enabled.'),
      scheduleDestruction:
        lifecycle !== VaultLifecycle.ACTIVE && lifecycle !== VaultLifecycle.DISABLED
          ? no('The vault is already past this step.')
          : blocked
            ? no(blockedReason)
            : yes,
      confirmDestroyed:
        lifecycle !== VaultLifecycle.DESTROY_PENDING
          ? no('Schedule the destruction first.')
          : blocked
            ? no(blockedReason)
            : yes,
    },
  };
}

export interface SecretsOverview {
  readonly activeCredentials: number;
  readonly openOverlaps: number;
  readonly writesInFlight: number;
  readonly needsAttention: number;
}

export function summarize(credentials: readonly CredentialView[]): SecretsOverview {
  return {
    activeCredentials: credentials.filter((entry) => entry.lifecycle === CredentialLifecycle.ACTIVE).length,
    openOverlaps: credentials.filter((entry) => entry.overlap !== undefined).length,
    writesInFlight: credentials.filter((entry) => entry.saga?.status === 'IN_FLIGHT').length,
    needsAttention: credentials.filter((entry) => entry.needsAttention).length,
  };
}

export function isWebhook(view: Pick<CredentialView, 'purpose'>): boolean {
  return isWebhookPurpose(view.purpose);
}
