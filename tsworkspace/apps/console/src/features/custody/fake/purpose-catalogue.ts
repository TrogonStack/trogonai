import { purposeId } from '../domain/identifiers';
import type { PurposeDefinition } from '../domain/purpose';

export const fakePurposeCatalogue: readonly PurposeDefinition[] = [
  {
    id: purposeId('session_messages'),
    label: 'Session messages',
    description: 'Wraps the data keys that protect conversation messages stored by the session service.',
    owningService: 'Session service',
  },
  {
    id: purposeId('session_artifacts'),
    label: 'Session artifacts',
    description: 'Wraps the data keys that protect files and attachments produced inside a session.',
    owningService: 'Session service',
  },
  {
    id: purposeId('workflow_run_interactions'),
    label: 'Workflow run interactions',
    description: 'Wraps the data keys that protect the inputs and replies exchanged during a workflow run.',
    owningService: 'Workflow service',
  },
];
