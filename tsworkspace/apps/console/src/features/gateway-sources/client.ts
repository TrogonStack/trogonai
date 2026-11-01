import type { ProjectId } from '@/features/projects/domain/project-id';

import type { SourceCommand } from './domain/commands';
import type { IntegrationState } from './domain/integration';

export interface DispatchOptions {
  readonly expectedRevision?: number;
}

export interface SourcesClient {
  list(): Promise<readonly IntegrationState[]>;
  get(integrationId: string): Promise<IntegrationState | undefined>;
  dispatch(command: SourceCommand, options?: DispatchOptions): Promise<IntegrationState>;
}

export interface SourcesClients {
  forProject(projectId: ProjectId): SourcesClient;
}
