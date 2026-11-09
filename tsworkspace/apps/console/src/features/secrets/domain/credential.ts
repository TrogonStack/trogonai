import { create } from '@bufbuild/protobuf';
import { timestampDate, timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  CredentialLifecycle,
  CredentialPolicyReason,
  CredentialWriteKind,
  MaterialVerificationOutcome,
  SecretAuditEventSchema,
  SecretFailureReason,
  SecretLeaseEventSchema,
  SecretMaterialKind,
  SecretMaterialOrigin,
  SecretMetadataCleanupScope,
  SecretPurpose,
  SecretUseAction,
  SecretUseDirection,
  SecretUseMode,
  SecretVersionState,
  VaultLifecycle,
  type CredentialEvent,
  type SecretUseAuthorization,
} from '@trogonai/proto';

import type { CredentialCommand, MaterialInput, VerificationSimulation } from './commands';
import type { DecisionContext } from './context';
import {
  hasOutstandingWrite,
  liveLeases,
  nextVersion,
  type CredentialState,
  type EventPointer,
} from './credential-state';
import { Emitter, eventRef, fakeDigest } from './events';
import { SecretFault } from './faults';
import { credentialLifecycleLabels, selectableRevocationReasons, vaultLifecycleLabels } from './labels';
import {
  MAX_AUTHORIZATION_SECONDS,
  MAX_OVERLAP_HOURS,
  admitsGeneration,
  admitsLease,
  isCompatible,
  isWebhookPurpose,
  requiresExternalVerification,
} from './policy';
import { parseSecretRefId, secretVersionRefMessage, type SecretRefId } from './secret-ref';
import { validateDisplayName } from './vault';

export type CredentialDecision =
  | { readonly ok: true; readonly events: readonly CredentialEvent[] }
  | { readonly ok: false; readonly fault: SecretFault };

const HOUR_MS = 3_600_000;
const SECOND_MS = 1_000;

interface WriteContinuation {
  readonly secret: SecretRefId;
  readonly provisioningId: string;
  readonly purpose: SecretPurpose;
  readonly kind: CredentialWriteKind;
  readonly candidateVersion: number;
  readonly priorActive?: number;
  readonly overlapExpiresAt?: Date;
  readonly writeReserved: EventPointer;
  readonly requirementId?: string;
}

export function validateMaterial(material: MaterialInput, purpose: SecretPurpose, kind: SecretMaterialKind): void {
  if (material.case === 'generated') {
    if (!admitsGeneration(purpose)) {
      throw new SecretFault('PURPOSE_MISMATCH', 'Only webhook shared secrets can be generated.');
    }
    return;
  }
  const value = material.material;
  if (value.trim().length === 0) throw new SecretFault('INVALID_MATERIAL', 'The material is empty.');
  if (kind === SecretMaterialKind.WEBHOOK_DESTINATION) {
    let url: URL | undefined;
    try {
      url = new URL(value.trim());
    } catch {
      url = undefined;
    }
    if (!url || url.protocol !== 'https:') {
      throw new SecretFault('INVALID_MATERIAL', 'A webhook destination must be an https URL.');
    }
    return;
  }
  if (kind === SecretMaterialKind.BASIC_AUTHENTICATION) {
    if (!value.includes(':')) throw new SecretFault('INVALID_MATERIAL', 'Basic authentication is "user:password".');
    return;
  }
  if (/\s/.test(value.trim())) throw new SecretFault('INVALID_MATERIAL', 'The material must not contain whitespace.');
  if (kind === SecretMaterialKind.WEBHOOK_SHARED_SECRET && value.length < 16) {
    throw new SecretFault('INVALID_MATERIAL', 'A shared secret needs at least 16 characters of entropy.');
  }
  if (value.length < 8) throw new SecretFault('INVALID_MATERIAL', 'The material is too short to be a credential.');
}

function requireCredential(state: CredentialState | undefined): CredentialState {
  if (!state) throw new SecretFault('NOT_FOUND', 'The credential does not exist.');
  if (state.lifecycle === CredentialLifecycle.DESTROYED) {
    throw new SecretFault('INVALID_LIFECYCLE', 'The credential is destroyed. Only its tombstone remains.');
  }
  return state;
}

function requireLifecycle(state: CredentialState, allowed: readonly CredentialLifecycle[], hint?: string): void {
  if (allowed.includes(state.lifecycle)) return;
  const current = credentialLifecycleLabels[state.lifecycle].toLowerCase();
  throw new SecretFault(
    'INVALID_LIFECYCLE',
    `A ${current} credential cannot take this action.${hint ? ` ${hint}` : ''}`,
  );
}

function requireNoWrite(state: CredentialState): void {
  if (hasOutstandingWrite(state)) {
    throw new SecretFault('OPERATION_IN_PROGRESS', 'Finish the outstanding write before starting another action.');
  }
}

function provisioning(context: DecisionContext, id: string) {
  return { id, projectId: context.projectId };
}

function emitActivation(
  emitter: Emitter,
  context: DecisionContext,
  write: WriteContinuation,
  verification?: EventPointer,
): void {
  const overlaps =
    write.overlapExpiresAt !== undefined && write.priorActive !== undefined && write.overlapExpiresAt > context.now;
  emitter.emit({
    case: 'versionActivated',
    value: {
      provisioning: provisioning(context, write.provisioningId),
      version: secretVersionRefMessage(write.secret, write.candidateVersion),
      prepared: eventRef(write.secret, write.writeReserved),
      verificationEvidence: verification ? [eventRef(write.secret, verification)] : [],
      previousWebhookVersion:
        overlaps && write.priorActive !== undefined && write.overlapExpiresAt
          ? {
              reference: secretVersionRefMessage(write.secret, write.priorActive),
              expiresAt: timestampFromDate(write.overlapExpiresAt),
              startsAt: timestampFromDate(context.now),
            }
          : undefined,
      retiredVersion:
        !overlaps && write.priorActive !== undefined
          ? secretVersionRefMessage(write.secret, write.priorActive)
          : undefined,
    },
  });
}

function emitVerification(
  emitter: Emitter,
  context: DecisionContext,
  write: WriteContinuation,
  outcome: MaterialVerificationOutcome,
): void {
  const passed = outcome === MaterialVerificationOutcome.PASSED;
  const pointer = emitter.emit({
    case: 'materialVerificationRecorded',
    value: {
      verification: {
        provisioning: provisioning(context, write.provisioningId),
        candidateVersion: secretVersionRefMessage(write.secret, write.candidateVersion),
        requirementId: write.requirementId ?? 'requirement',
        connectionConfigurationDigest: fakeDigest(`connection:${write.secret}`),
        verifiedIdentity: passed
          ? { case: 'verifiedAccountFingerprint', value: fakeDigest(`account:${write.secret}`) }
          : undefined,
        evidenceId: context.newId('evd'),
        outcome,
        verifierPrincipalId: 'verifier-simulated',
        verifiedAt: timestampFromDate(context.now),
      },
    },
  });
  if (passed) {
    emitActivation(emitter, context, write, pointer);
    return;
  }
  emitter.emit({
    case: 'writeRejected',
    value: {
      provisioning: provisioning(context, write.provisioningId),
      reason:
        outcome === MaterialVerificationOutcome.PROVIDER_UNAVAILABLE
          ? SecretFailureReason.PROVIDER_UNAVAILABLE
          : SecretFailureReason.VERIFICATION_REJECTED,
    },
  });
}

function emitWrite(
  emitter: Emitter,
  context: DecisionContext,
  plan: {
    secret: SecretRefId;
    provisioningId: string;
    purpose: SecretPurpose;
    kind: CredentialWriteKind;
    origin: SecretMaterialOrigin;
    candidateVersion: number;
    priorActive?: number;
    overlapExpiresAt?: Date;
    simulation: VerificationSimulation;
    initialMarker?: EventPointer;
  },
): void {
  const required = requiresExternalVerification(plan.purpose);
  const requirementId = required ? context.newId('req') : undefined;
  const stepId = context.newId('step');
  const ownershipToken = context.newId('own');
  const reserved = emitter.emit({
    case: 'candidateWriteReserved',
    value: {
      provisioning: provisioning(context, plan.provisioningId),
      stepId,
      ownershipToken,
      expectedStorageVersion: BigInt(plan.priorActive ?? 0),
      kind: plan.kind,
      priorActiveVersion:
        plan.priorActive !== undefined ? secretVersionRefMessage(plan.secret, plan.priorActive) : undefined,
      requiredVerifications:
        required && requirementId
          ? [
              {
                id: requirementId,
                connectionId: `conn_${plan.secret.slice(0, 8)}`,
                bindingId: `bind_${plan.secret.slice(0, 8)}`,
                bindingRevision: 1n,
                connectionConfigurationDigest: fakeDigest(`connection:${plan.secret}`),
                endpointPolicyDigest: fakeDigest(`endpoint:${plan.secret}`),
                expectedIdentity: {
                  case: 'expectedAccountFingerprint',
                  value: fakeDigest(`account:${plan.secret}`),
                },
              },
            ]
          : [],
      previousWebhookExpiresAt: plan.overlapExpiresAt ? timestampFromDate(plan.overlapExpiresAt) : undefined,
      origin: plan.origin,
      initialMarker: plan.initialMarker ? eventRef(plan.secret, plan.initialMarker) : undefined,
    },
  });
  emitter.emit({
    case: 'candidateObserved',
    value: {
      provisioning: provisioning(context, plan.provisioningId),
      stepId,
      attemptId: context.newId('att'),
      fence: 1n,
      candidate: {
        reference: secretVersionRefMessage(plan.secret, plan.candidateVersion),
        state: SecretVersionState.AVAILABLE,
        observedAt: timestampFromDate(context.now),
      },
      ownershipToken,
    },
  });
  const continuation: WriteContinuation = {
    secret: plan.secret,
    provisioningId: plan.provisioningId,
    purpose: plan.purpose,
    kind: plan.kind,
    candidateVersion: plan.candidateVersion,
    priorActive: plan.priorActive,
    overlapExpiresAt: plan.overlapExpiresAt,
    writeReserved: reserved,
    requirementId,
  };
  if (!required) {
    emitActivation(emitter, context, continuation);
    return;
  }
  if (plan.simulation === 'PENDING') return;
  emitVerification(emitter, context, continuation, plan.simulation);
}

function resolveOverlap(state: CredentialState, hours: number | undefined, context: DecisionContext): Date | undefined {
  if (hours === undefined || hours === 0) return undefined;
  if (!Number.isFinite(hours) || hours < 0 || hours > MAX_OVERLAP_HOURS) {
    throw new SecretFault('COMMAND_INVALID', `A webhook overlap is between 0 and ${MAX_OVERLAP_HOURS} hours.`);
  }
  if (!isWebhookPurpose(state.purpose)) {
    throw new SecretFault('PURPOSE_MISMATCH', 'Only webhook credentials admit a rotation overlap.');
  }
  return new Date(context.now.getTime() + hours * HOUR_MS);
}

function createCredential(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'createCredential' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  if (state) throw new SecretFault('IDEMPOTENCY_CONFLICT', 'A credential with this secret reference already exists.');
  if (!parseSecretRefId(command.secret)) {
    throw new SecretFault('COMMAND_INVALID', 'A secret reference is 43 base64url characters.');
  }
  const displayName = validateDisplayName(command.displayName);
  const vault = context.vault(command.vaultId);
  if (!vault) throw new SecretFault('NOT_FOUND', 'The vault does not exist.');
  if (vault.lifecycle !== VaultLifecycle.ACTIVE) {
    throw new SecretFault(
      'INVALID_LIFECYCLE',
      `The vault is ${vaultLifecycleLabels[vault.lifecycle].toLowerCase()} and takes no new credentials.`,
    );
  }
  if (!isCompatible(command.purpose, command.kind)) {
    throw new SecretFault('PURPOSE_MISMATCH', 'The material kind is not permitted for this purpose.');
  }
  validateMaterial(command.material, command.purpose, command.kind);

  const emitter = new Emitter(context, command.secret, 0);
  const provisioningId = context.newId('prov');
  const reserved = emitter.emit({
    case: 'reserved',
    value: {
      provisioning: provisioning(context, provisioningId),
      vaultId: command.vaultId,
      displayName,
      policy: { purpose: command.purpose, materialKind: command.kind, environment: vault.environment },
    },
  });
  emitWrite(emitter, context, {
    secret: command.secret,
    provisioningId,
    purpose: command.purpose,
    kind: CredentialWriteKind.INITIAL,
    origin: command.material.case === 'generated' ? SecretMaterialOrigin.GENERATED : SecretMaterialOrigin.IMPORTED,
    candidateVersion: 1,
    simulation: command.simulation ?? MaterialVerificationOutcome.PASSED,
    initialMarker: reserved,
  });
  return emitter.events;
}

function rotateCredential(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'rotateCredential' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  requireLifecycle(credential, [CredentialLifecycle.ACTIVE, CredentialLifecycle.RECONNECT_REQUIRED]);
  requireNoWrite(credential);
  if (credential.overlap) {
    throw new SecretFault('OPERATION_IN_PROGRESS', 'End the open webhook overlap before rotating again.');
  }
  validateMaterial(command.material, credential.purpose, credential.kind);
  const reconnect = credential.lifecycle === CredentialLifecycle.RECONNECT_REQUIRED;
  if (reconnect && command.overlapHours) {
    throw new SecretFault('PURPOSE_MISMATCH', 'A reconnect takes no overlap window.');
  }
  const overlapExpiresAt = resolveOverlap(credential, command.overlapHours, context);
  const emitter = new Emitter(context, credential.secret, credential.revision);
  emitWrite(emitter, context, {
    secret: credential.secret,
    provisioningId: context.newId('prov'),
    purpose: credential.purpose,
    kind: reconnect ? CredentialWriteKind.RECONNECT : CredentialWriteKind.ROTATION,
    origin: command.material.case === 'generated' ? SecretMaterialOrigin.GENERATED : SecretMaterialOrigin.IMPORTED,
    candidateVersion: nextVersion(credential),
    priorActive: credential.activeVersion,
    overlapExpiresAt,
    simulation: command.simulation ?? MaterialVerificationOutcome.PASSED,
  });
  return emitter.events;
}

function completeVerification(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'completeVerification' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  const write = credential.write;
  if (
    !write ||
    write.status !== 'IN_FLIGHT' ||
    !write.verificationRequired ||
    !write.candidateObservedAt ||
    write.verification
  ) {
    throw new SecretFault('INVALID_LIFECYCLE', 'No candidate is waiting for verification.');
  }
  const emitter = new Emitter(context, credential.secret, credential.revision);
  emitVerification(
    emitter,
    context,
    {
      secret: credential.secret,
      provisioningId: write.provisioningId,
      purpose: credential.purpose,
      kind: write.kind,
      candidateVersion: write.candidateVersion,
      priorActive: write.priorActiveVersion,
      overlapExpiresAt: write.previousWebhookExpiresAt,
      writeReserved: write.writeReserved,
      requirementId: write.requirementId,
    },
    command.outcome,
  );
  return emitter.events;
}

function changeLifecycle(emitter: Emitter, lifecycle: CredentialLifecycle, reason: CredentialPolicyReason): void {
  emitter.emit({ case: 'lifecycleChanged', value: { lifecycle, reason } });
}

function endOverlapIfOpen(emitter: Emitter, state: CredentialState, reason: CredentialPolicyReason): void {
  if (!state.overlap) return;
  emitter.emit({
    case: 'webhookRotationOverlapEnded',
    value: { previousVersion: secretVersionRefMessage(state.secret, state.overlap.version), reason },
  });
}

function abortWriteIfOutstanding(emitter: Emitter, context: DecisionContext, state: CredentialState): void {
  if (!state.write || state.write.status !== 'IN_FLIGHT') return;
  emitter.emit({
    case: 'writeRejected',
    value: {
      provisioning: provisioning(context, state.write.provisioningId),
      reason: SecretFailureReason.INVALID_LIFECYCLE,
    },
  });
}

function revokeCredential(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'revokeCredential' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  requireLifecycle(
    credential,
    [CredentialLifecycle.ACTIVE, CredentialLifecycle.DISABLED, CredentialLifecycle.RECONNECT_REQUIRED],
    'Revocation is irreversible and only applies to credentials that were provisioned.',
  );
  if (!selectableRevocationReasons.includes(command.reason)) {
    throw new SecretFault('COMMAND_INVALID', 'The revocation reason is not recognised.');
  }
  const emitter = new Emitter(context, credential.secret, credential.revision);
  abortWriteIfOutstanding(emitter, context, credential);
  endOverlapIfOpen(emitter, credential, command.reason);
  changeLifecycle(emitter, CredentialLifecycle.REVOKED, command.reason);
  return emitter.events;
}

function scheduleDestruction(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'scheduleDestruction' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  requireLifecycle(
    credential,
    [CredentialLifecycle.DISABLED, CredentialLifecycle.REVOKED, CredentialLifecycle.PROVISIONING_ABORTED],
    'Disable or revoke the credential first.',
  );
  const executeAfter = command.executeAfter < context.now ? context.now : command.executeAfter;
  const emitter = new Emitter(context, credential.secret, credential.revision);
  emitter.emit({
    case: 'destructionScheduled',
    value: {
      provisioning: provisioning(context, context.newId('prov')),
      executeAfter: timestampFromDate(executeAfter),
      reason: command.reason ?? credential.lifecycleReason ?? CredentialPolicyReason.OPERATOR_REQUESTED,
    },
  });
  return emitter.events;
}

function confirmDestruction(state: CredentialState | undefined, context: DecisionContext): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  requireLifecycle(credential, [CredentialLifecycle.DESTROY_PENDING], 'Schedule the destruction first.');
  const due = credential.destruction?.executeAfter;
  if (due && due > context.now) {
    throw new SecretFault('INVALID_LIFECYCLE', `Destruction is not due until ${due.toISOString()}.`);
  }
  if (liveLeases(credential).length > 0) {
    throw new SecretFault('OPERATION_IN_PROGRESS', 'A lease is still live. Wait for it to end and be cleaned up.');
  }
  const emitter = new Emitter(context, credential.secret, credential.revision);
  const removal = provisioning(context, context.newId('prov'));
  const pending = credential.versions.filter((entry) => entry.state !== SecretVersionState.DESTROYED);
  pending.forEach((entry, index) => {
    emitter.emit({
      case: 'versionDestructionObserved',
      value: {
        provisioning: removal,
        cleanupId: context.newId('cln'),
        version: secretVersionRefMessage(credential.secret, entry.version),
        ownershipToken: context.newId('own'),
        throughFence: BigInt(index + 1),
        observedAt: timestampFromDate(context.now),
      },
    });
  });
  const purge = emitter.emit({
    case: 'metadataPurgeObserved',
    value: {
      provisioning: removal,
      cleanupId: context.newId('cln'),
      scope: SecretMetadataCleanupScope.EXCLUSIVE_CREDENTIAL_KEY,
      throughFence: BigInt(pending.length + 1),
      observedAt: timestampFromDate(context.now),
    },
  });
  emitter.emit({
    case: 'destroyed',
    value: { provisioning: removal, cleanupEvidence: [eventRef(credential.secret, purge)] },
  });
  return emitter.events;
}

export function validateAuthorization(state: CredentialState, authorization: SecretUseAuthorization): void {
  const issued = authorization.issuedAt ? timestampDate(authorization.issuedAt).getTime() : 0;
  const expires = authorization.expiresAt ? timestampDate(authorization.expiresAt).getTime() : 0;
  if (expires <= issued || expires - issued > MAX_AUTHORIZATION_SECONDS * SECOND_MS) {
    throw new SecretFault('COMMAND_INVALID', `A use authorization lives at most ${MAX_AUTHORIZATION_SECONDS} seconds.`);
  }
  if (authorization.purpose !== state.purpose) {
    throw new SecretFault('PURPOSE_MISMATCH', 'The authorization purpose differs from the credential purpose.');
  }
  const { action, direction, mode } = authorization;
  if (action === SecretUseAction.COMPUTE_MAC) {
    if (state.purpose !== SecretPurpose.WEBHOOK_SIGNING || direction !== SecretUseDirection.OUTBOUND) {
      throw new SecretFault('PURPOSE_MISMATCH', 'Computing a MAC requires an outbound webhook signing secret.');
    }
  }
  if (action === SecretUseAction.VERIFY_MAC || action === SecretUseAction.VERIFY_TOKEN) {
    if (state.purpose !== SecretPurpose.WEBHOOK_VERIFICATION || direction !== SecretUseDirection.INBOUND) {
      throw new SecretFault('PURPOSE_MISMATCH', 'Verification requires an inbound webhook verification secret.');
    }
  }
  if (
    action === SecretUseAction.RESOLVE &&
    isWebhookPurpose(state.purpose) &&
    mode !== SecretUseMode.MANAGEMENT_PROVISIONING &&
    mode !== SecretUseMode.OWNED_CLEANUP
  ) {
    throw new SecretFault('PURPOSE_MISMATCH', 'Webhook material resolves only for provisioning or cleanup.');
  }
  if ((mode === SecretUseMode.OWNED_CLEANUP) !== (authorization.cleanupObligation !== undefined)) {
    throw new SecretFault('COMMAND_INVALID', 'A cleanup obligation accompanies owned cleanup and nothing else.');
  }
}

function recordAudit(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'recordAudit' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  const audit = create(SecretAuditEventSchema, { fact: command.fact });
  const fact = audit.fact;
  if (fact.case === 'accepted') {
    requireLifecycle(credential, [CredentialLifecycle.ACTIVE]);
    validateAuthorization(credential, fact.value.authorization!);
  }
  if (fact.case === 'completed' || fact.case === 'authorizationEnded') {
    if (!credential.uses.some((use) => use.id === fact.value.authorizationId)) {
      throw new SecretFault('NOT_FOUND', 'The authorization was never accepted.');
    }
  }
  const emitter = new Emitter(context, credential.secret, credential.revision);
  emitter.emit({ case: 'audit', value: audit });
  return emitter.events;
}

function recordLease(
  state: CredentialState | undefined,
  command: Extract<CredentialCommand, { case: 'recordLease' }>,
  context: DecisionContext,
): readonly CredentialEvent[] {
  const credential = requireCredential(state);
  const lease = create(SecretLeaseEventSchema, { fact: command.fact });
  const fact = lease.fact;
  const known = (id: string | undefined) => credential.leases.find((entry) => entry.id === id);
  if (fact.case === 'reserved') {
    requireLifecycle(credential, [CredentialLifecycle.ACTIVE]);
    if (!admitsLease(credential.purpose)) {
      throw new SecretFault('PURPOSE_MISMATCH', 'Only provider, tool and MCP authentication secrets are leased.');
    }
    if (fact.value.purpose !== credential.purpose) {
      throw new SecretFault('PURPOSE_MISMATCH', 'The lease purpose differs from the credential purpose.');
    }
  }
  if (fact.case === 'issued') {
    const entry = known(fact.value.lease?.id);
    if (!entry) throw new SecretFault('NOT_FOUND', 'The lease was never reserved.');
    const issued = timestampDate(fact.value.issuedAt!).getTime();
    const expires = timestampDate(fact.value.expiresAt!).getTime();
    const provider = timestampDate(fact.value.providerExpiresAt!).getTime();
    if (expires <= issued || expires - issued > MAX_AUTHORIZATION_SECONDS * SECOND_MS || expires > provider) {
      throw new SecretFault(
        'COMMAND_INVALID',
        `A lease lives at most ${MAX_AUTHORIZATION_SECONDS} seconds and not beyond the provider expiry.`,
      );
    }
  }
  if (fact.case === 'useEnded' || fact.case === 'cleanupConfirmed') {
    if (!known(fact.value.lease?.id)) throw new SecretFault('NOT_FOUND', 'The lease was never reserved.');
  }
  const emitter = new Emitter(context, credential.secret, credential.revision);
  emitter.emit({ case: 'lease', value: lease });
  return emitter.events;
}

function decideOrThrow(
  state: CredentialState | undefined,
  command: CredentialCommand,
  context: DecisionContext,
): readonly CredentialEvent[] {
  switch (command.case) {
    case 'createCredential':
      return createCredential(state, command, context);
    case 'rotateCredential':
      return rotateCredential(state, command, context);
    case 'completeVerification':
      return completeVerification(state, command, context);
    case 'revokeCredential':
      return revokeCredential(state, command, context);
    case 'scheduleDestruction':
      return scheduleDestruction(state, command, context);
    case 'confirmDestruction':
      return confirmDestruction(state, context);
    case 'recordAudit':
      return recordAudit(state, command, context);
    case 'recordLease':
      return recordLease(state, command, context);
    case 'renameCredential': {
      const credential = requireCredential(state);
      const displayName = validateDisplayName(command.displayName);
      if (displayName === credential.displayName) throw new SecretFault('COMMAND_INVALID', 'The name is unchanged.');
      const emitter = new Emitter(context, credential.secret, credential.revision);
      emitter.emit({ case: 'metadataUpdated', value: { displayName } });
      return emitter.events;
    }
    case 'disableCredential': {
      const credential = requireCredential(state);
      requireLifecycle(credential, [CredentialLifecycle.ACTIVE]);
      requireNoWrite(credential);
      const emitter = new Emitter(context, credential.secret, credential.revision);
      endOverlapIfOpen(emitter, credential, CredentialPolicyReason.OPERATOR_REQUESTED);
      changeLifecycle(emitter, CredentialLifecycle.DISABLED, CredentialPolicyReason.OPERATOR_REQUESTED);
      return emitter.events;
    }
    case 'enableCredential': {
      const credential = requireCredential(state);
      requireLifecycle(credential, [CredentialLifecycle.DISABLED], 'A revoked credential is never re-enabled.');
      const emitter = new Emitter(context, credential.secret, credential.revision);
      changeLifecycle(emitter, CredentialLifecycle.ACTIVE, CredentialPolicyReason.OPERATOR_REQUESTED);
      return emitter.events;
    }
    case 'markReconnectRequired': {
      const credential = requireCredential(state);
      requireLifecycle(credential, [CredentialLifecycle.ACTIVE]);
      const emitter = new Emitter(context, credential.secret, credential.revision);
      changeLifecycle(
        emitter,
        CredentialLifecycle.RECONNECT_REQUIRED,
        CredentialPolicyReason.PROVIDER_AUTHENTICATION_REJECTED,
      );
      return emitter.events;
    }
    case 'endWebhookOverlap': {
      const credential = requireCredential(state);
      if (!credential.overlap) throw new SecretFault('INVALID_LIFECYCLE', 'No webhook overlap window is open.');
      const emitter = new Emitter(context, credential.secret, credential.revision);
      endOverlapIfOpen(emitter, credential, command.reason ?? CredentialPolicyReason.OPERATOR_REQUESTED);
      return emitter.events;
    }
  }
}

export function decideCredential(
  state: CredentialState | undefined,
  command: CredentialCommand,
  context: DecisionContext,
): CredentialDecision {
  try {
    return { ok: true, events: decideOrThrow(state, command, context) };
  } catch (error) {
    if (error instanceof SecretFault) return { ok: false, fault: error };
    throw error;
  }
}
