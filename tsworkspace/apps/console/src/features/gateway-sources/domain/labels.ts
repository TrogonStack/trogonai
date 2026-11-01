import type { LedTone } from '@/features/secrets/domain/labels';

import type { SlotState } from './integration';
import type { IntegrationStatus, RegistrationStatus } from './view';

export const statusLabels: Record<IntegrationStatus, string> = {
  receiving: 'Receiving',
  degraded: 'Degraded',
  paused: 'Paused',
  misconfigured: 'Misconfigured',
};

export const statusTones: Record<IntegrationStatus, LedTone> = {
  receiving: 'success',
  degraded: 'warning',
  paused: 'neutral',
  misconfigured: 'danger',
};

export const slotStateLabels: Record<SlotState, string> = {
  active: 'Active',
  pending_write: 'Pending write',
  pending_rotation: 'Pending rotation',
  revoked: 'Revoked',
  write_failed: 'Write failed',
  cleanup_failed: 'Cleanup failed',
  destroyed: 'Destroyed',
};

export const slotStateTones: Record<SlotState, LedTone> = {
  active: 'success',
  pending_write: 'warning',
  pending_rotation: 'warning',
  revoked: 'danger',
  write_failed: 'danger',
  cleanup_failed: 'warning',
  destroyed: 'neutral',
};

export const registrationLabels: Record<RegistrationStatus, string> = {
  registered: 'Registered',
  registration_pending: 'Registration pending',
  registration_failed: 'Registration failed',
  not_applicable: 'Not applicable',
};

export const registrationTones: Record<RegistrationStatus, LedTone> = {
  registered: 'success',
  registration_pending: 'warning',
  registration_failed: 'danger',
  not_applicable: 'neutral',
};

export const resolutionLabels = {
  runtime: 'Runtime',
  static: 'Static',
} as const;

export const resolutionDescriptions = {
  runtime: 'Resolved from the credential platform at delivery time.',
  static: 'Read from the gateway environment at startup.',
} as const;
