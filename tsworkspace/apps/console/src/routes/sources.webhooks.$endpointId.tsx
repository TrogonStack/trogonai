import { createFileRoute } from '@tanstack/react-router';

import { WebhookDetailScreen } from '@/screens/webhook-detail-screen';

export const Route = createFileRoute('/sources/webhooks/$endpointId')({ component: WebhookRoute });

function WebhookRoute() {
  const { endpointId } = Route.useParams();
  return <WebhookDetailScreen key={endpointId} endpointId={endpointId} />;
}
