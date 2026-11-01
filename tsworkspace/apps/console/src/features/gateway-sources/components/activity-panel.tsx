import { StatusPill } from '@/components/lifecycle-badge';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { rejectionLabels } from '../domain/delivery';
import type { SourceView } from '../domain/view';
import { ActivityChart } from './activity-chart';

export function ActivityPanel({ source }: { source: SourceView }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Delivery activity</CardTitle>
        <CardDescription>
          {source.deliveries24h} deliveries in the last 24 hours, {source.rejected24h} rejected.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-6">
        <ActivityChart hourly={source.hourly} />
        <div className="flex flex-col gap-2">
          <h3 className="micro text-muted-foreground text-[10px]">Recent deliveries</h3>
          <ul aria-label="Recent deliveries">
            {source.recent.map((delivery) => (
              <li
                key={delivery.id}
                className="border-border flex flex-wrap items-center justify-between gap-3 border-b py-2 last:border-b-0"
              >
                <span className="flex min-w-0 items-center gap-3">
                  <RelativeTime date={delivery.receivedAt} className="text-muted-foreground w-20 text-xs" />
                  <span className="font-mono text-xs">{delivery.event}</span>
                </span>
                {delivery.outcome.status === 'accepted' ? (
                  <StatusPill tone="success" label="Accepted" />
                ) : (
                  <StatusPill tone="danger" label={`Rejected: ${rejectionLabels[delivery.outcome.reason]}`} />
                )}
              </li>
            ))}
          </ul>
        </div>
      </CardContent>
    </Card>
  );
}
