import { createFileRoute } from '@tanstack/react-router';

import { NewWorkspaceScreen } from '@/screens/new-workspace-screen';

export const Route = createFileRoute('/workspaces/new')({
  component: NewWorkspaceScreen,
});
