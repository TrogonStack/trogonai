import { describe, expect, it } from 'vitest';

import { environmentCommands } from '../domain/commands';
import { mintEnvironmentId } from '../domain/environment-id';
import { EnvironmentFault } from '../domain/faults';
import { createFakeEnvironmentsClient } from './index';
import { seedEnvironmentIds } from './seed';

const loadTesting = mintEnvironmentId('load-testing');

async function rejection(promise: Promise<unknown>): Promise<EnvironmentFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof EnvironmentFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

describe('in-memory environments client', () => {
  it('seeds the built-ins and two custom environments', async () => {
    const client = createFakeEnvironmentsClient();
    const environments = await client.list();
    expect(environments.map((environment) => environment.environmentId)).toEqual([
      'production',
      'preview',
      'development',
      'staging',
      'qa',
    ]);
    expect(environments.filter((environment) => environment.builtIn)).toHaveLength(3);
  });

  it('creates an environment', async () => {
    const client = createFakeEnvironmentsClient();
    const created = await client.dispatch(
      environmentCommands.create({ environmentId: loadTesting, displayName: 'Load testing' }),
    );
    expect(created).toMatchObject({ environmentId: 'load-testing', revision: 1, builtIn: false });
    expect(await client.get('load-testing')).toBeDefined();
  });

  it('rejects a duplicate id', async () => {
    const client = createFakeEnvironmentsClient();
    const fault = await rejection(
      client.dispatch(environmentCommands.create({ environmentId: seedEnvironmentIds.qa, displayName: 'Again' })),
    );
    expect(fault.reason).toBe('ENVIRONMENT_ALREADY_EXISTS');
  });

  it('rejects an invalid id', async () => {
    const client = createFakeEnvironmentsClient();
    const invalid = { ...environmentCommands.create({ environmentId: loadTesting, displayName: 'Bad' }) };
    const fault = await rejection(client.dispatch({ ...invalid, environmentId: 'Bad Id' as typeof loadTesting }));
    expect(fault.reason).toBe('COMMAND_INVALID');
  });

  it('edits the name and description', async () => {
    const client = createFakeEnvironmentsClient();
    const updated = await client.dispatch(
      environmentCommands.update(seedEnvironmentIds.qa, { displayName: 'Quality', description: 'New words' }),
      { expectedRevision: 1 },
    );
    expect(updated).toMatchObject({
      environmentId: 'qa',
      displayName: 'Quality',
      description: 'New words',
      revision: 2,
    });
  });

  it('refuses to delete a built-in environment', async () => {
    const client = createFakeEnvironmentsClient();
    const fault = await rejection(
      client.dispatch(environmentCommands.delete(seedEnvironmentIds.production), { expectedRevision: 1 }),
    );
    expect(fault.reason).toBe('ENVIRONMENT_BUILT_IN');
  });

  it('refuses to delete an environment in use', async () => {
    const client = createFakeEnvironmentsClient({
      usage: { blockingVaults: async (id) => (id === seedEnvironmentIds.staging ? 1 : 0) },
    });
    const fault = await rejection(
      client.dispatch(environmentCommands.delete(seedEnvironmentIds.staging), { expectedRevision: 1 }),
    );
    expect(fault.reason).toBe('ENVIRONMENT_IN_USE');
    expect(await client.get('staging')).toBeDefined();
  });

  it('deletes an unused custom environment and frees the id', async () => {
    const client = createFakeEnvironmentsClient();
    await client.dispatch(environmentCommands.delete(seedEnvironmentIds.qa), { expectedRevision: 1 });
    expect(await client.get('qa')).toBeUndefined();
    expect((await client.list()).map((environment) => environment.environmentId)).not.toContain('qa');
    const again = await client.dispatch(
      environmentCommands.create({ environmentId: seedEnvironmentIds.qa, displayName: 'QA' }),
    );
    expect(again.lifecycle).toBe('ACTIVE');
  });

  it('answers a repeated command id without applying it twice', async () => {
    const client = createFakeEnvironmentsClient();
    const command = environmentCommands.update(seedEnvironmentIds.qa, { displayName: 'Quality' });
    const first = await client.dispatch(command, { expectedRevision: 1, commandId: 'cmd-1' });
    const second = await client.dispatch(command, { expectedRevision: 1, commandId: 'cmd-1' });
    expect(second.revision).toBe(first.revision);
    expect(await client.history('qa')).toHaveLength(2);
  });

  it('rejects a stale revision', async () => {
    const client = createFakeEnvironmentsClient();
    const fault = await rejection(
      client.dispatch(environmentCommands.update(seedEnvironmentIds.qa, { displayName: 'Quality' }), {
        expectedRevision: 5,
      }),
    );
    expect(fault.reason).toBe('REVISION_CONFLICT');
  });

  it('requires an expected revision on existing environments', async () => {
    const client = createFakeEnvironmentsClient();
    const fault = await rejection(
      client.dispatch(environmentCommands.update(seedEnvironmentIds.qa, { displayName: 'Quality' })),
    );
    expect(fault.reason).toBe('COMMAND_INVALID');
  });

  it('reports an unknown environment', async () => {
    const client = createFakeEnvironmentsClient();
    const fault = await rejection(client.dispatch(environmentCommands.delete(loadTesting), { expectedRevision: 1 }));
    expect(fault.reason).toBe('ENVIRONMENT_NOT_FOUND');
  });
});
