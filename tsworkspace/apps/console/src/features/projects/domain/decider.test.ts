import { ProjectAccessPreset, ProjectLifecycle } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { projectCommands, type ProjectCommand } from './commands';
import { decide, evolve, fold, initialState, presentProject, ReplayFault, MAX_DISPLAY_NAME_LENGTH } from './decider';
import { mintNodeId } from './node-id';
import { mintProjectId } from './project-id';

const id = mintProjectId('portal');
const parent = mintNodeId('node_platform');

const create = (overrides: Partial<Parameters<typeof projectCommands.create>[0]> = {}): ProjectCommand =>
  projectCommands.create({
    projectId: id,
    displayName: 'Portal',
    goal: 'Ship the portal.',
    parent,
    initialAccessPreset: ProjectAccessPreset.INHERITED,
    ...overrides,
  });

function accept(state: ReturnType<typeof initialState>, command: ProjectCommand) {
  const decision = decide(state, command);
  if (!decision.ok) throw decision.fault;
  return fold(decision.events, state);
}

function reason(state: ReturnType<typeof initialState>, command: ProjectCommand) {
  const decision = decide(state, command);
  return decision.ok ? undefined : decision.fault.reason;
}

const active = () => accept(initialState(), create());
const archived = () => accept(active(), projectCommands.archive(id));

describe('project decider', () => {
  it('creates an active project that keeps its genesis facts', () => {
    const project = presentProject(active());
    expect(project).toMatchObject({
      displayName: 'Portal',
      goal: 'Ship the portal.',
      lifecycle: ProjectLifecycle.ACTIVE,
      initialAccessPreset: ProjectAccessPreset.INHERITED,
    });
    expect(project?.parent?.value).toBe('node_platform');
  });

  it('carries the members-only preset exactly as sent', () => {
    const state = accept(initialState(), create({ initialAccessPreset: ProjectAccessPreset.MEMBERS_ONLY }));
    expect(presentProject(state)?.initialAccessPreset).toBe(ProjectAccessPreset.MEMBERS_ONLY);
  });

  it('rejects an unspecified access preset instead of resolving it', () => {
    const state = initialState();
    const command = create({ initialAccessPreset: ProjectAccessPreset.UNSPECIFIED });
    expect(reason(state, command)).toBe('PROJECT_COMMAND_INVALID');
  });

  it('rejects a missing name, goal or parent', () => {
    expect(reason(initialState(), create({ displayName: '  ' }))).toBe('PROJECT_COMMAND_INVALID');
    expect(reason(initialState(), create({ goal: '' }))).toBe('PROJECT_COMMAND_INVALID');
    expect(reason(initialState(), create({ parent: '' as never }))).toBe('PROJECT_COMMAND_INVALID');
    expect(reason(initialState(), create({ displayName: 'x'.repeat(MAX_DISPLAY_NAME_LENGTH + 1) }))).toBe(
      'PROJECT_COMMAND_INVALID',
    );
  });

  it('rejects creating the same project twice', () => {
    expect(reason(active(), create())).toBe('PROJECT_ALREADY_EXISTS');
  });

  it('renames and restates the goal of an active project', () => {
    const state = accept(
      accept(active(), projectCommands.rename(id, 'Customer portal')),
      projectCommands.restateGoal(id, 'Keep it rotated.'),
    );
    expect(presentProject(state)).toMatchObject({ displayName: 'Customer portal', goal: 'Keep it rotated.' });
  });

  it('rejects a rename or restatement that changes nothing', () => {
    expect(reason(active(), projectCommands.rename(id, 'Portal'))).toBe('PROJECT_COMMAND_INVALID');
    expect(reason(active(), projectCommands.restateGoal(id, 'Ship the portal.'))).toBe('PROJECT_COMMAND_INVALID');
  });

  it('accepts only unarchive while archived', () => {
    const state = archived();
    expect(presentProject(state)?.lifecycle).toBe(ProjectLifecycle.ARCHIVED);
    expect(reason(state, projectCommands.rename(id, 'Other'))).toBe('PROJECT_ARCHIVED');
    expect(reason(state, projectCommands.restateGoal(id, 'Other'))).toBe('PROJECT_ARCHIVED');
    expect(reason(state, projectCommands.archive(id))).toBe('PROJECT_ARCHIVED');
    expect(presentProject(accept(state, projectCommands.unarchive(id)))?.lifecycle).toBe(ProjectLifecycle.ACTIVE);
  });

  it('rejects unarchiving an active project and commands on a missing one', () => {
    expect(reason(active(), projectCommands.unarchive(id))).toBe('PROJECT_NOT_ARCHIVED');
    expect(reason(initialState(), projectCommands.unarchive(id))).toBe('PROJECT_NOT_FOUND');
    expect(reason(initialState(), projectCommands.rename(id, 'Other'))).toBe('PROJECT_NOT_FOUND');
    expect(reason(initialState(), projectCommands.archive(id))).toBe('PROJECT_NOT_FOUND');
  });

  it('never changes the parent or the access preset after creation', () => {
    const before = presentProject(active());
    const after = presentProject(accept(accept(active(), projectCommands.archive(id)), projectCommands.unarchive(id)));
    expect(after?.parent?.value).toBe(before?.parent?.value);
    expect(after?.initialAccessPreset).toBe(before?.initialAccessPreset);
  });
});

describe('project evolve', () => {
  it('refuses an event that precedes creation', () => {
    const decision = decide(active(), projectCommands.archive(id));
    if (!decision.ok) throw decision.fault;
    expect(() => evolve(initialState(), decision.events[0]!)).toThrow(ReplayFault);
  });

  it('refuses a second creation event', () => {
    const decision = decide(initialState(), create());
    if (!decision.ok) throw decision.fault;
    expect(() => fold([...decision.events, ...decision.events])).toThrow(ReplayFault);
  });
});
