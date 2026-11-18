import { SendIcon } from 'lucide-react';
import { useState } from 'react';

import { SelectField, TextField } from '@/components/field';
import { StatusPill } from '@/components/lifecycle-badge';
import { Button } from '@/components/ui/button';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import {
  webhookOutcomeLabels,
  type WebhookEndpoint,
  type WebhookTestRequest,
  type WebhookTestScenario,
} from '../domain/webhook-endpoint';

export function WebhookDeliveries({
  endpoint,
  pending,
  onTest,
}: {
  endpoint: WebhookEndpoint;
  pending: boolean;
  onTest: (request: WebhookTestRequest) => void;
}) {
  const [deliveryId, setDeliveryId] = useState('order-1042-created');
  const [scenario, setScenario] = useState<WebhookTestScenario>('valid');
  const scenarios = [
    { value: 'valid' as const, label: 'Valid request' },
    { value: 'invalid_credential' as const, label: 'Invalid authentication' },
    ...(endpoint.authentication.mode === 'hmac'
      ? [{ value: 'stale_timestamp' as const, label: 'Expired timestamp' }]
      : []),
    { value: 'oversized_body' as const, label: 'Body over limit' },
  ];
  const latest = endpoint.recent[0];
  return (
    <section className="flex min-w-0 flex-col gap-4" aria-label="Webhook delivery preview">
      <div className="flex flex-col gap-1">
        <h4 className="text-sm font-medium">Try a delivery</h4>
        <p className="text-muted-foreground text-xs">
          Simulated admission only. No HTTP request is sent and no business action runs.
        </p>
      </div>
      <form
        className="grid items-end gap-3 sm:grid-cols-[1fr_1fr_auto]"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          onTest({ deliveryId, scenario });
        }}
      >
        <TextField id="test-delivery-id" label="Delivery ID" value={deliveryId} onChange={setDeliveryId} mono />
        <SelectField<WebhookTestScenario>
          id="test-scenario"
          label="Request scenario"
          value={scenario}
          onChange={setScenario}
          options={scenarios}
        />
        <Button type="submit" disabled={pending}>
          <SendIcon data-icon="inline-start" />
          Simulate delivery
        </Button>
      </form>
      <p className="text-muted-foreground text-xs">
        Use 1 to 128 visible ASCII characters without periods. Reuse an ID to preview a retry within the configured
        duplicate window.
      </p>
      {latest ? (
        <div
          role="status"
          className="border-border bg-muted/35 flex flex-wrap items-center gap-2 rounded-md border p-3"
        >
          <StatusPill
            tone={latest.outcome.status === 202 ? 'success' : 'danger'}
            label={`Simulated ${latest.outcome.status}`}
          />
          <span className="text-sm">{webhookOutcomeLabels[latest.outcome.kind]}</span>
          <span className="text-muted-foreground text-xs">
            {latest.outcome.kind === 'stored'
              ? 'One WebhookReceived event.'
              : latest.outcome.kind === 'duplicate'
                ? 'The original event is retained.'
                : 'No event is published.'}
          </span>
        </div>
      ) : null}
      <div className="flex flex-col gap-2">
        <h4 className="text-sm font-medium">Recent simulated deliveries</h4>
        {endpoint.recent.length === 0 ? (
          <p className="text-muted-foreground text-xs">
            No deliveries yet. Try a valid request, then repeat the same delivery ID.
          </p>
        ) : (
          <ul aria-label="Recent webhook deliveries" className="flex flex-col">
            {endpoint.recent.map((delivery) => (
              <li key={delivery.attemptId} className="border-border border-b py-2 last:border-b-0">
                <details>
                  <summary className="flex cursor-pointer flex-wrap items-center justify-between gap-2 text-xs">
                    <span className="font-mono break-all">{delivery.deliveryId || '(empty ID)'}</span>
                    <span className="flex flex-wrap items-center gap-3">
                      <RelativeTime date={delivery.receivedAt} />
                      <StatusPill
                        tone={delivery.outcome.status === 202 ? 'success' : 'danger'}
                        label={`${delivery.outcome.status} ${webhookOutcomeLabels[delivery.outcome.kind]}`}
                      />
                    </span>
                  </summary>
                  <dl className="text-muted-foreground grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 pt-3 text-xs">
                    <dt>Attempt</dt>
                    <dd className="font-mono break-all">{delivery.attemptId}</dd>
                    <dt>Received</dt>
                    <dd>{delivery.receivedAt.toISOString()}</dd>
                    <dt>Configuration</dt>
                    <dd>Revision {delivery.configurationRevision}</dd>
                    <dt>Stored event</dt>
                    <dd>
                      {delivery.outcome.kind === 'stored' ? 'WebhookReceived (simulated)' : 'No additional event'}
                    </dd>
                  </dl>
                </details>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
