import { SecretFailureReason } from '@trogonai/proto';

export type SecretFaultReason =
  | Exclude<keyof typeof SecretFailureReason, 'UNSPECIFIED'>
  | 'COMMAND_INVALID'
  | 'ENVIRONMENT_NOT_FOUND';

const messages: Record<SecretFaultReason, string> = {
  PERMISSION_DENIED: 'The caller is not permitted to perform this operation.',
  NOT_FOUND: 'The vault or credential does not exist.',
  REVISION_CONFLICT: 'The resource changed since it was loaded. Reload and try again.',
  PURPOSE_MISMATCH: 'The material kind or origin does not match the immutable purpose of the credential.',
  INVALID_MATERIAL: 'The submitted material was rejected.',
  PROVIDER_UNAVAILABLE: 'The provider could not be reached to verify the material.',
  AUDIT_UNAVAILABLE: 'The audit trail is unavailable, so the operation was refused.',
  MATERIAL_RESUBMISSION_REQUIRED: 'The candidate write was lost. Submit the material again.',
  VERSION_UNAVAILABLE: 'The requested version is not available.',
  RECONNECT_REQUIRED: 'The provider authorization is gone. Reconnect the credential.',
  INVALID_LIFECYCLE: 'The operation is not permitted in the current lifecycle.',
  OPERATION_IN_PROGRESS: 'Another write is already outstanding for this credential.',
  AUTHORIZATION_EXPIRED: 'The use authorization has expired.',
  BINDING_CHANGED: 'The binding changed since the authorization was issued.',
  IDEMPOTENCY_CONFLICT: 'The command id was already used for a different request.',
  OWNERSHIP_MISMATCH: 'The write is owned by a different provisioning operation.',
  VERIFICATION_REJECTED: 'The provider rejected the candidate material.',
  EFFECT_UNKNOWN: 'The outcome of the write could not be determined.',
  COMMAND_INVALID: 'The command is malformed.',
  ENVIRONMENT_NOT_FOUND: 'The environment is not registered for this project.',
};

export class SecretFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: SecretFaultReason,
    readonly detail?: string,
  ) {
    super(detail ? `${messages[reason]} ${detail}` : messages[reason]);
    this.name = 'SecretFault';
  }
}

export function isSecretFault(error: unknown): error is SecretFault {
  return error instanceof SecretFault;
}

export function faultMessage(reason: SecretFaultReason): string {
  return messages[reason];
}

export function failureReasonMessage(reason: SecretFailureReason): string {
  const name = SecretFailureReason[reason] as keyof typeof SecretFailureReason | undefined;
  return name && name !== 'UNSPECIFIED' && name in messages ? messages[name as SecretFaultReason] : 'The write failed.';
}
