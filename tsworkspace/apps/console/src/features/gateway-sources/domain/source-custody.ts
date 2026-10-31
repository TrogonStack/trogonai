import { KeyBackendAdmission, KeyState, KeyUseDenialReason } from '@trogonai/proto';

import { keyUseDenial } from '@/features/custody/domain/admission';
import { denialReasonMessages } from '@/features/custody/domain/faults';
import { keyStateSummaries } from '@/features/custody/domain/key-state';

export type SourceCustody =
  | { readonly tier: 'managed' }
  | { readonly tier: 'dedicated' }
  | { readonly tier: 'customer_managed'; readonly keyState: KeyState; readonly admission: KeyBackendAdmission };

function unwrapDenial(custody: SourceCustody): KeyUseDenialReason | undefined {
  return custody.tier === 'customer_managed' ? keyUseDenial(custody.admission, custody.keyState, 'unwrap') : undefined;
}

export function failsClosed(custody: SourceCustody): boolean {
  return unwrapDenial(custody) !== undefined;
}

export function failClosedReason(custody: SourceCustody): string | undefined {
  if (custody.tier !== 'customer_managed') return undefined;
  const denial = unwrapDenial(custody);
  if (denial === undefined) return undefined;
  return denial === KeyUseDenialReason.BACKEND_PAUSED
    ? denialReasonMessages[KeyUseDenialReason.BACKEND_PAUSED]
    : keyStateSummaries[custody.keyState];
}
