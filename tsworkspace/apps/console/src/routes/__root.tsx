import { TanStackDevtools } from '@tanstack/react-devtools';
import type { QueryClient } from '@tanstack/react-query';
import { Outlet, createRootRouteWithContext, retainSearchParams } from '@tanstack/react-router';
import { TanStackRouterDevtoolsPanel } from '@tanstack/react-router-devtools';

import { AppShell } from '@/components/app-shell';
import { ThemeProvider } from '@/components/theme-provider';
import { ToastProvider } from '@/components/ui/toaster';
import { TooltipProvider } from '@/components/ui/tooltip';

import '../styles.css';

interface RouterContext {
  queryClient: QueryClient;
}

export const Route = createRootRouteWithContext<RouterContext>()({
  validateSearch: (search: Record<string, unknown>): { project?: string } =>
    typeof search.project === 'string' && search.project !== '' ? { project: search.project } : {},
  search: { middlewares: [retainSearchParams(['project'])] },
  component: RootComponent,
});

function RootComponent() {
  return (
    <ThemeProvider>
      <TooltipProvider delay={300}>
        <ToastProvider>
          <AppShell>
            <Outlet />
          </AppShell>
        </ToastProvider>
      </TooltipProvider>
      {import.meta.env.MODE === 'development' ? (
        <TanStackDevtools
          config={{
            position: 'bottom-right',
          }}
          plugins={[
            {
              name: 'TanStack Router',
              render: <TanStackRouterDevtoolsPanel />,
            },
          ]}
        />
      ) : null}
    </ThemeProvider>
  );
}
