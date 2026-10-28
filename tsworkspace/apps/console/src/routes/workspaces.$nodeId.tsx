import { createFileRoute } from '@tanstack/react-router';

import { WorkspaceDetailScreen } from '@/screens/workspace-detail-screen';

export const Route = createFileRoute('/workspaces/$nodeId')({
  component: WorkspaceRoute,
});

function WorkspaceRoute() {
  const { nodeId } = Route.useParams();
  return <WorkspaceDetailScreen nodeId={nodeId} />;
}
