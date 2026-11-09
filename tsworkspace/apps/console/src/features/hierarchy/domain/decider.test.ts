import { describe, expect, it } from 'vitest';

import { hierarchyCommands, type HierarchyCommand } from './commands';
import { decideHierarchy, evolveHierarchy, foldHierarchy, type HierarchyDecisionContext } from './decider';
import type { HierarchyEvent } from './events';
import type { HierarchyFaultReason } from './faults';
import { mintNodeId } from './node-id';
import { MAX_TREE_DEPTH, depthOf, emptyTree, moveTargets, wordForDepth, type TreeState } from './tree';

const id = mintNodeId;
const root = id('root');
const alpha = id('alpha');
const beta = id('beta');
const gamma = id('gamma');
const delta = id('delta');

const at = new Date('2026-01-01T00:00:00Z');

function context(attached: Record<string, number> = {}): HierarchyDecisionContext {
  let counter = 0;
  return { now: at, newId: (prefix) => `${prefix}_${++counter}`, attachedTo: (nodeId) => attached[nodeId] ?? 0 };
}

function apply(tree: TreeState, command: HierarchyCommand, attached: Record<string, number> = {}): TreeState {
  const decision = decideHierarchy(tree, command, context(attached));
  if (!decision.ok) throw decision.fault;
  return decision.events.reduce(evolveHierarchy, tree);
}

function build(...commands: HierarchyCommand[]): TreeState {
  return commands.reduce((tree, command) => apply(tree, command), emptyTree);
}

function fault(tree: TreeState, command: HierarchyCommand, attached: Record<string, number> = {}) {
  const decision = decideHierarchy(tree, command, context(attached));
  if (decision.ok) throw new Error('The command was accepted.');
  return decision.fault.reason satisfies HierarchyFaultReason;
}

const sample = () =>
  build(
    hierarchyCommands.establishRoot(root),
    hierarchyCommands.add({ nodeId: alpha, parent: root }),
    hierarchyCommands.add({ nodeId: beta, parent: alpha }),
    hierarchyCommands.add({ nodeId: gamma, parent: root }),
  );

describe('hierarchy decider', () => {
  it('establishes the root as the first node', () => {
    const tree = build(hierarchyCommands.establishRoot(root));
    expect(tree.rootId).toBe(root);
    expect(tree.revision).toBe(1);
  });

  it('refuses a second root and a first node with a parent', () => {
    expect(fault(emptyTree, hierarchyCommands.add({ nodeId: alpha, parent: root }))).toBe('PARENT_NOT_FOUND');
    expect(fault(sample(), hierarchyCommands.establishRoot(delta))).toBe('PARENT_NOT_FOUND');
  });

  it('adds a node below an existing parent', () => {
    const tree = sample();
    expect(tree.parents.get(beta)).toBe(alpha);
    expect(tree.revision).toBe(4);
  });

  it('refuses a duplicate id', () => {
    expect(fault(sample(), hierarchyCommands.add({ nodeId: alpha, parent: root }))).toBe('NODE_ALREADY_EXISTS');
  });

  it('refuses a missing parent', () => {
    expect(fault(sample(), hierarchyCommands.add({ nodeId: delta, parent: id('ghost') }))).toBe('PARENT_NOT_FOUND');
  });

  it('caps the depth at ten levels counting the root', () => {
    let tree = build(hierarchyCommands.establishRoot(id('n1')));
    for (let level = 2; level <= MAX_TREE_DEPTH; level += 1) {
      tree = apply(tree, hierarchyCommands.add({ nodeId: id(`n${level}`), parent: id(`n${level - 1}`) }));
    }
    expect(depthOf(tree, id(`n${MAX_TREE_DEPTH}`))).toBe(MAX_TREE_DEPTH);
    expect(fault(tree, hierarchyCommands.add({ nodeId: id('n11'), parent: id(`n${MAX_TREE_DEPTH}`) }))).toBe(
      'DEPTH_EXCEEDED',
    );
  });

  it('derives the word from the depth and never stores it', () => {
    const tree = sample();
    expect(wordForDepth(depthOf(tree, root))).toBe('organization');
    expect(wordForDepth(depthOf(tree, alpha))).toBe('workspace');
    expect(wordForDepth(depthOf(tree, beta))).toBe('folder');
  });

  it('moves a node and re-derives its word', () => {
    const tree = apply(sample(), hierarchyCommands.move({ nodeId: beta, parent: root }));
    expect(tree.parents.get(beta)).toBe(root);
    expect(wordForDepth(depthOf(tree, beta))).toBe('workspace');
  });

  it('refuses to move an unknown node, the root, or to an unknown parent', () => {
    const tree = sample();
    expect(fault(tree, hierarchyCommands.move({ nodeId: delta, parent: root }))).toBe('NODE_NOT_FOUND');
    expect(fault(tree, hierarchyCommands.move({ nodeId: root, parent: alpha }))).toBe('ROOT_IMMUTABLE');
    expect(fault(tree, hierarchyCommands.move({ nodeId: beta, parent: delta }))).toBe('PARENT_NOT_FOUND');
  });

  it('refuses a cycle through itself or a descendant', () => {
    const tree = sample();
    expect(fault(tree, hierarchyCommands.move({ nodeId: alpha, parent: alpha }))).toBe('NODE_CYCLE');
    expect(fault(tree, hierarchyCommands.move({ nodeId: alpha, parent: beta }))).toBe('NODE_CYCLE');
  });

  it('refuses a move that changes nothing', () => {
    expect(fault(sample(), hierarchyCommands.move({ nodeId: beta, parent: alpha }))).toBe('COMMAND_INVALID');
  });

  it('refuses a move that pushes the subtree past the depth cap', () => {
    let tree = build(hierarchyCommands.establishRoot(id('n1')));
    for (let level = 2; level <= MAX_TREE_DEPTH; level += 1) {
      tree = apply(tree, hierarchyCommands.add({ nodeId: id(`n${level}`), parent: id(`n${level - 1}`) }));
    }
    tree = apply(tree, hierarchyCommands.add({ nodeId: id('side'), parent: id('n1') }));
    tree = apply(tree, hierarchyCommands.add({ nodeId: id('leaf'), parent: id('side') }));
    expect(fault(tree, hierarchyCommands.move({ nodeId: id('side'), parent: id(`n${MAX_TREE_DEPTH - 1}`) }))).toBe(
      'DEPTH_EXCEEDED',
    );
    expect(moveTargets(tree, id('side')).map((target) => target.nodeId)).not.toContain(id(`n${MAX_TREE_DEPTH - 1}`));
  });

  it('refuses to remove the root, an unknown node, a node with children or one with attachments', () => {
    const tree = sample();
    expect(fault(tree, hierarchyCommands.remove(root))).toBe('ROOT_IMMUTABLE');
    expect(fault(tree, hierarchyCommands.remove(delta))).toBe('NODE_NOT_FOUND');
    expect(fault(tree, hierarchyCommands.remove(alpha))).toBe('NODE_HAS_CHILDREN');
    expect(fault(tree, hierarchyCommands.remove(gamma), { gamma: 2 })).toBe('NODE_HAS_ATTACHMENTS');
  });

  it('removes an empty node', () => {
    const tree = apply(sample(), hierarchyCommands.remove(gamma));
    expect(tree.parents.has(gamma)).toBe(false);
  });

  it('numbers events by the single tree revision', () => {
    const decision = decideHierarchy(sample(), hierarchyCommands.add({ nodeId: delta, parent: root }), context());
    expect(decision.ok && decision.events.map((event) => event.revision)).toEqual([5]);
  });

  it('folds the same state from history as from commands', () => {
    const events: HierarchyEvent[] = [];
    let tree = emptyTree;
    for (const command of [
      hierarchyCommands.establishRoot(root),
      hierarchyCommands.add({ nodeId: alpha, parent: root }),
      hierarchyCommands.add({ nodeId: beta, parent: root }),
      hierarchyCommands.move({ nodeId: beta, parent: alpha }),
    ]) {
      const decision = decideHierarchy(tree, command, context());
      if (!decision.ok) throw decision.fault;
      events.push(...decision.events);
      tree = foldHierarchy(events);
    }
    expect(foldHierarchy(events).parents.get(beta)).toBe(alpha);
    expect(foldHierarchy(events).revision).toBe(4);
  });
});
