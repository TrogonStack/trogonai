import { createFileRoute } from '@tanstack/react-router';

import { ProjectDetailScreen } from '@/screens/project-detail-screen';

export const Route = createFileRoute('/projects/$projectId')({
  component: ProjectRoute,
});

function ProjectRoute() {
  const { projectId } = Route.useParams();
  return <ProjectDetailScreen projectId={projectId} />;
}
