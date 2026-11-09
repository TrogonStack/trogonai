import { createFileRoute } from '@tanstack/react-router';

import { WorkspacesScreen } from '@/screens/workspaces-screen';

export const Route = createFileRoute('/workspaces/')({
  component: WorkspacesScreen,
});
