import type { KeyPurposeDeclaration } from '@trogonai/proto';

import { purposeIdOf, type PurposeId } from './identifiers';

export interface PurposeDefinition {
  readonly id: PurposeId;
  readonly label: string;
  readonly description: string;
  readonly owningService: string;
}

export function purposeDefinitionOf(declaration: KeyPurposeDeclaration): PurposeDefinition {
  return {
    id: purposeIdOf(declaration.purpose),
    label: declaration.displayName,
    description: declaration.description,
    owningService: declaration.owner?.value ?? '',
  };
}

export function purposeLabel(catalogue: readonly PurposeDefinition[], id: PurposeId): string {
  return catalogue.find((purpose) => purpose.id === id)?.label ?? id;
}
