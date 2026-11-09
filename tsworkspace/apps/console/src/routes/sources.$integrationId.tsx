import { createFileRoute } from '@tanstack/react-router';

import { SourceDetailScreen } from '@/screens/source-detail-screen';

export const Route = createFileRoute('/sources/$integrationId')({ component: SourceRoute });

function SourceRoute() {
  const { integrationId } = Route.useParams();
  return <SourceDetailScreen integrationId={integrationId} />;
}
