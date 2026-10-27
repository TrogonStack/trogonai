import { createFileRoute } from '@tanstack/react-router';

import { ProviderDetailScreen } from '@/screens/provider-detail-screen';

export const Route = createFileRoute('/providers/$connectionId')({ component: DetailRoute });

function DetailRoute() {
  const { connectionId } = Route.useParams();
  return <ProviderDetailScreen connectionId={connectionId} />;
}
