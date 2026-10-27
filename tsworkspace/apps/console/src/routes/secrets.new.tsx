import { createFileRoute } from '@tanstack/react-router';

import { NewCredentialScreen } from '@/screens/new-credential-screen';

export const Route = createFileRoute('/secrets/new')({
  validateSearch: (search: Record<string, unknown>): { vault?: string } =>
    typeof search.vault === 'string' && search.vault !== '' ? { vault: search.vault } : {},
  component: NewRoute,
});

function NewRoute() {
  const { vault } = Route.useSearch();
  return <NewCredentialScreen vaultId={vault} />;
}
