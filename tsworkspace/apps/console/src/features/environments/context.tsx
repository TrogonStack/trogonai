import { createContext, useContext, type ReactNode } from 'react';

import { unresolvedClient, useCurrentProjectId } from '@/features/projects/current-project';

import type { EnvironmentsClient, EnvironmentsClients } from './client';

const EnvironmentsClientsContext = createContext<EnvironmentsClients | undefined>(undefined);

const unresolved = unresolvedClient<EnvironmentsClient>();

export function EnvironmentsClientProvider({
  clients,
  children,
}: {
  clients: EnvironmentsClients;
  children: ReactNode;
}) {
  return <EnvironmentsClientsContext value={clients}>{children}</EnvironmentsClientsContext>;
}

export function useEnvironmentsClient(): EnvironmentsClient {
  const clients = useContext(EnvironmentsClientsContext);
  const projectId = useCurrentProjectId();
  if (!clients) throw new Error('EnvironmentsClientProvider is missing from the component tree.');
  return projectId ? clients.forProject(projectId) : unresolved;
}
