import { createFileRoute } from '@tanstack/react-router';

import { NewKeyScreen } from '@/screens/new-key-screen';

export const Route = createFileRoute('/keys/keys/new')({
  validateSearch: (search: Record<string, unknown>): { backend?: string } =>
    typeof search.backend === 'string' && search.backend !== '' ? { backend: search.backend } : {},
  component: NewKeyRoute,
});

function NewKeyRoute() {
  const { backend } = Route.useSearch();
  return <NewKeyScreen backendId={backend} />;
}
