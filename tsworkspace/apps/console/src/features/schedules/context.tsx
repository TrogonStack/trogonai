import { createContext, useContext, type ReactNode } from 'react';

import { unresolvedClient, useCurrentProjectId } from '@/features/projects/current-project';

import type { SchedulesClient, SchedulesClients } from './client';
const Context = createContext<SchedulesClients | undefined>(undefined);
const unresolved = unresolvedClient<SchedulesClient>();
export function SchedulesClientProvider({ clients, children }: { clients: SchedulesClients; children: ReactNode }) {
  return <Context value={clients}>{children}</Context>;
}
export function useSchedulesClient(): SchedulesClient {
  const clients = useContext(Context);
  const project = useCurrentProjectId();
  if (!clients) throw new Error('SchedulesClientProvider is missing.');
  return project ? clients.forProject(project) : unresolved;
}
