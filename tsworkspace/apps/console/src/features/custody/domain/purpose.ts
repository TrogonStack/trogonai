import type { PurposeId } from './identifiers';

export interface PurposeDefinition {
  readonly id: PurposeId;
  readonly label: string;
  readonly description: string;
  readonly owningService: string;
}

export function purposeLabel(catalogue: readonly PurposeDefinition[], id: PurposeId): string {
  return catalogue.find((purpose) => purpose.id === id)?.label ?? id;
}
