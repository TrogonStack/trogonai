import { createFileRoute } from '@tanstack/react-router';

import { BackendDetailScreen } from '@/screens/backend-detail-screen';

export const Route = createFileRoute('/keys/backends/$backendId')({ component: DetailRoute });

function DetailRoute() {
  const { backendId } = Route.useParams();
  return <BackendDetailScreen backendId={backendId} />;
}
