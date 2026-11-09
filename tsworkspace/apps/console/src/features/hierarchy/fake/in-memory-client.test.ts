import { describe, expect, it } from 'vitest';

import { hierarchyCommands } from '../domain/commands';
import { HierarchyFault } from '../domain/faults';
import { mintNodeId } from '../domain/node-id';
import { mintNodeName } from '../domain/node-name';
import { childrenOf, depthOf } from '../domain/tree';
import { createFakeHierarchyClient } from './index';
import { seedNodeIds } from './seed';
import { workspaceNodeDirectory } from './workspace-nodes';

const fresh = mintNodeId('fresh');

async function rejection(promise: Promise<unknown>): Promise<HierarchyFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof HierarchyFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

describe('in-memory hierarchy client', () => {
  it('seeds one workspace with two folders and an empty nested folder', async () => {
    const client = createFakeHierarchyClient();
    const tree = await client.tree();
    expect(tree.rootId).toBe(seedNodeIds.tenantRoot);
    expect(childrenOf(tree, seedNodeIds.tenantRoot)).toEqual([seedNodeIds.workspace]);
    expect(childrenOf(tree, seedNodeIds.workspace)).toEqual([seedNodeIds.platform, seedNodeIds.research]);
    expect(childrenOf(tree, seedNodeIds.platform)).toEqual([seedNodeIds.payments]);
    expect(depthOf(tree, seedNodeIds.payments)).toBe(4);
    expect((await client.history()).every((stored) => stored.occurredAt.getTime() < Date.now())).toBe(true);
  });

  it('keeps names in a label directory apart from the tree', async () => {
    const client = createFakeHierarchyClient();
    const labels = await client.labels();
    expect(labels.find((entry) => entry.nodeId === seedNodeIds.workspace)?.name).toBe('Acme workspace');
    await client.setLabel(seedNodeIds.workspace, mintNodeName('Renamed'));
    expect((await client.labels()).find((entry) => entry.nodeId === seedNodeIds.workspace)?.name).toBe('Renamed');
    expect((await client.tree()).revision).toBe(5);
  });

  it('refuses to label an unknown node', async () => {
    const client = createFakeHierarchyClient();
    expect((await rejection(client.setLabel(fresh, mintNodeName('Ghost')))).reason).toBe('NODE_NOT_FOUND');
  });

  it('requires the expected revision', async () => {
    const client = createFakeHierarchyClient();
    const command = hierarchyCommands.add({ nodeId: fresh, parent: seedNodeIds.workspace });
    expect((await rejection(client.dispatch(command))).reason).toBe('COMMAND_INVALID');
  });

  it('refuses a stale revision with a conflict', async () => {
    const client = createFakeHierarchyClient();
    const { revision } = await client.tree();
    await client.dispatch(hierarchyCommands.add({ nodeId: fresh, parent: seedNodeIds.workspace }), {
      expectedRevision: revision,
    });
    const stale = client.dispatch(
      hierarchyCommands.add({ nodeId: mintNodeId('other'), parent: seedNodeIds.workspace }),
      {
        expectedRevision: revision,
      },
    );
    expect((await rejection(stale)).reason).toBe('REVISION_CONFLICT');
  });

  it('answers a repeated command id with the same tree and appends nothing', async () => {
    const client = createFakeHierarchyClient();
    const { revision } = await client.tree();
    const command = hierarchyCommands.add({ nodeId: fresh, parent: seedNodeIds.workspace });
    const first = await client.dispatch(command, { expectedRevision: revision, commandId: 'once' });
    const second = await client.dispatch(command, { expectedRevision: revision, commandId: 'once' });
    expect(second.revision).toBe(first.revision);
    expect((await client.history()).filter((stored) => stored.commandId === 'once')).toHaveLength(1);
  });

  it('surfaces the decider faults', async () => {
    const client = createFakeHierarchyClient();
    const { revision } = await client.tree();
    const cycle = client.dispatch(
      hierarchyCommands.move({ nodeId: seedNodeIds.platform, parent: seedNodeIds.payments }),
      {
        expectedRevision: revision,
      },
    );
    expect((await rejection(cycle)).reason).toBe('NODE_CYCLE');
    const root = client.dispatch(hierarchyCommands.remove(seedNodeIds.tenantRoot), { expectedRevision: revision });
    expect((await rejection(root)).reason).toBe('ROOT_IMMUTABLE');
  });

  it('asks the attachments port before removing a node', async () => {
    const client = createFakeHierarchyClient({
      attachments: { attachedTo: (nodeId) => Promise.resolve(nodeId === seedNodeIds.payments ? 1 : 0) },
    });
    const { revision } = await client.tree();
    const removal = client.dispatch(hierarchyCommands.remove(seedNodeIds.payments), { expectedRevision: revision });
    expect((await rejection(removal)).reason).toBe('NODE_HAS_ATTACHMENTS');
  });

  it('offers every node below the root as a project parent, in tree order', async () => {
    const nodes = await workspaceNodeDirectory(createFakeHierarchyClient()).workspaceNodes();
    expect(nodes.map((node) => [node.nodeId, node.level])).toEqual([
      [seedNodeIds.workspace, 'workspace'],
      [seedNodeIds.platform, 'folder'],
      [seedNodeIds.payments, 'folder'],
      [seedNodeIds.research, 'folder'],
    ]);
    expect(nodes.map((node) => node.nodeId)).not.toContain(seedNodeIds.tenantRoot);
  });
});
