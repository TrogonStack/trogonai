import { ProjectAccessPreset, ProjectLifecycle } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { mintNodeId } from '@/features/hierarchy/domain/node-id';
import { createFakeHierarchyClient } from '@/features/hierarchy/fake';
import { seedNodeIds } from '@/features/hierarchy/fake/seed';
import { workspaceNodeDirectory } from '@/features/hierarchy/fake/workspace-nodes';

import { projectCommands } from '../domain/commands';
import { ProjectFault } from '../domain/faults';
import { mintProjectId } from '../domain/project-id';
import { createFakeProjectsClient } from './index';
import { seedProjectIds } from './seed';

const seedNodeDirectory = () => workspaceNodeDirectory(createFakeHierarchyClient());

const fresh = mintProjectId('fresh');

const newProject = () =>
  projectCommands.create({
    projectId: fresh,
    displayName: 'Fresh',
    goal: 'Try things.',
    parent: seedNodeIds.workspace,
    initialAccessPreset: ProjectAccessPreset.MEMBERS_ONLY,
  });

async function rejection(promise: Promise<unknown>): Promise<ProjectFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof ProjectFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

describe('in-memory projects client', () => {
  it('seeds two active projects and one archived project', async () => {
    const client = createFakeProjectsClient();
    const records = await client.list();
    expect(records.map((record) => record.project.projectId?.value).sort()).toEqual([
      'proj-atlas',
      'proj-beacon',
      'proj-demo',
    ]);
    const lifecycles = Object.fromEntries(
      records.map((record) => [record.project.projectId?.value, record.project.lifecycle]),
    );
    expect(lifecycles['proj-atlas']).toBe(ProjectLifecycle.ARCHIVED);
    expect(lifecycles['proj-demo']).toBe(ProjectLifecycle.ACTIVE);
  });

  it('creates a project at revision one without an expected revision', async () => {
    const client = createFakeProjectsClient({ seeded: false, nodes: seedNodeDirectory() });
    const record = await client.dispatch(newProject());
    expect(record.revision).toBe(1);
    expect(record.project.initialAccessPreset).toBe(ProjectAccessPreset.MEMBERS_ONLY);
  });

  it('rejects a duplicate id', async () => {
    const client = createFakeProjectsClient({ seeded: false, nodes: seedNodeDirectory() });
    await client.dispatch(newProject());
    expect((await rejection(client.dispatch(newProject()))).reason).toBe('PROJECT_ALREADY_EXISTS');
  });

  it('rejects a parent outside the workspace, including the tenant root', async () => {
    const client = createFakeProjectsClient({ seeded: false, nodes: seedNodeDirectory() });
    const command = (parent: string) =>
      projectCommands.create({
        projectId: fresh,
        displayName: 'Fresh',
        goal: 'Try things.',
        parent: mintNodeId(parent),
        initialAccessPreset: ProjectAccessPreset.INHERITED,
      });
    expect((await rejection(client.dispatch(command(seedNodeIds.tenantRoot)))).reason).toBe('PROJECT_COMMAND_INVALID');
    expect((await rejection(client.dispatch(command('node_unknown')))).reason).toBe('PROJECT_COMMAND_INVALID');
  });

  it('requires and checks the expected revision on later commands', async () => {
    const client = createFakeProjectsClient({ seeded: false, nodes: seedNodeDirectory() });
    await client.dispatch(newProject());
    const rename = projectCommands.rename(fresh, 'Renamed');
    expect((await rejection(client.dispatch(rename))).reason).toBe('EXPECTED_REVISION_REQUIRED');
    expect((await rejection(client.dispatch(rename, { expectedRevision: 5 }))).reason).toBe('REVISION_MISMATCH');
    const renamed = await client.dispatch(rename, { expectedRevision: 1 });
    expect(renamed).toMatchObject({ revision: 2 });
    expect(renamed.project.displayName).toBe('Renamed');
  });

  it('applies a command id once', async () => {
    const client = createFakeProjectsClient({ seeded: false, nodes: seedNodeDirectory() });
    await client.dispatch(newProject());
    const archive = projectCommands.archive(fresh);
    const first = await client.dispatch(archive, { expectedRevision: 1, commandId: 'cmd-1' });
    const again = await client.dispatch(archive, { expectedRevision: 1, commandId: 'cmd-1' });
    expect(again.revision).toBe(first.revision);
    expect(await client.history(fresh)).toHaveLength(2);
  });

  it('keeps a history in revision order', async () => {
    const client = createFakeProjectsClient();
    const history = await client.history(seedProjectIds.demo);
    expect(history.map((stored) => stored.revision)).toEqual([1, 2, 3]);
    expect(history.map((stored) => stored.event.event.case)).toEqual([
      'projectCreated',
      'projectGoalRestated',
      'projectRenamed',
    ]);
  });

  it('lists parent candidates without the tenant root', async () => {
    const client = createFakeProjectsClient({ nodes: seedNodeDirectory() });
    const nodes = await client.parentCandidates();
    expect(nodes.map((node) => node.nodeId)).not.toContain(seedNodeIds.tenantRoot);
    expect(nodes.length).toBeGreaterThan(0);
  });

  it('counts every project attached to a node, archived ones included', async () => {
    const client = createFakeProjectsClient();
    expect(await client.attachedTo(seedNodeIds.research)).toBe(1);
    expect(await client.attachedTo(seedNodeIds.platform)).toBe(1);
    expect(await client.attachedTo(seedNodeIds.payments)).toBe(0);
  });
});
