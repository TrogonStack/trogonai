import { MaterialVerificationOutcome, SecretMaterialKind, type SecretPurpose } from '@trogonai/proto';

import type { MaterialInput, VerificationSimulation } from './domain/commands';
import { admitsGeneration, requiresExternalVerification } from './domain/policy';

export type MaterialOrigin = 'imported' | 'generated';

export interface MaterialDraft {
  readonly origin: MaterialOrigin;
  readonly material: string;
  readonly simulation: string;
}

export const PENDING_SIMULATION = 'PENDING';

export const initialMaterialDraft: MaterialDraft = {
  origin: 'imported',
  material: '',
  simulation: String(MaterialVerificationOutcome.PASSED),
};

export function effectiveOrigin(draft: MaterialDraft, purpose: SecretPurpose): MaterialOrigin {
  return draft.origin === 'generated' && admitsGeneration(purpose) ? 'generated' : 'imported';
}

export function toMaterialInput(draft: MaterialDraft, purpose: SecretPurpose): MaterialInput {
  return effectiveOrigin(draft, purpose) === 'generated'
    ? { case: 'generated' }
    : { case: 'imported', material: draft.material };
}

export function toSimulation(draft: MaterialDraft, purpose: SecretPurpose): VerificationSimulation {
  if (!requiresExternalVerification(purpose)) return MaterialVerificationOutcome.PASSED;
  return draft.simulation === PENDING_SIMULATION
    ? PENDING_SIMULATION
    : (Number(draft.simulation) as MaterialVerificationOutcome);
}

export function materialError(
  draft: MaterialDraft,
  purpose: SecretPurpose,
  kind: SecretMaterialKind,
): string | undefined {
  if (effectiveOrigin(draft, purpose) === 'generated') return undefined;
  const value = draft.material.trim();
  if (value === '') return 'The secret material is required.';
  if (kind === SecretMaterialKind.WEBHOOK_DESTINATION && !/^https:\/\/\S+$/i.test(value)) {
    return 'A webhook destination must be an https URL.';
  }
  return undefined;
}
