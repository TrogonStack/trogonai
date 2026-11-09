import type { EnvironmentId } from './environment-id';

export type EnvironmentFact =
  | {
      readonly case: 'created';
      readonly displayName: string;
      readonly description: string;
      readonly builtIn: boolean;
    }
  | { readonly case: 'metadataUpdated'; readonly displayName: string; readonly description: string }
  | { readonly case: 'deleted' };

export interface EnvironmentEvent {
  readonly environmentId: EnvironmentId;
  readonly projectId: string;
  readonly eventId: string;
  readonly revision: number;
  readonly occurredAt: Date;
  readonly fact: EnvironmentFact;
}
