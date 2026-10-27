import { createFileRoute } from '@tanstack/react-router';

import { VaultDetailScreen } from '@/screens/vault-detail-screen';

export const Route = createFileRoute('/secrets/vaults/$vaultId')({ component: VaultRoute });

function VaultRoute() {
  const { vaultId } = Route.useParams();
  return <VaultDetailScreen vaultId={vaultId} />;
}
