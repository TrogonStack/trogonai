import { KeyBackendAdmission, KeyState, KeyUseDenialReason } from '@trogonai/proto';

import { keyUseDenial, type GatedOperation } from '@/features/custody/domain/admission';
import { denialReasonMessages } from '@/features/custody/domain/faults';
import { keyStateSummaries } from '@/features/custody/domain/key-state';

export type SourceCustody =
  | { readonly tier: 'managed' }
  | { readonly tier: 'dedicated' }
  | { readonly tier: 'customer_managed'; readonly keyState: KeyState; readonly admission: KeyBackendAdmission };

function useDenial(custody: SourceCustody, operation: GatedOperation): KeyUseDenialReason | undefined {
  return custody.tier === 'customer_managed' ? keyUseDenial(custody.admission, custody.keyState, operation) : undefined;
}

export function sourceKeyUnavailable(custody: SourceCustody, operation: GatedOperation): boolean {
  return useDenial(custody, operation) !== undefined;
}

export function failsClosed(custody: SourceCustody): boolean {
  return sourceKeyUnavailable(custody, 'unwrap');
}

export function failClosedReason(custody: SourceCustody): string | undefined {
  if (custody.tier !== 'customer_managed') return undefined;
  const denial = useDenial(custody, 'unwrap');
  if (denial === undefined) return undefined;
  return denial === KeyUseDenialReason.BACKEND_PAUSED
    ? denialReasonMessages[KeyUseDenialReason.BACKEND_PAUSED]
    : keyStateSummaries[custody.keyState];
}
