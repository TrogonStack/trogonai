import { KeyState } from '@trogonai/proto';

export const keyStateLabels: Record<KeyState, string> = {
  [KeyState.UNSPECIFIED]: 'Unknown',
  [KeyState.ACTIVE]: 'Active',
  [KeyState.DECRYPT_ONLY]: 'Decrypt only',
  [KeyState.DISABLED]: 'Disabled',
  [KeyState.PENDING_DESTRUCTION]: 'Pending destruction',
  [KeyState.DESTROYED]: 'Destroyed',
};

export const keyStateSummaries: Record<KeyState, string> = {
  [KeyState.UNSPECIFIED]: 'The platform has not observed this key.',
  [KeyState.ACTIVE]: 'Wraps new payloads and unwraps existing ones.',
  [KeyState.DECRYPT_ONLY]: 'Unwraps existing payloads and can be migrated away from, but wraps nothing new.',
  [KeyState.DISABLED]: 'Neither wraps nor unwraps. Use is suspended at the provider.',
  [KeyState.PENDING_DESTRUCTION]: 'Neither wraps nor unwraps while destruction is scheduled.',
  [KeyState.DESTROYED]: 'The key material is gone. Payloads wrapped under it cannot be read.',
};

export function canWrap(state: KeyState): boolean {
  return state === KeyState.ACTIVE;
}

export function canUnwrap(state: KeyState): boolean {
  return state === KeyState.ACTIVE || state === KeyState.DECRYPT_ONLY;
}

export function canBeDefault(state: KeyState): boolean {
  return canWrap(state);
}

export type StateTone = 'success' | 'warning' | 'danger';

export function keyStateTone(state: KeyState): StateTone {
  if (state === KeyState.ACTIVE) return 'success';
  if (state === KeyState.DECRYPT_ONLY) return 'warning';
  return 'danger';
}
