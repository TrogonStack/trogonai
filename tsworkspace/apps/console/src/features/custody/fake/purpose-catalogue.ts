import { create } from '@bufbuild/protobuf';
import { KeyPurposeDeclarationSchema, PurposeOwnerSchema } from '@trogonai/proto';

import { purposeMessage, purposeId } from '../domain/identifiers';
import { purposeDefinitionOf, type PurposeDefinition } from '../domain/purpose';

function declaration(id: string, displayName: string, description: string, owner: string) {
  return create(KeyPurposeDeclarationSchema, {
    purpose: purposeMessage(purposeId(id)),
    displayName,
    description,
    owner: create(PurposeOwnerSchema, { value: owner }),
  });
}

const declarations = [
  declaration(
    'session_messages',
    'Session messages',
    'Wraps the data keys that protect conversation messages stored by the session service.',
    'Session service',
  ),
  declaration(
    'session_artifacts',
    'Session artifacts',
    'Wraps the data keys that protect files and attachments produced inside a session.',
    'Session service',
  ),
  declaration(
    'workflow_run_interactions',
    'Workflow run interactions',
    'Wraps the data keys that protect the inputs and replies exchanged during a workflow run.',
    'Workflow service',
  ),
];

export const fakePurposeCatalogue: readonly PurposeDefinition[] = declarations.map(purposeDefinitionOf);
