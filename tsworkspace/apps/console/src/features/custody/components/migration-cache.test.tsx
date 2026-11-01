import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { expect, it } from 'vitest';

import { TenantCustodyClientProvider } from '../context';
import { purposeId } from '../domain/identifiers';
import { toCustodyView, type CustodyView } from '../domain/view';
import { createFakeTenantCustodyClient } from '../fake';
import { custodyKeys, useStartMigration } from '../queries';

it('makes the new migration available in cache before reporting success', async () => {
  const client = createFakeTenantCustodyClient({ latencyMs: 0 });
  const queryClient = new QueryClient();
  queryClient.setQueryData(custodyKeys.state, toCustodyView(await client.load()));
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>
      <TenantCustodyClientProvider client={client}>{children}</TenantCustodyClientProvider>
    </QueryClientProvider>
  );
  const { result, unmount } = renderHook(() => useStartMigration(), { wrapper });
  await act(async () => {
    const started = await result.current.mutateAsync({
      purpose: purposeId('session_artifacts'),
      target: { tier: 'managed' },
    });
    expect(started.migration).toBeDefined();
    const cached = queryClient.getQueryData<CustodyView>(custodyKeys.state);
    expect(cached?.migrations.some((entry) => entry.migration.id === started.migration?.id)).toBe(true);
  });
  unmount();
  queryClient.clear();
});
