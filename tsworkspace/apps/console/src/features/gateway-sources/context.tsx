import { createContext, useContext, type ReactNode } from 'react';

import { unresolvedClient, useCurrentProjectId } from '@/features/projects/current-project';

import type { SourcesClient, SourcesClients } from './client';

const SourcesClientsContext = createContext<SourcesClients | undefined>(undefined);

const unresolved = unresolvedClient<SourcesClient>();

export function SourcesClientProvider({ clients, children }: { clients: SourcesClients; children: ReactNode }) {
  return <SourcesClientsContext value={clients}>{children}</SourcesClientsContext>;
}

export function useSourcesClient(): SourcesClient {
  const clients = useContext(SourcesClientsContext);
  const projectId = useCurrentProjectId();
  if (!clients) throw new Error('SourcesClientProvider is missing from the component tree.');
  return projectId ? clients.forProject(projectId) : unresolved;
}
