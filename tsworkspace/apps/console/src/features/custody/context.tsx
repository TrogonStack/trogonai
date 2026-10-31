import { createContext, useContext, type ReactNode } from 'react';

import type { TenantCustodyClient } from './client';

const TenantCustodyClientContext = createContext<TenantCustodyClient | undefined>(undefined);

export function TenantCustodyClientProvider({
  client,
  children,
}: {
  client: TenantCustodyClient;
  children: ReactNode;
}) {
  return <TenantCustodyClientContext value={client}>{children}</TenantCustodyClientContext>;
}

export function useTenantCustodyClient(): TenantCustodyClient {
  const client = useContext(TenantCustodyClientContext);
  if (!client) throw new Error('TenantCustodyClientProvider is missing from the component tree.');
  return client;
}
