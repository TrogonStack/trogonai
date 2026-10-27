import type { EnvironmentId } from './environment-id';

export type EnvironmentCommand =
  | {
      readonly case: 'createEnvironment';
      readonly environmentId: EnvironmentId;
      readonly displayName: string;
      readonly description: string;
    }
  | {
      readonly case: 'updateEnvironment';
      readonly environmentId: EnvironmentId;
      readonly displayName: string;
      readonly description: string;
    }
  | { readonly case: 'deleteEnvironment'; readonly environmentId: EnvironmentId };

export const environmentCommands = {
  create: (input: { environmentId: EnvironmentId; displayName: string; description?: string }): EnvironmentCommand => ({
    case: 'createEnvironment',
    environmentId: input.environmentId,
    displayName: input.displayName,
    description: input.description ?? '',
  }),
  update: (environmentId: EnvironmentId, input: { displayName: string; description?: string }): EnvironmentCommand => ({
    case: 'updateEnvironment',
    environmentId,
    displayName: input.displayName,
    description: input.description ?? '',
  }),
  delete: (environmentId: EnvironmentId): EnvironmentCommand => ({ case: 'deleteEnvironment', environmentId }),
};
