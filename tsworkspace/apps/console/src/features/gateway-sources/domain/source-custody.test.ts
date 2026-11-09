import { KeyBackendAdmission, KeyState, KeyUseDenialReason } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { denialReasonMessages } from '@/features/custody/domain/faults';
import { keyStateSummaries } from '@/features/custody/domain/key-state';

import { failClosedReason, failsClosed, type SourceCustody } from './source-custody';

function customerManaged(keyState: KeyState, admission: KeyBackendAdmission): SourceCustody {
  return { tier: 'customer_managed', keyState, admission };
}

describe('source custody', () => {
  it('stays open for platform tiers', () => {
    expect(failsClosed({ tier: 'managed' })).toBe(false);
    expect(failsClosed({ tier: 'dedicated' })).toBe(false);
    expect(failClosedReason({ tier: 'managed' })).toBeUndefined();
  });

  it('stays open when the backend is enabled and the key can unwrap', () => {
    const custody = customerManaged(KeyState.ACTIVE, KeyBackendAdmission.ENABLED);
    expect(failsClosed(custody)).toBe(false);
    expect(failClosedReason(custody)).toBeUndefined();
  });

  it('fails closed on a paused backend even when the key is usable', () => {
    const custody = customerManaged(KeyState.ACTIVE, KeyBackendAdmission.PAUSED);
    expect(failsClosed(custody)).toBe(true);
    expect(failClosedReason(custody)).toBe(denialReasonMessages[KeyUseDenialReason.BACKEND_PAUSED]);
  });

  it('fails closed on a key that cannot unwrap and explains the key state', () => {
    const custody = customerManaged(KeyState.DISABLED, KeyBackendAdmission.ENABLED);
    expect(failsClosed(custody)).toBe(true);
    expect(failClosedReason(custody)).toBe(keyStateSummaries[KeyState.DISABLED]);
  });

  it('reports the pause first when both the backend is paused and the key cannot unwrap', () => {
    const custody = customerManaged(KeyState.DISABLED, KeyBackendAdmission.PAUSED);
    expect(failClosedReason(custody)).toBe(denialReasonMessages[KeyUseDenialReason.BACKEND_PAUSED]);
  });
});
