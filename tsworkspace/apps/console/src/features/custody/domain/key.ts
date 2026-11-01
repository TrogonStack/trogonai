import type { CustomerManagedKeyBindingDetail, KeyCapability, KeyStateObservation } from '@trogonai/proto';

import type { KeyRef, PurposeId } from './identifiers';

export interface DailyUsage {
  readonly wraps: number;
  readonly unwraps: number;
}

export const USAGE_DAYS = 7;

export type KeyBinding =
  | { readonly kind: 'dedicated' }
  | { readonly kind: 'customer_managed'; readonly detail: CustomerManagedKeyBindingDetail };

export interface TenantKey {
  readonly keyRef: KeyRef;
  readonly purpose: PurposeId;
  readonly state: KeyStateObservation;
  readonly capabilities: readonly KeyCapability[];
  readonly boundAt: Date;
  readonly usage: readonly DailyUsage[];
  readonly binding: KeyBinding;
}

export function emptyUsage(): readonly DailyUsage[] {
  return Array.from({ length: USAGE_DAYS }, () => ({ wraps: 0, unwraps: 0 }));
}
