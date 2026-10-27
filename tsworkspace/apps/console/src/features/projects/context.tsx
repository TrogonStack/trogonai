import { createContext, useContext, type ReactNode } from 'react';

import type { ProjectsClient } from './client';

const ProjectsClientContext = createContext<ProjectsClient | undefined>(undefined);

export function ProjectsClientProvider({ client, children }: { client: ProjectsClient; children: ReactNode }) {
  return <ProjectsClientContext value={client}>{children}</ProjectsClientContext>;
}

export function useProjectsClient(): ProjectsClient {
  const client = useContext(ProjectsClientContext);
  if (!client) throw new Error('ProjectsClientProvider is missing from the component tree.');
  return client;
}
