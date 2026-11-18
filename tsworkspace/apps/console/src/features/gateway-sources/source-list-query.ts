import { useMemo } from 'react';

import { providerSourceRow, webhookSourceRow } from './domain/source-list';
import { useSourcesQuery } from './queries';
import { useWebhookEndpointsQuery } from './webhook-queries';

export function useSourceList() {
  const providers = useSourcesQuery();
  const webhooks = useWebhookEndpointsQuery();
  const data = useMemo(() => {
    if (!providers.data || !webhooks.data) return undefined;
    const now = new Date();
    return [
      ...providers.data.map(providerSourceRow),
      ...webhooks.data.map((endpoint) => webhookSourceRow(endpoint, now)),
    ];
  }, [providers.data, webhooks.data]);
  return { data, error: providers.error ?? webhooks.error, isPending: providers.isPending || webhooks.isPending };
}
