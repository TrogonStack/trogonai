export type SourceFaultReason =
  | 'COMMAND_INVALID'
  | 'INTEGRATION_NOT_FOUND'
  | 'REVISION_CONFLICT'
  | 'ALREADY_PAUSED'
  | 'NOT_PAUSED'
  | 'SLOT_NOT_FOUND'
  | 'SLOT_NOT_ROTATABLE'
  | 'REGISTRATION_NOT_APPLICABLE'
  | 'OPERATION_IN_PROGRESS'
  | 'KEY_UNAVAILABLE';

const messages: Record<SourceFaultReason, string> = {
  COMMAND_INVALID: 'The command is malformed.',
  INTEGRATION_NOT_FOUND: 'The source integration does not exist.',
  REVISION_CONFLICT: 'The integration changed since it was loaded. Reload and try again.',
  ALREADY_PAUSED: 'The integration is already paused.',
  NOT_PAUSED: 'The integration is not paused.',
  SLOT_NOT_FOUND: 'The integration has no such credential slot.',
  SLOT_NOT_ROTATABLE: 'Only a runtime credential that is not revoked or destroyed can be rotated.',
  REGISTRATION_NOT_APPLICABLE: 'This source does not register a webhook with the provider.',
  OPERATION_IN_PROGRESS: 'Another change is still settling for this integration.',
  KEY_UNAVAILABLE:
    'The customer managed key cannot be used, so the platform fails closed and cannot use these credentials.',
};

export class SourceFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: SourceFaultReason,
    readonly detail?: string,
  ) {
    super(detail ? `${messages[reason]} ${detail}` : messages[reason]);
    this.name = 'SourceFault';
  }
}

export function isSourceFault(error: unknown): error is SourceFault {
  return error instanceof SourceFault;
}

export function faultMessage(reason: SourceFaultReason): string {
  return messages[reason];
}
