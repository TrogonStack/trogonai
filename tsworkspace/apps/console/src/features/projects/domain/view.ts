import { ProjectLifecycle, type ProjectAccessPreset } from '@trogonai/proto';

import type { ProjectRecord } from '../client';
import { nodeIdOf, type NodeId } from './node-id';
import { projectIdOf, type ProjectId } from './project-id';

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

export interface ProjectView {
  readonly projectId: ProjectId;
  readonly revision: number;
  readonly displayName: string;
  readonly goal: string;
  readonly lifecycle: ProjectLifecycle;
  readonly archived: boolean;
  readonly parent: NodeId;
  readonly initialAccessPreset: ProjectAccessPreset;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly actions: {
    readonly rename: ActionAvailability;
    readonly restateGoal: ActionAvailability;
    readonly archive: ActionAvailability;
    readonly unarchive: ActionAvailability;
  };
}

const ARCHIVED_REASON = 'An archived project accepts nothing but unarchive.';
const ACTIVE_REASON = 'The project is already active.';

export function toProjectView(record: ProjectRecord): ProjectView {
  const { project } = record;
  const archived = project.lifecycle === ProjectLifecycle.ARCHIVED;
  const blocked: ActionAvailability = { available: false, reason: ARCHIVED_REASON };
  return {
    projectId: projectIdOf(project.projectId),
    revision: record.revision,
    displayName: project.displayName,
    goal: project.goal,
    lifecycle: project.lifecycle,
    archived,
    parent: nodeIdOf(project.parent),
    initialAccessPreset: project.initialAccessPreset,
    createdAt: record.createdAt,
    updatedAt: record.updatedAt,
    actions: {
      rename: archived ? blocked : { available: true },
      restateGoal: archived ? blocked : { available: true },
      archive: archived ? blocked : { available: true },
      unarchive: archived ? { available: true } : { available: false, reason: ACTIVE_REASON },
    },
  };
}

export function byAge(left: ProjectView, right: ProjectView): number {
  return left.createdAt.getTime() - right.createdAt.getTime();
}

export function activeFirst(projects: readonly ProjectView[]): readonly ProjectView[] {
  return [...projects].sort((left, right) => Number(left.archived) - Number(right.archived) || byAge(left, right));
}

export function resolveCurrentProject(
  projects: readonly ProjectView[],
  requested: string | undefined,
): ProjectView | undefined {
  const requestedProject = requested ? projects.find((project) => project.projectId === requested) : undefined;
  return requestedProject ?? activeFirst(projects)[0];
}
