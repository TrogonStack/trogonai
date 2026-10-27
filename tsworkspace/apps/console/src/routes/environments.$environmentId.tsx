import { createFileRoute } from '@tanstack/react-router';

import { EnvironmentDetailScreen } from '@/screens/environment-detail-screen';

export const Route = createFileRoute('/environments/$environmentId')({ component: EnvironmentRoute });

function EnvironmentRoute() {
  const { environmentId } = Route.useParams();
  return <EnvironmentDetailScreen environmentId={environmentId} />;
}
