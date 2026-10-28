import { describe, expect, it } from 'vitest';

import { environmentCommands, type EnvironmentCommand } from './commands';
import {
  MAX_DESCRIPTION_LENGTH,
  MAX_DISPLAY_NAME_LENGTH,
  decideEnvironment,
  foldEnvironment,
  type EnvironmentDecisionContext,
} from './environment';
import { builtInEnvironmentIds, mintEnvironmentId } from './environment-id';
import type { EnvironmentEvent } from './events';

const qa = mintEnvironmentId('qa');

function context(overrides: Partial<EnvironmentDecisionContext> = {}): EnvironmentDecisionContext {
  let counter = 0;
  return {
    now: new Date('2026-01-01T00:00:00Z'),
    projectId: 'proj-test',
    newId: (prefix) => `${prefix}_${++counter}`,
    vaultsInUse: () => 0,
    ...overrides,
  };
}

function history(overrides: Partial<EnvironmentDecisionContext> = {}) {
  const events: EnvironmentEvent[] = [];
  const ctx = context(overrides);
  return {
    state: () => foldEnvironment(events),
    run(command: EnvironmentCommand) {
      const decision = decideEnvironment(foldEnvironment(events), command, ctx);
      if (!decision.ok) throw decision.fault;
      events.push(...decision.events);
      return foldEnvironment(events)!;
    },
    decide: (command: EnvironmentCommand) => decideEnvironment(foldEnvironment(events), command, ctx),
  };
}

const create = () =>
  environmentCommands.create({ environmentId: qa, displayName: 'QA', description: 'Regression runs' });

describe('environment decider', () => {
  it('creates a custom environment', () => {
    const environment = history().run(create());
    expect(environment).toMatchObject({
      environmentId: 'qa',
      displayName: 'QA',
      description: 'Regression runs',
      builtIn: false,
      lifecycle: 'ACTIVE',
      revision: 1,
    });
  });

  it('marks the fixed ids as built-in', () => {
    const environment = history().run(
      environmentCommands.create({ environmentId: builtInEnvironmentIds.production, displayName: 'Production' }),
    );
    expect(environment.builtIn).toBe(true);
  });

  it('trims the name and description', () => {
    const environment = history().run(
      environmentCommands.create({ environmentId: qa, displayName: '  QA  ', description: '  runs  ' }),
    );
    expect(environment).toMatchObject({ displayName: 'QA', description: 'runs' });
  });

  it('rejects an empty or oversized name and an oversized description', () => {
    for (const command of [
      environmentCommands.create({ environmentId: qa, displayName: '   ' }),
      environmentCommands.create({ environmentId: qa, displayName: 'x'.repeat(MAX_DISPLAY_NAME_LENGTH + 1) }),
      environmentCommands.create({
        environmentId: qa,
        displayName: 'QA',
        description: 'x'.repeat(MAX_DESCRIPTION_LENGTH + 1),
      }),
    ]) {
      const decision = history().decide(command);
      expect(decision.ok).toBe(false);
      if (!decision.ok) expect(decision.fault.reason).toBe('COMMAND_INVALID');
    }
  });

  it('rejects a duplicate id', () => {
    const flow = history();
    flow.run(create());
    const decision = flow.decide(create());
    expect(decision.ok).toBe(false);
    if (!decision.ok) expect(decision.fault.reason).toBe('ENVIRONMENT_ALREADY_EXISTS');
  });

  it('edits the name and description and keeps the id', () => {
    const flow = history();
    flow.run(create());
    const environment = flow.run(environmentCommands.update(qa, { displayName: 'Quality', description: '' }));
    expect(environment).toMatchObject({ environmentId: 'qa', displayName: 'Quality', description: '', revision: 2 });
  });

  it('rejects an edit that changes nothing', () => {
    const flow = history();
    flow.run(create());
    const decision = flow.decide(environmentCommands.update(qa, { displayName: 'QA', description: 'Regression runs' }));
    expect(decision.ok).toBe(false);
    if (!decision.ok) expect(decision.fault.reason).toBe('COMMAND_INVALID');
  });

  it('rejects an edit of an unknown environment', () => {
    const decision = history().decide(environmentCommands.update(qa, { displayName: 'Quality' }));
    expect(decision.ok).toBe(false);
    if (!decision.ok) expect(decision.fault.reason).toBe('ENVIRONMENT_NOT_FOUND');
  });

  it('deletes an unused custom environment', () => {
    const flow = history();
    flow.run(create());
    expect(flow.run(environmentCommands.delete(qa)).lifecycle).toBe('DELETED');
  });

  it('lets a deleted id be created again', () => {
    const flow = history();
    flow.run(create());
    flow.run(environmentCommands.delete(qa));
    expect(flow.run(create())).toMatchObject({ lifecycle: 'ACTIVE', revision: 3 });
  });

  it('refuses to delete a built-in environment', () => {
    const flow = history();
    flow.run(environmentCommands.create({ environmentId: builtInEnvironmentIds.preview, displayName: 'Preview' }));
    const decision = flow.decide(environmentCommands.delete(builtInEnvironmentIds.preview));
    expect(decision.ok).toBe(false);
    if (!decision.ok) expect(decision.fault.reason).toBe('ENVIRONMENT_BUILT_IN');
  });

  it('refuses to delete an environment that vaults still use', () => {
    const flow = history({ vaultsInUse: () => 2 });
    flow.run(create());
    const decision = flow.decide(environmentCommands.delete(qa));
    expect(decision.ok).toBe(false);
    if (!decision.ok) {
      expect(decision.fault.reason).toBe('ENVIRONMENT_IN_USE');
      expect(decision.fault.detail).toContain('2 vaults');
    }
  });
});
