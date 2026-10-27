import type { ProjectId } from '@/features/projects/domain/project-id';

import type { EnvironmentCommand } from './domain/commands';
import type { EnvironmentState } from './domain/environment';
import type { EnvironmentId } from './domain/environment-id';
import type { EnvironmentEvent } from './domain/events';

export interface StoredEnvironmentEvent {
  readonly revision: number;
  readonly event: EnvironmentEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface DispatchOptions {
  readonly expectedRevision?: number;
  readonly commandId?: string;
}

export interface EnvironmentUsage {
  blockingVaults(environmentId: EnvironmentId): Promise<number>;
}

export interface EnvironmentsClient {
  list(): Promise<readonly EnvironmentState[]>;
  get(environmentId: string): Promise<EnvironmentState | undefined>;
  history(environmentId: string): Promise<readonly StoredEnvironmentEvent[]>;
  dispatch(command: EnvironmentCommand, options?: DispatchOptions): Promise<EnvironmentState>;
}

export interface EnvironmentsClients {
  forProject(projectId: ProjectId): EnvironmentsClient;
}
