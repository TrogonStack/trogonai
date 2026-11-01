import { KeyBackendFailureReason, KeyProvider, type ProviderKeyLocator } from '@trogonai/proto';

import { locatorText } from '../domain/locator';
import {
  locatorBoundaryMismatch,
  MIN_OPENBAO_VERSION,
  versionAtLeast,
  type RegistrationInput,
} from '../domain/registration';
import type { BackendRecord } from '../domain/state';
import type { CheckResult } from '../domain/validation';

const pass = (detail: string): CheckResult => ({ checkId: '', outcome: 'pass', detail });
const fail = (detail: string, reason?: KeyBackendFailureReason): CheckResult => ({
  checkId: '',
  outcome: 'fail',
  detail,
  ...(reason === undefined ? {} : { reason }),
});

export const OPEN_TRUST_ROLE_MARKER = 'no-external-id';
export const DISABLED_KEY_MARKER = 'disabled';

export function evaluateRegistrationCheck(input: RegistrationInput, checkId: string, priorRuns: number): CheckResult {
  return { ...evaluate(input, checkId, priorRuns), checkId };
}

function evaluate(input: RegistrationInput, checkId: string, priorRuns: number): CheckResult {
  if (input.provider === KeyProvider.AWS_KMS) {
    switch (checkId) {
      case 'assume_role':
        return pass('A session was issued for 15 minutes with the external ID.');
      case 'missing_external_id_denied':
        return input.roleArn.includes(OPEN_TRUST_ROLE_MARKER)
          ? fail(
              'The role was assumed without an external ID. Any party that can name the platform principal could use this role, which is the confused deputy risk. Add the sts:ExternalId condition to the trust policy.',
              KeyBackendFailureReason.ACCESS_DENIED,
            )
          : pass('AccessDenied, as required.');
      case 'wrong_external_id_denied':
        return input.roleArn.includes(OPEN_TRUST_ROLE_MARKER)
          ? fail('The role was assumed with an incorrect external ID.', KeyBackendFailureReason.ACCESS_DENIED)
          : pass('AccessDenied, as required.');
      default:
        return pass(`KMS in ${input.region} answered.`);
    }
  }
  if (input.provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    switch (checkId) {
      case 'token_exchange':
        return pass('A federated access token was issued.');
      case 'attribute_conditions':
        return pass('A token for a different subject was rejected.');
      case 'impersonation':
        return pass('Only the named service account could be impersonated.');
      default:
        return pass('Cloud KMS answered.');
    }
  }
  switch (checkId) {
    case 'tls_trust':
      return pass('The certificate chain verified against the trust root you supplied.');
    case 'authentication':
      return pass('A token was issued with a 10 minute lifetime.');
    case 'version': {
      const version = input.version.trim();
      return versionAtLeast(version, MIN_OPENBAO_VERSION)
        ? pass(`OpenBao ${version} is at least ${MIN_OPENBAO_VERSION}.`)
        : fail(
            `OpenBao ${version} is older than ${MIN_OPENBAO_VERSION}, which the platform requires.`,
            KeyBackendFailureReason.CAPABILITY_MISSING,
          );
    }
    case 'disable_upsert':
      return priorRuns === 0
        ? fail(
            `disable_upsert is not enabled on the ${input.transitMount} mount. An encrypt call against a deleted key would create new key material, turning a deletion into a silent replacement. Run the command in the setup instructions, then run the checks again.`,
            KeyBackendFailureReason.CAPABILITY_MISSING,
          )
        : pass('An encrypt call against a missing key was rejected.');
    default:
      return pass('A wrap and unwrap round trip succeeded.');
  }
}

export function evaluateBindingCheck(
  backend: BackendRecord,
  locator: ProviderKeyLocator,
  checkId: string,
): CheckResult {
  return { ...bindingOutcome(backend, locator, checkId), checkId };
}

function bindingOutcome(backend: BackendRecord, locator: ProviderKeyLocator, checkId: string): CheckResult {
  switch (checkId) {
    case 'identity': {
      const mismatch = locatorBoundaryMismatch(backend.registration.configuration?.access, locator);
      return mismatch ? fail(mismatch) : pass('The key is inside the boundary this backend was registered for.');
    }
    case 'access': {
      if (backend.registration.health?.status.case === 'failed') {
        return fail(
          'The platform could not reach the provider, so it could not use the key. Check the connection status first.',
          backend.registration.health.status.value.reason,
        );
      }
      return locatorText(locator).toLowerCase().includes(DISABLED_KEY_MARKER)
        ? fail('The key is disabled at the provider, so the platform cannot use it.')
        : pass('The key is enabled and the platform may use it.');
    }
    default:
      return pass('Wrapping and unwrapping a test data key returned the original bytes.');
  }
}
