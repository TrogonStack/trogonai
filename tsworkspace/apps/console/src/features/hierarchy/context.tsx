import { createContext, useContext, type ReactNode } from 'react';

import type { HierarchyClient } from './client';

const HierarchyClientContext = createContext<HierarchyClient | undefined>(undefined);

export function HierarchyClientProvider({ client, children }: { client: HierarchyClient; children: ReactNode }) {
  return <HierarchyClientContext value={client}>{children}</HierarchyClientContext>;
}

export function useHierarchyClient(): HierarchyClient {
  const client = useContext(HierarchyClientContext);
  if (!client) throw new Error('HierarchyClientProvider is missing from the component tree.');
  return client;
}
