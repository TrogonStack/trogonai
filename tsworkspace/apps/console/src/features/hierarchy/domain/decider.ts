import type { HierarchyCommand } from './commands';
import type { HierarchyEvent, HierarchyFact } from './events';
import { HierarchyFault } from './faults';
import type { NodeId } from './node-id';
import {
  MAX_TREE_DEPTH,
  childrenOf,
  depthAfterMove,
  depthOf,
  emptyTree,
  hasNode,
  isBelow,
  parentOf,
  type TreeState,
} from './tree';

export interface HierarchyDecisionContext {
  readonly now: Date;
  newId(prefix: string): string;
  attachedTo(nodeId: NodeId): number;
}

export type HierarchyDecision =
  | { readonly ok: true; readonly events: readonly HierarchyEvent[] }
  | { readonly ok: false; readonly fault: HierarchyFault };

function plural(count: number, singular: string, many: string): string {
  return `${count} ${count === 1 ? singular : many}`;
}

function event(tree: TreeState, context: HierarchyDecisionContext, fact: HierarchyFact): HierarchyEvent {
  return { eventId: context.newId('evt'), revision: tree.revision + 1, occurredAt: context.now, fact };
}

function requireNode(tree: TreeState, nodeId: NodeId): void {
  if (!hasNode(tree, nodeId)) throw new HierarchyFault('NODE_NOT_FOUND');
}

function requireMovable(tree: TreeState, nodeId: NodeId): void {
  requireNode(tree, nodeId);
  if (nodeId === tree.rootId) throw new HierarchyFault('ROOT_IMMUTABLE');
}

function requireDepth(depth: number): void {
  if (depth > MAX_TREE_DEPTH) {
    throw new HierarchyFault('DEPTH_EXCEEDED', `The tree is at most ${MAX_TREE_DEPTH} levels deep, counting the root.`);
  }
}

function decideAdd(
  tree: TreeState,
  command: Extract<HierarchyCommand, { case: 'addNode' }>,
  context: HierarchyDecisionContext,
): readonly HierarchyEvent[] {
  if (hasNode(tree, command.nodeId)) throw new HierarchyFault('NODE_ALREADY_EXISTS');
  const { parent } = command;
  if (tree.rootId === undefined) {
    if (parent !== undefined) throw new HierarchyFault('PARENT_NOT_FOUND', 'The tree has no root yet.');
  } else {
    if (parent === undefined) {
      throw new HierarchyFault(
        'PARENT_NOT_FOUND',
        'Only the first node can be the root. Every other node needs a parent.',
      );
    }
    if (!hasNode(tree, parent)) throw new HierarchyFault('PARENT_NOT_FOUND');
    requireDepth(depthOf(tree, parent) + 1);
  }
  return [event(tree, context, { case: 'nodeAdded', nodeId: command.nodeId, parent })];
}

function decideMove(
  tree: TreeState,
  command: Extract<HierarchyCommand, { case: 'moveNode' }>,
  context: HierarchyDecisionContext,
): readonly HierarchyEvent[] {
  const { nodeId, parent } = command;
  requireMovable(tree, nodeId);
  if (!hasNode(tree, parent)) throw new HierarchyFault('PARENT_NOT_FOUND');
  if (parent === nodeId || isBelow(tree, parent, nodeId)) throw new HierarchyFault('NODE_CYCLE');
  const from = parentOf(tree, nodeId)!;
  if (parent === from) throw new HierarchyFault('COMMAND_INVALID', 'Nothing changed.');
  requireDepth(depthAfterMove(tree, nodeId, parent));
  return [event(tree, context, { case: 'nodeMoved', nodeId, from, to: parent })];
}

function decideRemove(
  tree: TreeState,
  command: Extract<HierarchyCommand, { case: 'removeNode' }>,
  context: HierarchyDecisionContext,
): readonly HierarchyEvent[] {
  const { nodeId } = command;
  requireMovable(tree, nodeId);
  const children = childrenOf(tree, nodeId).length;
  if (children > 0) {
    throw new HierarchyFault(
      'NODE_HAS_CHILDREN',
      `${plural(children, 'child node remains', 'child nodes remain')}. Move or remove ${children === 1 ? 'it' : 'them'} first.`,
    );
  }
  const attached = context.attachedTo(nodeId);
  if (attached > 0) {
    throw new HierarchyFault(
      'NODE_HAS_ATTACHMENTS',
      `${plural(attached, 'resource is', 'resources are')} still attached. Removing a node never deletes or re-homes work.`,
    );
  }
  return [event(tree, context, { case: 'nodeRemoved', nodeId, parent: parentOf(tree, nodeId)! })];
}

function decideOrThrow(
  tree: TreeState,
  command: HierarchyCommand,
  context: HierarchyDecisionContext,
): readonly HierarchyEvent[] {
  switch (command.case) {
    case 'addNode':
      return decideAdd(tree, command, context);
    case 'moveNode':
      return decideMove(tree, command, context);
    case 'removeNode':
      return decideRemove(tree, command, context);
  }
}

export function decideHierarchy(
  tree: TreeState,
  command: HierarchyCommand,
  context: HierarchyDecisionContext,
): HierarchyDecision {
  try {
    return { ok: true, events: decideOrThrow(tree, command, context) };
  } catch (error) {
    if (error instanceof HierarchyFault) return { ok: false, fault: error };
    throw error;
  }
}

export function evolveHierarchy(tree: TreeState, occurred: HierarchyEvent): TreeState {
  const { fact, revision } = occurred;
  const parents = new Map(tree.parents);
  switch (fact.case) {
    case 'nodeAdded':
      parents.set(fact.nodeId, fact.parent);
      return { rootId: fact.parent === undefined ? fact.nodeId : tree.rootId, parents, revision };
    case 'nodeMoved':
      parents.set(fact.nodeId, fact.to);
      return { ...tree, parents, revision };
    case 'nodeRemoved':
      parents.delete(fact.nodeId);
      return { ...tree, parents, revision };
  }
}

export function foldHierarchy(events: readonly HierarchyEvent[]): TreeState {
  return events.reduce(evolveHierarchy, emptyTree);
}
