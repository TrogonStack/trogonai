import { KeyUseDenialReason } from '@trogonai/proto';

export type KeyFaultReason =
  | 'COMMAND_INVALID'
  | 'BACKEND_NOT_FOUND'
  | 'KEY_NOT_FOUND'
  | 'MIGRATION_NOT_FOUND'
  | 'PURPOSE_NOT_FOUND'
  | 'ATTEMPT_NOT_FOUND'
  | 'CHECK_NOT_FOUND'
  | 'VALIDATION_INCOMPLETE'
  | 'VALIDATION_FAILED'
  | 'RESUME_VALIDATION_FAILED'
  | 'BACKEND_ALREADY_REGISTERED'
  | 'BACKEND_NOT_ACTIVE'
  | 'BACKEND_ALREADY_PAUSED'
  | 'BACKEND_NOT_PAUSED'
  | 'KEY_ALREADY_BOUND'
  | 'KEY_USE_DENIED'
  | 'ALREADY_DEFAULT'
  | 'PURPOSE_MISMATCH'
  | 'SAME_CUSTODY'
  | 'NOTHING_TO_MIGRATE'
  | 'MIGRATION_IN_FLIGHT'
  | 'NOT_RUNNING'
  | 'NOT_PAUSED'
  | 'ALREADY_COMPLETED';

export const keyFaultMessages: Record<KeyFaultReason, string> = {
  COMMAND_INVALID: 'The command is malformed.',
  BACKEND_NOT_FOUND: 'The key backend does not exist.',
  KEY_NOT_FOUND: 'The key reference does not exist.',
  MIGRATION_NOT_FOUND: 'The migration does not exist.',
  PURPOSE_NOT_FOUND: 'No service declares that key purpose.',
  ATTEMPT_NOT_FOUND: 'The setup attempt does not exist or has expired.',
  CHECK_NOT_FOUND: 'The validation check does not exist.',
  VALIDATION_INCOMPLETE: 'Every validation check must run before the setup can finish.',
  VALIDATION_FAILED: 'A validation check failed, so nothing was retained.',
  RESUME_VALIDATION_FAILED: 'The registration did not validate, so it stays paused.',
  BACKEND_ALREADY_REGISTERED: 'A backend for this boundary is already registered.',
  BACKEND_NOT_ACTIVE: 'The operation requires an active registration.',
  BACKEND_ALREADY_PAUSED: 'The registration is already paused.',
  BACKEND_NOT_PAUSED: 'Resume requires a paused registration.',
  KEY_ALREADY_BOUND: 'That provider key is already bound to a key reference. A binding is immutable and unique.',
  KEY_USE_DENIED: 'The platform may not use this key.',
  ALREADY_DEFAULT: 'This key is already the default for its purpose.',
  PURPOSE_MISMATCH: 'The key was bound for a different purpose.',
  SAME_CUSTODY: 'That purpose already uses this custody.',
  NOTHING_TO_MIGRATE: 'Nothing is wrapped under the source, so there is nothing to rewrap.',
  MIGRATION_IN_FLIGHT: 'A migration is already moving payloads for this purpose.',
  NOT_RUNNING: 'The migration is not running.',
  NOT_PAUSED: 'The migration is not paused.',
  ALREADY_COMPLETED: 'The migration already completed.',
};

export const denialReasonMessages: Record<KeyUseDenialReason, string> = {
  [KeyUseDenialReason.UNSPECIFIED]: 'The platform may not use this key.',
  [KeyUseDenialReason.PROVIDER_DENIED]: 'The provider denied the request.',
  [KeyUseDenialReason.BACKEND_PAUSED]: 'The backend is paused. Resume it before the platform can use its keys.',
  [KeyUseDenialReason.KEY_STATE]: 'The key state does not allow this operation.',
};

export class KeyFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: KeyFaultReason,
    readonly detail?: string,
    readonly denial?: KeyUseDenialReason,
  ) {
    const base =
      reason === 'KEY_USE_DENIED' && denial !== undefined ? denialReasonMessages[denial] : keyFaultMessages[reason];
    super(detail ? `${base} ${detail}` : base);
    this.name = 'KeyFault';
  }
}

export function keyUseDenied(denial: KeyUseDenialReason, detail?: string): KeyFault {
  return new KeyFault('KEY_USE_DENIED', detail, denial);
}

export function isKeyFault(error: unknown): error is KeyFault {
  return error instanceof KeyFault;
}
