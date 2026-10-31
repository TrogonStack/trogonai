import { KeyState } from '@trogonai/proto';

import { canUnwrap, keyStateSummaries } from '@/features/custody/domain/key-state';

export type SourceCustody =
  | { readonly tier: 'managed' }
  | { readonly tier: 'dedicated' }
  | { readonly tier: 'customer_managed'; readonly keyState: KeyState };

export function failsClosed(custody: SourceCustody): boolean {
  return custody.tier === 'customer_managed' && !canUnwrap(custody.keyState);
}

export function failClosedReason(custody: SourceCustody): string | undefined {
  return custody.tier === 'customer_managed' && failsClosed(custody) ? keyStateSummaries[custody.keyState] : undefined;
}
