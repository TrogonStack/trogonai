import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider, createMemoryHistory } from '@tanstack/react-router';
import { render } from '@testing-library/react';

import { createFakeBackend, type FakeBackend } from '@/fake-backend';
import { EnvironmentsClientProvider } from '@/features/environments/context';
import { ProjectsClientProvider } from '@/features/projects/context';
import { ProviderConnectionsClientProvider } from '@/features/provider-connections/context';
import { SecretsClientProvider } from '@/features/secrets/context';

import { getRouter } from '../router';

export function renderApp(path: string, backend: FakeBackend = createFakeBackend({ latencyMs: 0 })) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const router = getRouter(queryClient, createMemoryHistory({ initialEntries: [path] }));
  const view = render(
    <QueryClientProvider client={queryClient}>
      <ProviderConnectionsClientProvider client={backend.providerConnections}>
        <ProjectsClientProvider client={backend.projects}>
          <EnvironmentsClientProvider clients={backend.environments}>
            <SecretsClientProvider clients={backend.secrets}>
              <RouterProvider router={router} />
            </SecretsClientProvider>
          </EnvironmentsClientProvider>
        </ProjectsClientProvider>
      </ProviderConnectionsClientProvider>
    </QueryClientProvider>,
  );
  return {
    ...view,
    client: backend.providerConnections,
    projects: backend.projects,
    secrets: backend.secrets,
    environments: backend.environments,
    router,
  };
}
