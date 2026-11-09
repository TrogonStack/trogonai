import { createFileRoute } from '@tanstack/react-router';

import { NewProjectScreen } from '@/screens/new-project-screen';

export const Route = createFileRoute('/projects/new')({
  component: NewProjectScreen,
});
