import type { WorkspaceNode, WorkspaceNodeDirectory } from '../client';
import { mintNodeId } from '../domain/node-id';

export const seedNodeIds = {
  tenantRoot: mintNodeId('node_tenant_root'),
  workspace: mintNodeId('node_workspace'),
  platform: mintNodeId('node_platform'),
  research: mintNodeId('node_research'),
} as const;

export const seedWorkspaceNodes: readonly WorkspaceNode[] = [
  { nodeId: seedNodeIds.workspace, displayName: 'Acme workspace', level: 'workspace' },
  { nodeId: seedNodeIds.platform, displayName: 'Platform folder', level: 'folder' },
  { nodeId: seedNodeIds.research, displayName: 'Research folder', level: 'folder' },
];

export const seedNodeDirectory: WorkspaceNodeDirectory = {
  workspaceNodes: () => Promise.resolve(seedWorkspaceNodes),
};
