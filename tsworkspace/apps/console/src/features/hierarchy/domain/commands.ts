import type { NodeId } from './node-id';

export type HierarchyCommand =
  | { readonly case: 'addNode'; readonly nodeId: NodeId; readonly parent: NodeId | undefined }
  | { readonly case: 'moveNode'; readonly nodeId: NodeId; readonly parent: NodeId }
  | { readonly case: 'removeNode'; readonly nodeId: NodeId };

export const hierarchyCommands = {
  establishRoot: (nodeId: NodeId): HierarchyCommand => ({ case: 'addNode', nodeId, parent: undefined }),
  add: (input: { nodeId: NodeId; parent: NodeId }): HierarchyCommand => ({
    case: 'addNode',
    nodeId: input.nodeId,
    parent: input.parent,
  }),
  move: (input: { nodeId: NodeId; parent: NodeId }): HierarchyCommand => ({
    case: 'moveNode',
    nodeId: input.nodeId,
    parent: input.parent,
  }),
  remove: (nodeId: NodeId): HierarchyCommand => ({ case: 'removeNode', nodeId }),
};
