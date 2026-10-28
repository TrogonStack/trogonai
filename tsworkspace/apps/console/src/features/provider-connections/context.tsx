import { createContext, useContext, type ReactNode } from 'react';

import type { ProviderConnectionsClient } from './client';

const ProviderConnectionsClientContext = createContext<ProviderConnectionsClient | undefined>(undefined);

export function ProviderConnectionsClientProvider({
  client,
  children,
}: {
  client: ProviderConnectionsClient;
  children: ReactNode;
}) {
  return <ProviderConnectionsClientContext value={client}>{children}</ProviderConnectionsClientContext>;
}

export function useProviderConnectionsClient(): ProviderConnectionsClient {
  const client = useContext(ProviderConnectionsClientContext);
  if (!client) throw new Error('ProviderConnectionsClientProvider is missing from the component tree.');
  return client;
}
