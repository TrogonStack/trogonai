import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider, createMemoryHistory } from '@tanstack/react-router';
import { render } from '@testing-library/react';

import { createFakeBackend, type FakeBackend } from '@/fake-backend';
import { TenantCustodyClientProvider } from '@/features/custody/context';
import { EnvironmentsClientProvider } from '@/features/environments/context';
import { HierarchyClientProvider } from '@/features/hierarchy/context';
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
        <HierarchyClientProvider client={backend.hierarchy}>
          <ProjectsClientProvider client={backend.projects}>
            <EnvironmentsClientProvider clients={backend.environments}>
              <SecretsClientProvider clients={backend.secrets}>
                <TenantCustodyClientProvider client={backend.keyCustody}>
                  <RouterProvider router={router} />
                </TenantCustodyClientProvider>
              </SecretsClientProvider>
            </EnvironmentsClientProvider>
          </ProjectsClientProvider>
        </HierarchyClientProvider>
      </ProviderConnectionsClientProvider>
    </QueryClientProvider>,
  );
  return {
    ...view,
    client: backend.providerConnections,
    projects: backend.projects,
    hierarchy: backend.hierarchy,
    secrets: backend.secrets,
    environments: backend.environments,
    custody: backend.keyCustody,
    queryClient,
    router,
  };
}
