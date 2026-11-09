import type { MessageInitShape } from '@bufbuild/protobuf';
import type {
  CredentialPolicyReason,
  MaterialVerificationOutcome,
  SecretAuditEventSchema,
  SecretLeaseEventSchema,
  SecretMaterialKind,
  SecretPurpose,
} from '@trogonai/proto';

import type { EnvironmentId } from '@/features/environments/domain/environment-id';

import { mintSecretRefId, mintShortId, type SecretRefId } from './secret-ref';

export type AuditFact = MessageInitShape<typeof SecretAuditEventSchema>['fact'];
export type LeaseFact = MessageInitShape<typeof SecretLeaseEventSchema>['fact'];

export type VerificationSimulation = MaterialVerificationOutcome | 'PENDING';

export type MaterialInput = { readonly case: 'imported'; readonly material: string } | { readonly case: 'generated' };

export type VaultTransition = 'DISABLE' | 'ENABLE' | 'SCHEDULE_DESTRUCTION' | 'CONFIRM_DESTROYED';

export type VaultCommand =
  | {
      readonly case: 'createVault';
      readonly vaultId: string;
      readonly displayName: string;
      readonly environment: EnvironmentId;
    }
  | { readonly case: 'renameVault'; readonly vaultId: string; readonly displayName: string }
  | { readonly case: 'changeVaultLifecycle'; readonly vaultId: string; readonly transition: VaultTransition };

export type CredentialCommand =
  | {
      readonly case: 'createCredential';
      readonly secret: SecretRefId;
      readonly vaultId: string;
      readonly displayName: string;
      readonly purpose: SecretPurpose;
      readonly kind: SecretMaterialKind;
      readonly material: MaterialInput;
      readonly simulation?: VerificationSimulation;
    }
  | { readonly case: 'renameCredential'; readonly secret: SecretRefId; readonly displayName: string }
  | {
      readonly case: 'rotateCredential';
      readonly secret: SecretRefId;
      readonly material: MaterialInput;
      readonly overlapHours?: number;
      readonly simulation?: VerificationSimulation;
    }
  | {
      readonly case: 'completeVerification';
      readonly secret: SecretRefId;
      readonly outcome: MaterialVerificationOutcome;
    }
  | { readonly case: 'disableCredential'; readonly secret: SecretRefId }
  | { readonly case: 'enableCredential'; readonly secret: SecretRefId }
  | { readonly case: 'markReconnectRequired'; readonly secret: SecretRefId }
  | { readonly case: 'revokeCredential'; readonly secret: SecretRefId; readonly reason: CredentialPolicyReason }
  | { readonly case: 'endWebhookOverlap'; readonly secret: SecretRefId; readonly reason?: CredentialPolicyReason }
  | {
      readonly case: 'scheduleDestruction';
      readonly secret: SecretRefId;
      readonly executeAfter: Date;
      readonly reason?: CredentialPolicyReason;
    }
  | { readonly case: 'confirmDestruction'; readonly secret: SecretRefId }
  | { readonly case: 'recordAudit'; readonly secret: SecretRefId; readonly fact: AuditFact }
  | { readonly case: 'recordLease'; readonly secret: SecretRefId; readonly fact: LeaseFact };

export const vaultCommands = {
  create: (input: { displayName: string; environment: EnvironmentId; vaultId?: string }): VaultCommand => ({
    case: 'createVault',
    vaultId: input.vaultId ?? mintShortId('vlt'),
    displayName: input.displayName,
    environment: input.environment,
  }),
  rename: (vaultId: string, displayName: string): VaultCommand => ({ case: 'renameVault', vaultId, displayName }),
  transition: (vaultId: string, transition: VaultTransition): VaultCommand => ({
    case: 'changeVaultLifecycle',
    vaultId,
    transition,
  }),
};

export const credentialCommands = {
  create: (input: {
    vaultId: string;
    displayName: string;
    purpose: SecretPurpose;
    kind: SecretMaterialKind;
    material: MaterialInput;
    simulation?: VerificationSimulation;
    secret?: SecretRefId;
  }): CredentialCommand => ({
    case: 'createCredential',
    secret: input.secret ?? mintSecretRefId(),
    vaultId: input.vaultId,
    displayName: input.displayName,
    purpose: input.purpose,
    kind: input.kind,
    material: input.material,
    simulation: input.simulation,
  }),
  rename: (secret: SecretRefId, displayName: string): CredentialCommand => ({
    case: 'renameCredential',
    secret,
    displayName,
  }),
  rotate: (
    secret: SecretRefId,
    input: { material: MaterialInput; overlapHours?: number; simulation?: VerificationSimulation },
  ): CredentialCommand => ({ case: 'rotateCredential', secret, ...input }),
  completeVerification: (secret: SecretRefId, outcome: MaterialVerificationOutcome): CredentialCommand => ({
    case: 'completeVerification',
    secret,
    outcome,
  }),
  disable: (secret: SecretRefId): CredentialCommand => ({ case: 'disableCredential', secret }),
  enable: (secret: SecretRefId): CredentialCommand => ({ case: 'enableCredential', secret }),
  markReconnectRequired: (secret: SecretRefId): CredentialCommand => ({ case: 'markReconnectRequired', secret }),
  revoke: (secret: SecretRefId, reason: CredentialPolicyReason): CredentialCommand => ({
    case: 'revokeCredential',
    secret,
    reason,
  }),
  endWebhookOverlap: (secret: SecretRefId, reason?: CredentialPolicyReason): CredentialCommand => ({
    case: 'endWebhookOverlap',
    secret,
    reason,
  }),
  scheduleDestruction: (
    secret: SecretRefId,
    executeAfter: Date,
    reason?: CredentialPolicyReason,
  ): CredentialCommand => ({ case: 'scheduleDestruction', secret, executeAfter, reason }),
  confirmDestruction: (secret: SecretRefId): CredentialCommand => ({ case: 'confirmDestruction', secret }),
  recordAudit: (secret: SecretRefId, fact: AuditFact): CredentialCommand => ({
    case: 'recordAudit',
    secret,
    fact,
  }),
  recordLease: (secret: SecretRefId, fact: LeaseFact): CredentialCommand => ({
    case: 'recordLease',
    secret,
    fact,
  }),
};

export type SecretCommand = VaultCommand | CredentialCommand;
