import { createFileRoute } from '@tanstack/react-router';

import { KeyDetailScreen } from '@/screens/key-detail-screen';

export const Route = createFileRoute('/keys/keys/$keyRef')({ component: KeyRoute });

function KeyRoute() {
  const { keyRef } = Route.useParams();
  return <KeyDetailScreen keyRef={keyRef} />;
}
