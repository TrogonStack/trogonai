import { hierarchyCommands } from '../domain/commands';
import { mintNodeId } from '../domain/node-id';
import { mintNodeName } from '../domain/node-name';
import type { HierarchySeedWriter } from './in-memory-client';

const DAY = 86_400_000;

export const seedNodeIds = {
  tenantRoot: mintNodeId('node_tenant_root'),
  workspace: mintNodeId('node_workspace'),
  platform: mintNodeId('node_platform'),
  research: mintNodeId('node_research'),
  payments: mintNodeId('node_payments'),
} as const;

export async function populateHierarchySeed(writer: HierarchySeedWriter, now: Date = new Date()): Promise<void> {
  const at = (days: number) => new Date(now.getTime() - days * DAY);

  await writer.hierarchy(hierarchyCommands.establishRoot(seedNodeIds.tenantRoot), at(260));
  await writer.hierarchy(
    hierarchyCommands.add({ nodeId: seedNodeIds.workspace, parent: seedNodeIds.tenantRoot }),
    at(260),
  );
  await writer.label(seedNodeIds.workspace, mintNodeName('Acme workspace'));
  await writer.hierarchy(
    hierarchyCommands.add({ nodeId: seedNodeIds.platform, parent: seedNodeIds.workspace }),
    at(240),
  );
  await writer.label(seedNodeIds.platform, mintNodeName('Platform folder'));
  await writer.hierarchy(
    hierarchyCommands.add({ nodeId: seedNodeIds.research, parent: seedNodeIds.workspace }),
    at(230),
  );
  await writer.label(seedNodeIds.research, mintNodeName('Research folder'));
  await writer.hierarchy(
    hierarchyCommands.add({ nodeId: seedNodeIds.payments, parent: seedNodeIds.platform }),
    at(100),
  );
  await writer.label(seedNodeIds.payments, mintNodeName('Payments'));
}
