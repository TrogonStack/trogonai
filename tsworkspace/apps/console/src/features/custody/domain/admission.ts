import { KeyBackendAdmission, KeyBackendLifecycle, KeyState, KeyUseDenialReason } from '@trogonai/proto';

import { canUnwrap, canWrap } from './key-state';

export type GatedOperation = 'wrap' | 'unwrap';

export function registrationDenial(admission: KeyBackendAdmission): KeyUseDenialReason | undefined {
  return admission === KeyBackendAdmission.PAUSED ? KeyUseDenialReason.BACKEND_PAUSED : undefined;
}

export function keyUseDenial(
  admission: KeyBackendAdmission | undefined,
  state: KeyState,
  operation: GatedOperation,
): KeyUseDenialReason | undefined {
  const registration = admission === undefined ? undefined : registrationDenial(admission);
  if (registration !== undefined) return registration;
  const allowed = operation === 'wrap' ? canWrap(state) : canUnwrap(state);
  return allowed ? undefined : KeyUseDenialReason.KEY_STATE;
}

export function canPause(lifecycle: KeyBackendLifecycle, admission: KeyBackendAdmission): boolean {
  return lifecycle === KeyBackendLifecycle.ACTIVE && admission === KeyBackendAdmission.ENABLED;
}

export function canResume(lifecycle: KeyBackendLifecycle, admission: KeyBackendAdmission): boolean {
  return lifecycle === KeyBackendLifecycle.ACTIVE && admission === KeyBackendAdmission.PAUSED;
}

export const pauseCopy =
  'Pausing stops the platform from calling your provider. To revoke access, disable the key or revoke access in your provider.';
