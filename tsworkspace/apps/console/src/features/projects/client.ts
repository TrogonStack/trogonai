import type { CreatedProject, ProjectEvent } from '@trogonai/proto';

import type { NodeId } from '@/features/hierarchy/domain/node-id';

import type { ProjectCommand } from './domain/commands';
import type { ProjectId } from './domain/project-id';

export interface ProjectRecord {
  readonly project: CreatedProject;
  readonly revision: number;
  readonly createdAt: Date;
  readonly updatedAt: Date;
}

export interface StoredProjectEvent {
  readonly revision: number;
  readonly event: ProjectEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface DispatchOptions {
  readonly expectedRevision?: number;
  readonly commandId?: string;
}

export interface WorkspaceNode {
  readonly nodeId: NodeId;
  readonly displayName: string;
  readonly level: 'workspace' | 'folder';
}

export interface ProjectResourceSummary {
  readonly vaults: number;
  readonly credentials: number;
  readonly environments: number;
}

export interface WorkspaceNodeDirectory {
  workspaceNodes(): Promise<readonly WorkspaceNode[]>;
}

export interface ProjectResources {
  summarize(projectId: ProjectId): Promise<ProjectResourceSummary>;
}

export interface ProjectsClient {
  list(): Promise<readonly ProjectRecord[]>;
  get(projectId: string): Promise<ProjectRecord | undefined>;
  history(projectId: string): Promise<readonly StoredProjectEvent[]>;
  parentCandidates(): Promise<readonly WorkspaceNode[]>;
  attachedTo(nodeId: NodeId): Promise<number>;
  resources(projectId: ProjectId): Promise<ProjectResourceSummary>;
  dispatch(command: ProjectCommand, options?: DispatchOptions): Promise<ProjectRecord>;
}
