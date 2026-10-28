import type { WorkspaceNodeDirectory } from '@/features/projects/client';

import type { HierarchyClient } from '../client';
import { depthOf, walk } from '../domain/tree';

export function workspaceNodeDirectory(hierarchy: HierarchyClient): WorkspaceNodeDirectory {
  return {
    async workspaceNodes() {
      const [tree, labels] = await Promise.all([hierarchy.tree(), hierarchy.labels()]);
      const names = new Map(labels.map((entry) => [entry.nodeId, entry.name as string]));
      return walk(tree)
        .filter((nodeId) => nodeId !== tree.rootId)
        .map((nodeId) => ({
          nodeId,
          displayName: names.get(nodeId) ?? nodeId,
          level: depthOf(tree, nodeId) === 2 ? ('workspace' as const) : ('folder' as const),
        }));
    },
  };
}
