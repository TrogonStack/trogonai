import { createContext, useContext, type ReactNode } from 'react';

import { unresolvedClient, useCurrentProjectId } from '@/features/projects/current-project';

import type { SecretsClient, SecretsClients } from './client';

const SecretsClientsContext = createContext<SecretsClients | undefined>(undefined);

const unresolved = unresolvedClient<SecretsClient>();

export function SecretsClientProvider({ clients, children }: { clients: SecretsClients; children: ReactNode }) {
  return <SecretsClientsContext value={clients}>{children}</SecretsClientsContext>;
}

export function useSecretsClient(): SecretsClient {
  const clients = useContext(SecretsClientsContext);
  const projectId = useCurrentProjectId();
  if (!clients) throw new Error('SecretsClientProvider is missing from the component tree.');
  return projectId ? clients.forProject(projectId) : unresolved;
}
