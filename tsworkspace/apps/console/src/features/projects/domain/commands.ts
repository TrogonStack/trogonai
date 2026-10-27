import { create } from '@bufbuild/protobuf';
import {
  ArchiveProjectSchema,
  CreateProjectSchema,
  RenameProjectSchema,
  RestateProjectGoalSchema,
  UnarchiveProjectSchema,
  type ArchiveProject,
  type CreateProject,
  type ProjectAccessPreset,
  type RenameProject,
  type RestateProjectGoal,
  type UnarchiveProject,
} from '@trogonai/proto';

import { nodeIdMessage, type NodeId } from './node-id';
import { projectIdMessage, type ProjectId } from './project-id';

export type ProjectCommand =
  | { readonly case: 'create'; readonly value: CreateProject }
  | { readonly case: 'rename'; readonly value: RenameProject }
  | { readonly case: 'restateGoal'; readonly value: RestateProjectGoal }
  | { readonly case: 'archive'; readonly value: ArchiveProject }
  | { readonly case: 'unarchive'; readonly value: UnarchiveProject };

export function commandProjectId(command: ProjectCommand): string {
  return command.value.projectId?.value ?? '';
}

export const projectCommands = {
  create: (input: {
    projectId: ProjectId;
    displayName: string;
    goal: string;
    parent: NodeId;
    initialAccessPreset: ProjectAccessPreset;
  }): ProjectCommand => ({
    case: 'create',
    value: create(CreateProjectSchema, {
      projectId: projectIdMessage(input.projectId),
      displayName: input.displayName,
      goal: input.goal,
      parent: nodeIdMessage(input.parent),
      initialAccessPreset: input.initialAccessPreset,
    }),
  }),
  rename: (projectId: ProjectId, displayName: string): ProjectCommand => ({
    case: 'rename',
    value: create(RenameProjectSchema, { projectId: projectIdMessage(projectId), displayName }),
  }),
  restateGoal: (projectId: ProjectId, goal: string): ProjectCommand => ({
    case: 'restateGoal',
    value: create(RestateProjectGoalSchema, { projectId: projectIdMessage(projectId), goal }),
  }),
  archive: (projectId: ProjectId): ProjectCommand => ({
    case: 'archive',
    value: create(ArchiveProjectSchema, { projectId: projectIdMessage(projectId) }),
  }),
  unarchive: (projectId: ProjectId): ProjectCommand => ({
    case: 'unarchive',
    value: create(UnarchiveProjectSchema, { projectId: projectIdMessage(projectId) }),
  }),
};
