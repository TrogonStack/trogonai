import { create } from '@bufbuild/protobuf';
import { EmptySchema } from '@bufbuild/protobuf/wkt';
import {
  CreatedProjectSchema,
  ProjectAccessPreset,
  ProjectArchivedSchema,
  ProjectCreatedSchema,
  ProjectEventSchema,
  ProjectGoalRestatedSchema,
  ProjectLifecycle,
  ProjectRenamedSchema,
  ProjectStateSchema,
  ProjectUnarchivedSchema,
  type CreatedProject,
  type CreateProject,
  type ProjectEvent,
  type ProjectState,
} from '@trogonai/proto';

import type { ProjectCommand } from './commands';
import { invalidProject, ProjectFault } from './faults';

export const MAX_DISPLAY_NAME_LENGTH = 80;
export const MAX_GOAL_LENGTH = 400;

export type Decision =
  | { readonly ok: true; readonly events: readonly ProjectEvent[] }
  | { readonly ok: false; readonly fault: ProjectFault };

export class ReplayFault extends Error {
  constructor(message: string) {
    super(`Replay rejected: ${message}`);
    this.name = 'ReplayFault';
  }
}

export function initialState(): ProjectState {
  return create(ProjectStateSchema, { state: { case: 'missing', value: create(EmptySchema) } });
}

function presentState(project: CreatedProject): ProjectState {
  return create(ProjectStateSchema, { state: { case: 'present', value: project } });
}

export function presentProject(state: ProjectState): CreatedProject | undefined {
  return state.state.case === 'present' ? state.state.value : undefined;
}

export function displayNameProblem(value: string): string | undefined {
  const length = value.trim().length;
  if (length === 0) return 'A project name is required.';
  if (length > MAX_DISPLAY_NAME_LENGTH) return `A project name is at most ${MAX_DISPLAY_NAME_LENGTH} characters.`;
  return undefined;
}

export function goalProblem(value: string): string | undefined {
  const length = value.trim().length;
  if (length === 0) return 'A project goal is required.';
  if (length > MAX_GOAL_LENGTH) return `A project goal is at most ${MAX_GOAL_LENGTH} characters.`;
  return undefined;
}

export function validateDisplayName(value: string): string {
  const problem = displayNameProblem(value);
  if (problem) throw invalidProject(problem);
  return value.trim();
}

export function validateGoal(value: string): string {
  const problem = goalProblem(value);
  if (problem) throw invalidProject(problem);
  return value.trim();
}

function requireProject(state: ProjectState): CreatedProject {
  const project = presentProject(state);
  if (!project) throw new ProjectFault('PROJECT_NOT_FOUND');
  return project;
}

function requireActive(state: ProjectState): CreatedProject {
  const project = requireProject(state);
  if (project.lifecycle !== ProjectLifecycle.ACTIVE) throw new ProjectFault('PROJECT_ARCHIVED');
  return project;
}

function event(init: ProjectEvent['event']): ProjectEvent {
  return create(ProjectEventSchema, { event: init });
}

function decideCreate(state: ProjectState, command: CreateProject): ProjectEvent[] {
  if (state.state.case !== 'missing') throw new ProjectFault('PROJECT_ALREADY_EXISTS');
  const projectId = command.projectId?.value ?? '';
  if (projectId === '') throw invalidProject('A project id is required.');
  const displayName = validateDisplayName(command.displayName);
  const goal = validateGoal(command.goal);
  if (!command.parent?.value) throw invalidProject('A parent node is required.');
  if (
    command.initialAccessPreset !== ProjectAccessPreset.INHERITED &&
    command.initialAccessPreset !== ProjectAccessPreset.MEMBERS_ONLY
  ) {
    throw invalidProject('An initial access preset is required and is never assumed.');
  }
  return [
    event({
      case: 'projectCreated',
      value: create(ProjectCreatedSchema, {
        projectId: command.projectId,
        displayName,
        goal,
        parent: command.parent,
        initialAccessPreset: command.initialAccessPreset,
      }),
    }),
  ];
}

export function decide(state: ProjectState, command: ProjectCommand): Decision {
  try {
    switch (command.case) {
      case 'create':
        return { ok: true, events: decideCreate(state, command.value) };
      case 'rename': {
        const project = requireActive(state);
        const displayName = validateDisplayName(command.value.displayName);
        if (displayName === project.displayName) throw invalidProject('The name did not change.');
        return {
          ok: true,
          events: [
            event({
              case: 'projectRenamed',
              value: create(ProjectRenamedSchema, { projectId: project.projectId, displayName }),
            }),
          ],
        };
      }
      case 'restateGoal': {
        const project = requireActive(state);
        const goal = validateGoal(command.value.goal);
        if (goal === project.goal) throw invalidProject('The goal did not change.');
        return {
          ok: true,
          events: [
            event({
              case: 'projectGoalRestated',
              value: create(ProjectGoalRestatedSchema, { projectId: project.projectId, goal }),
            }),
          ],
        };
      }
      case 'archive': {
        const project = requireActive(state);
        return {
          ok: true,
          events: [
            event({ case: 'projectArchived', value: create(ProjectArchivedSchema, { projectId: project.projectId }) }),
          ],
        };
      }
      case 'unarchive': {
        const project = requireProject(state);
        if (project.lifecycle !== ProjectLifecycle.ARCHIVED) throw new ProjectFault('PROJECT_NOT_ARCHIVED');
        return {
          ok: true,
          events: [
            event({
              case: 'projectUnarchived',
              value: create(ProjectUnarchivedSchema, { projectId: project.projectId }),
            }),
          ],
        };
      }
    }
  } catch (error) {
    if (error instanceof ProjectFault) return { ok: false, fault: error };
    throw error;
  }
}

function sameProject(project: CreatedProject, occurred: { projectId?: { value: string } }): boolean {
  return project.projectId?.value === occurred.projectId?.value;
}

export function evolve(state: ProjectState, input: ProjectEvent): ProjectState {
  const next = input.event;
  const project = presentProject(state);

  if (next.case === undefined) throw new ReplayFault('event is not set');

  if (next.case === 'projectCreated') {
    if (project) throw new ReplayFault('project is already created');
    const created = next.value;
    return presentState(
      create(CreatedProjectSchema, {
        projectId: created.projectId,
        displayName: created.displayName,
        goal: created.goal,
        lifecycle: ProjectLifecycle.ACTIVE,
        parent: created.parent,
        initialAccessPreset: created.initialAccessPreset,
      }),
    );
  }

  if (!project) throw new ReplayFault('event precedes creation');
  if (!sameProject(project, next.value)) throw new ReplayFault('event names a different project');

  switch (next.case) {
    case 'projectRenamed':
      if (project.lifecycle !== ProjectLifecycle.ACTIVE) throw new ReplayFault('rename requires an active project');
      return presentState({ ...project, displayName: next.value.displayName });
    case 'projectGoalRestated':
      if (project.lifecycle !== ProjectLifecycle.ACTIVE) throw new ReplayFault('restating requires an active project');
      return presentState({ ...project, goal: next.value.goal });
    case 'projectArchived':
      if (project.lifecycle !== ProjectLifecycle.ACTIVE) throw new ReplayFault('archive requires an active project');
      return presentState({ ...project, lifecycle: ProjectLifecycle.ARCHIVED });
    case 'projectUnarchived':
      if (project.lifecycle !== ProjectLifecycle.ARCHIVED) {
        throw new ReplayFault('unarchive requires an archived project');
      }
      return presentState({ ...project, lifecycle: ProjectLifecycle.ACTIVE });
  }
}

export function fold(events: Iterable<ProjectEvent>, from: ProjectState = initialState()): ProjectState {
  let state = from;
  for (const next of events) state = evolve(state, next);
  return state;
}
