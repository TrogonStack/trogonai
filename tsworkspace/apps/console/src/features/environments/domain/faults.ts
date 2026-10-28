export type EnvironmentFaultReason =
  | 'COMMAND_INVALID'
  | 'ENVIRONMENT_ALREADY_EXISTS'
  | 'ENVIRONMENT_NOT_FOUND'
  | 'ENVIRONMENT_BUILT_IN'
  | 'ENVIRONMENT_IN_USE'
  | 'REVISION_CONFLICT'
  | 'IDEMPOTENCY_CONFLICT';

const messages: Record<EnvironmentFaultReason, string> = {
  COMMAND_INVALID: 'The command is malformed.',
  ENVIRONMENT_ALREADY_EXISTS: 'An environment with this id already exists.',
  ENVIRONMENT_NOT_FOUND: 'The environment does not exist.',
  ENVIRONMENT_BUILT_IN: 'A built-in environment cannot be deleted.',
  ENVIRONMENT_IN_USE: 'Vaults still use this environment, and a vault environment never changes.',
  REVISION_CONFLICT: 'The environment changed since it was loaded. Reload and try again.',
  IDEMPOTENCY_CONFLICT: 'The command id was already used for a different request.',
};

export class EnvironmentFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: EnvironmentFaultReason,
    readonly detail?: string,
  ) {
    super(detail ? `${messages[reason]} ${detail}` : messages[reason]);
    this.name = 'EnvironmentFault';
  }
}

export function isEnvironmentFault(error: unknown): error is EnvironmentFault {
  return error instanceof EnvironmentFault;
}
