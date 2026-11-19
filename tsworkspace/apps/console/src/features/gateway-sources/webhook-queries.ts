import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { useCurrentProjectId } from '@/features/projects/current-project';

import { useSourcesClient } from './context';
import type { WebhookDraft, WebhookEndpointId, WebhookTarget, WebhookTestRequest } from './domain/webhook-endpoint';

const root = ['webhook-endpoints'] as const;

export function useWebhookEndpointsQuery() {
  const client = useSourcesClient();
  const projectId = useCurrentProjectId();
  return useQuery({
    queryKey: [...root, projectId],
    enabled: projectId !== undefined,
    queryFn: () => client.listWebhookEndpoints(),
  });
}

export type WebhookMutation =
  | { readonly case: 'create'; readonly draft: WebhookDraft }
  | { readonly case: 'update'; readonly target: WebhookTarget; readonly draft: WebhookDraft }
  | { readonly case: 'enable'; readonly target: WebhookTarget; readonly enabled: boolean }
  | { readonly case: 'rotate' | 'revoke' | 'delete'; readonly target: WebhookTarget }
  | { readonly case: 'test'; readonly id: WebhookEndpointId; readonly request: WebhookTestRequest };

export function useWebhookMutation() {
  const client = useSourcesClient();
  const cache = useQueryClient();
  return useMutation({
    mutationFn: async (command: WebhookMutation) => {
      switch (command.case) {
        case 'create':
          return client.createWebhookEndpoint(command.draft);
        case 'update':
          return client.updateWebhookEndpoint(command.target, command.draft);
        case 'enable':
          return client.setWebhookEnabled(command.target, command.enabled);
        case 'rotate':
          return client.rotateWebhookCredential(command.target);
        case 'revoke':
          return client.revokeWebhookCredential(command.target);
        case 'delete':
          return client.deleteWebhookEndpoint(command.target);
        case 'test':
          return client.simulateWebhookDelivery(command.id, command.request);
      }
    },
    onSettled: () => cache.invalidateQueries({ queryKey: root }),
  });
}
