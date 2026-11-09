import { getOption } from '@bufbuild/protobuf';
import type { DescMessage } from '@bufbuild/protobuf';
import {
  CredentialBindingIdentityReusedErrorSchema,
  CredentialBindingInUseErrorSchema,
  CredentialBindingNotActiveErrorSchema,
  CredentialBindingNotFoundErrorSchema,
  CredentialBindingRevokedErrorSchema,
  CredentialDrainNotPermittedErrorSchema,
  ProviderAuthenticationUnsupportedErrorSchema,
  ProviderCommandInvalidErrorSchema,
  ProviderConnectionAlreadyExistsErrorSchema,
  ProviderConnectionNotActiveErrorSchema,
  ProviderConnectionNotFoundErrorSchema,
  ProviderConnectionNotPendingErrorSchema,
  ProviderConnectionRevokedErrorSchema,
  ProviderModelMismatchErrorSchema,
  ProviderSubjectMismatchErrorSchema,
  ProviderValidationRejectedErrorSchema,
  errorTemplateOption,
} from '@trogonai/proto';

const faultSchemas = {
  PROVIDER_COMMAND_INVALID: ProviderCommandInvalidErrorSchema,
  PROVIDER_CONNECTION_ALREADY_EXISTS: ProviderConnectionAlreadyExistsErrorSchema,
  PROVIDER_CONNECTION_NOT_FOUND: ProviderConnectionNotFoundErrorSchema,
  PROVIDER_CONNECTION_NOT_PENDING: ProviderConnectionNotPendingErrorSchema,
  PROVIDER_CONNECTION_NOT_ACTIVE: ProviderConnectionNotActiveErrorSchema,
  PROVIDER_CONNECTION_REVOKED: ProviderConnectionRevokedErrorSchema,
  CREDENTIAL_BINDING_NOT_FOUND: CredentialBindingNotFoundErrorSchema,
  CREDENTIAL_BINDING_IDENTITY_REUSED: CredentialBindingIdentityReusedErrorSchema,
  CREDENTIAL_BINDING_NOT_ACTIVE: CredentialBindingNotActiveErrorSchema,
  CREDENTIAL_BINDING_REVOKED: CredentialBindingRevokedErrorSchema,
  CREDENTIAL_BINDING_IN_USE: CredentialBindingInUseErrorSchema,
  PROVIDER_VALIDATION_REJECTED: ProviderValidationRejectedErrorSchema,
  PROVIDER_AUTHENTICATION_UNSUPPORTED: ProviderAuthenticationUnsupportedErrorSchema,
  PROVIDER_MODEL_MISMATCH: ProviderModelMismatchErrorSchema,
  PROVIDER_SUBJECT_MISMATCH: ProviderSubjectMismatchErrorSchema,
  CREDENTIAL_DRAIN_NOT_PERMITTED: CredentialDrainNotPermittedErrorSchema,
} as const satisfies Record<string, DescMessage>;

export type ProviderFaultReason = keyof typeof faultSchemas;

export type ConcurrencyFaultReason = 'STREAM_ALREADY_EXISTS' | 'REVISION_MISMATCH' | 'EXPECTED_REVISION_REQUIRED';

export interface Violation {
  readonly path: string;
  readonly constraint: string;
  readonly message: string;
}

export class ProviderConnectionFault extends Error {
  readonly kind = 'domain' as const;

  constructor(
    readonly reason: ProviderFaultReason,
    readonly detail?: string,
    readonly violations: readonly Violation[] = [],
  ) {
    super(detail ? `${faultMessage(reason)} ${detail}` : faultMessage(reason));
    this.name = 'ProviderConnectionFault';
  }
}

export class ConcurrencyFault extends Error {
  readonly kind = 'concurrency' as const;

  constructor(
    readonly reason: ConcurrencyFaultReason,
    message: string,
  ) {
    super(message);
    this.name = 'ConcurrencyFault';
  }
}

export type DecisionFault = ProviderConnectionFault | ConcurrencyFault;

export function faultMessage(reason: ProviderFaultReason): string {
  return getOption(faultSchemas[reason], errorTemplateOption).template?.message ?? reason;
}

export function isDecisionFault(error: unknown): error is DecisionFault {
  return error instanceof ProviderConnectionFault || error instanceof ConcurrencyFault;
}

export function invalid(violations: readonly Violation[]): ProviderConnectionFault {
  const detail = violations.map((violation) => violation.message).join('; ');
  return new ProviderConnectionFault('PROVIDER_COMMAND_INVALID', detail, violations);
}
