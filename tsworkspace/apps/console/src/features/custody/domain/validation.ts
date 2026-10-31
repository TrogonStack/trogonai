import { KeyProvider, type KeyBackendFailureReason } from '@trogonai/proto';

import type { AttemptId, ExternalId, KeyBackendId } from './identifiers';
import type { RegistrationInput } from './registration';

export interface CheckDefinition {
  readonly id: string;
  readonly label: string;
}

export type CheckOutcome = 'pass' | 'fail' | 'skipped';

export interface CheckResult {
  readonly checkId: string;
  readonly outcome: CheckOutcome;
  readonly detail: string;
  readonly reason?: KeyBackendFailureReason;
}

export type InstructionLanguage = 'json' | 'bash' | 'hcl';

export interface SetupInstruction {
  readonly id: string;
  readonly title: string;
  readonly description: string;
  readonly language: InstructionLanguage;
  readonly content: string;
}

export interface RegistrationPlan {
  readonly attemptId: AttemptId;
  readonly input: RegistrationInput;
  readonly externalId?: ExternalId;
  readonly instructions: readonly SetupInstruction[];
  readonly checks: readonly CheckDefinition[];
}

export interface BindingPlan {
  readonly attemptId: AttemptId;
  readonly backendId: KeyBackendId;
  readonly checks: readonly CheckDefinition[];
}

export const bindingChecks: readonly CheckDefinition[] = [
  { id: 'identity', label: 'Identity: the key lives inside the registered boundary' },
  { id: 'access', label: 'Access: the platform can use exactly this key' },
  { id: 'round_trip', label: 'Authenticated-data round trip: wrap then unwrap returns the data key' },
];

export function registrationChecks(
  provider: RegistrationInput['provider'],
  impersonating: boolean,
): readonly CheckDefinition[] {
  if (provider === KeyProvider.AWS_KMS) {
    return [
      { id: 'assume_role', label: 'AssumeRole with the platform external ID succeeds' },
      { id: 'missing_external_id_denied', label: 'AssumeRole without an external ID is denied' },
      { id: 'wrong_external_id_denied', label: 'AssumeRole with an incorrect external ID is denied' },
      { id: 'kms_reachable', label: 'The assumed session can reach KMS in the chosen region' },
    ];
  }
  if (provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    return [
      { id: 'token_exchange', label: 'The federated token exchange succeeds' },
      { id: 'attribute_conditions', label: 'Attribute conditions admit only the expected subject' },
      ...(impersonating
        ? [{ id: 'impersonation', label: 'The federated principal can impersonate only the service account' }]
        : []),
      { id: 'kms_reachable', label: 'The credentials can reach Cloud KMS' },
    ];
  }
  return [
    { id: 'tls_trust', label: 'TLS verifies against the explicit trust root' },
    { id: 'authentication', label: 'The platform authenticates with a short-lived token' },
    { id: 'version', label: 'OpenBao is version 2.6 or newer' },
    { id: 'disable_upsert', label: 'The Transit mount has disable_upsert enabled' },
    { id: 'transit_reachable', label: 'The Transit mount is reachable with use-only permissions' },
  ];
}
