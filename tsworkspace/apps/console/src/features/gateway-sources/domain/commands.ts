import type { IntegrationId } from './integration-id';
import type { SlotName } from './source-kind';

export type SourceCommand =
  | { readonly case: 'pauseSource'; readonly integrationId: IntegrationId }
  | { readonly case: 'resumeSource'; readonly integrationId: IntegrationId }
  | { readonly case: 'rotateSlot'; readonly integrationId: IntegrationId; readonly slot: SlotName }
  | { readonly case: 'reregisterWebhook'; readonly integrationId: IntegrationId };

export const sourceCommands = {
  pause: (integrationId: IntegrationId): SourceCommand => ({ case: 'pauseSource', integrationId }),
  resume: (integrationId: IntegrationId): SourceCommand => ({ case: 'resumeSource', integrationId }),
  rotateSlot: (integrationId: IntegrationId, slot: SlotName): SourceCommand => ({
    case: 'rotateSlot',
    integrationId,
    slot,
  }),
  reregisterWebhook: (integrationId: IntegrationId): SourceCommand => ({ case: 'reregisterWebhook', integrationId }),
};
