import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { RouterProvider } from '@tanstack/react-router';
import ReactDOM from 'react-dom/client';

import { createFakeBackend } from './fake-backend';
import { TenantCustodyClientProvider } from './features/custody/context';
import { EnvironmentsClientProvider } from './features/environments/context';
import { SourcesClientProvider } from './features/gateway-sources/context';
import { HierarchyClientProvider } from './features/hierarchy/context';
import { ProjectsClientProvider } from './features/projects/context';
import { ProviderConnectionsClientProvider } from './features/provider-connections/context';
import { SecretsClientProvider } from './features/secrets/context';
import { getRouter } from './router';

const queryClient = new QueryClient();
const backend = createFakeBackend({ latencyMs: 250 });
const router = getRouter(queryClient);

const rootElement = document.getElementById('app')!;

if (!rootElement.innerHTML) {
  const root = ReactDOM.createRoot(rootElement);
  root.render(
    <QueryClientProvider client={queryClient}>
      <ProviderConnectionsClientProvider client={backend.providerConnections}>
        <HierarchyClientProvider client={backend.hierarchy}>
          <ProjectsClientProvider client={backend.projects}>
            <EnvironmentsClientProvider clients={backend.environments}>
              <SecretsClientProvider clients={backend.secrets}>
                <SourcesClientProvider clients={backend.gatewaySources}>
                  <TenantCustodyClientProvider client={backend.keyCustody}>
                    <RouterProvider router={router} />
                  </TenantCustodyClientProvider>
                </SourcesClientProvider>
              </SecretsClientProvider>
            </EnvironmentsClientProvider>
          </ProjectsClientProvider>
        </HierarchyClientProvider>
      </ProviderConnectionsClientProvider>
    </QueryClientProvider>,
  );
}
