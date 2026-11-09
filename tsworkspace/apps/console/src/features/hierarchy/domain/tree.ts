import type { NodeId } from './node-id';

export const MAX_TREE_DEPTH = 10;

export type NodeWord = 'organization' | 'workspace' | 'folder';

export const nodeWordLabels: Record<NodeWord, string> = {
  organization: 'Organization',
  workspace: 'Workspace',
  folder: 'Folder',
};

export interface TreeState {
  readonly rootId: NodeId | undefined;
  readonly parents: ReadonlyMap<NodeId, NodeId | undefined>;
  readonly revision: number;
}

export const emptyTree: TreeState = { rootId: undefined, parents: new Map(), revision: 0 };

export function wordForDepth(depth: number): NodeWord {
  if (depth <= 1) return 'organization';
  return depth === 2 ? 'workspace' : 'folder';
}

export function hasNode(tree: TreeState, nodeId: NodeId): boolean {
  return tree.parents.has(nodeId);
}

export function parentOf(tree: TreeState, nodeId: NodeId): NodeId | undefined {
  return tree.parents.get(nodeId);
}

export function childrenOf(tree: TreeState, nodeId: NodeId): readonly NodeId[] {
  return [...tree.parents].filter(([, parent]) => parent === nodeId).map(([child]) => child);
}

export function pathTo(tree: TreeState, nodeId: NodeId): readonly NodeId[] {
  const path: NodeId[] = [];
  for (let current: NodeId | undefined = nodeId; current !== undefined; current = tree.parents.get(current)) {
    path.unshift(current);
  }
  return path;
}

export function depthOf(tree: TreeState, nodeId: NodeId): number {
  return pathTo(tree, nodeId).length;
}

export function descendantsOf(tree: TreeState, nodeId: NodeId): readonly NodeId[] {
  const found: NodeId[] = [];
  const pending = [...childrenOf(tree, nodeId)];
  for (let next = pending.shift(); next !== undefined; next = pending.shift()) {
    found.push(next);
    pending.push(...childrenOf(tree, next));
  }
  return found;
}

export function walk(tree: TreeState): readonly NodeId[] {
  if (tree.rootId === undefined) return [];
  const ordered: NodeId[] = [];
  const visit = (nodeId: NodeId) => {
    ordered.push(nodeId);
    childrenOf(tree, nodeId).forEach(visit);
  };
  visit(tree.rootId);
  return ordered;
}

export function subtreeHeight(tree: TreeState, nodeId: NodeId): number {
  const below = childrenOf(tree, nodeId).map((child) => subtreeHeight(tree, child));
  return 1 + Math.max(0, ...below);
}

export function isBelow(tree: TreeState, nodeId: NodeId, ancestor: NodeId): boolean {
  return pathTo(tree, nodeId).slice(0, -1).includes(ancestor);
}

export function depthAfterMove(tree: TreeState, nodeId: NodeId, newParent: NodeId): number {
  return depthOf(tree, newParent) + subtreeHeight(tree, nodeId);
}

export interface MoveTarget {
  readonly nodeId: NodeId;
  readonly becomes: NodeWord;
}

export function moveTargets(tree: TreeState, nodeId: NodeId): readonly MoveTarget[] {
  if (nodeId === tree.rootId || !hasNode(tree, nodeId)) return [];
  const current = parentOf(tree, nodeId);
  return walk(tree)
    .filter(
      (candidate) =>
        candidate !== nodeId &&
        candidate !== current &&
        !isBelow(tree, candidate, nodeId) &&
        depthAfterMove(tree, nodeId, candidate) <= MAX_TREE_DEPTH,
    )
    .map((candidate) => ({ nodeId: candidate, becomes: wordForDepth(depthOf(tree, candidate) + 1) }));
}
