import { createFileRoute } from '@tanstack/react-router';

import { ProjectsScreen } from '@/screens/projects-screen';

export const Route = createFileRoute('/projects/')({
  component: ProjectsScreen,
});
