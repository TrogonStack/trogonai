import { createFileRoute } from '@tanstack/react-router';

import { CredentialDetailScreen } from '@/screens/credential-detail-screen';

export const Route = createFileRoute('/secrets/credentials/$secretRef')({ component: CredentialRoute });

function CredentialRoute() {
  const { secretRef } = Route.useParams();
  return <CredentialDetailScreen secretRef={secretRef} />;
}
