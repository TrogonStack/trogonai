import { KeyUseDenialReason, OperationFailureCategory, type OperationFailure } from '@trogonai/proto';

import { denialReasonMessages } from './faults';

export const failureCategories = [
  OperationFailureCategory.KEY_USE_DENIED,
  OperationFailureCategory.PROVIDER_UNAVAILABLE,
  OperationFailureCategory.PROVIDER_THROTTLED,
  OperationFailureCategory.BINDING_MISMATCH,
  OperationFailureCategory.TRANSPORT_INTEGRITY_MISMATCH,
  OperationFailureCategory.INTEGRITY_FAILURE,
  OperationFailureCategory.UNSUPPORTED_CAPABILITY,
  OperationFailureCategory.INTERNAL_FAILURE,
] as const;

export type FailureTone = 'danger' | 'warning' | 'neutral';

export interface FailureSemantics {
  readonly label: string;
  readonly summary: string;
  readonly retry: string;
  readonly tone: FailureTone;
}

export const failureSemantics: Record<OperationFailureCategory, FailureSemantics> = {
  [OperationFailureCategory.UNSPECIFIED]: {
    label: 'Unknown',
    summary: 'The failure category was not recorded.',
    retry: 'Not known.',
    tone: 'neutral',
  },
  [OperationFailureCategory.KEY_USE_DENIED]: {
    label: 'Key use denied',
    summary: 'The key may not be used: the provider denied it, the backend is paused, or the key state forbids it.',
    retry: 'Never retried automatically. An owner action must change first.',
    tone: 'danger',
  },
  [OperationFailureCategory.PROVIDER_UNAVAILABLE]: {
    label: 'Provider unavailable',
    summary: 'Transport failure, timeout or a provider service failure.',
    retry: 'Bounded backoff within the operation deadline.',
    tone: 'warning',
  },
  [OperationFailureCategory.PROVIDER_THROTTLED]: {
    label: 'Provider throttled',
    summary: 'The provider reported a quota or rate limit.',
    retry: 'Bounded backoff directed by the provider.',
    tone: 'warning',
  },
  [OperationFailureCategory.BINDING_MISMATCH]: {
    label: 'Binding mismatch',
    summary: 'The key reference belongs to another tenant or purpose.',
    retry: 'Never retried.',
    tone: 'danger',
  },
  [OperationFailureCategory.TRANSPORT_INTEGRITY_MISMATCH]: {
    label: 'Transport integrity mismatch',
    summary: 'A provider request or response checksum did not match.',
    retry: 'Limited retry against the same provider. A persistent mismatch is terminal.',
    tone: 'danger',
  },
  [OperationFailureCategory.INTEGRITY_FAILURE]: {
    label: 'Integrity failure',
    summary: 'Authenticated data or ciphertext validation failed.',
    retry: 'Never retried.',
    tone: 'danger',
  },
  [OperationFailureCategory.UNSUPPORTED_CAPABILITY]: {
    label: 'Unsupported capability',
    summary: 'The bound key lacks the requested capability.',
    retry: 'Never retried.',
    tone: 'neutral',
  },
  [OperationFailureCategory.INTERNAL_FAILURE]: {
    label: 'Internal failure',
    summary: 'An unmapped provider error, malformed response or adapter fault.',
    retry: 'Never retried.',
    tone: 'neutral',
  },
};

export const denialReasonLabels: Record<KeyUseDenialReason, string> = {
  [KeyUseDenialReason.UNSPECIFIED]: 'Unspecified',
  [KeyUseDenialReason.PROVIDER_DENIED]: 'Provider denied',
  [KeyUseDenialReason.BACKEND_PAUSED]: 'Backend paused',
  [KeyUseDenialReason.KEY_STATE]: 'Key state',
  [KeyUseDenialReason.BACKEND_NOT_ACTIVE]: 'Backend not active',
};

export function failureLabel(failure: OperationFailure): string {
  const category = failureSemantics[failure.category].label;
  return failure.category === OperationFailureCategory.KEY_USE_DENIED
    ? `${category}: ${denialReasonLabels[failure.denialReason]}`
    : category;
}

export function failureExplanation(failure: OperationFailure): string {
  return failure.category === OperationFailureCategory.KEY_USE_DENIED
    ? denialReasonMessages[failure.denialReason]
    : failureSemantics[failure.category].summary;
}
