import type { NodeLabelEntry } from '../client';
import type { NodeId } from './node-id';
import {
  childrenOf,
  depthOf,
  descendantsOf,
  moveTargets,
  parentOf,
  pathTo,
  walk,
  wordForDepth,
  type MoveTarget,
  type NodeWord,
  type TreeState,
} from './tree';

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

export interface NodeView {
  readonly nodeId: NodeId;
  readonly name: string;
  readonly parent: NodeId | undefined;
  readonly path: readonly NodeId[];
  readonly depth: number;
  readonly word: NodeWord;
  readonly children: readonly NodeId[];
  readonly descendants: number;
  readonly attached: number;
  readonly attachedBelow: number;
  readonly moveTargets: readonly MoveTarget[];
  readonly actions: {
    readonly rename: ActionAvailability;
    readonly move: ActionAvailability;
    readonly remove: ActionAvailability;
  };
}

export interface OrganizationView {
  readonly name: string | undefined;
  readonly revealed: boolean;
}

export interface HierarchyView {
  readonly revision: number;
  readonly rootId: NodeId | undefined;
  readonly organization: OrganizationView;
  readonly nodes: readonly NodeView[];
  readonly workspaces: readonly NodeView[];
  readonly byId: ReadonlyMap<NodeId, NodeView>;
}

export const ORGANIZATION_FALLBACK_NAME = 'Organization';

function plural(count: number, singular: string, many: string): string {
  return `${count} ${count === 1 ? singular : many}`;
}

function removal(isRoot: boolean, children: number, attached: number): ActionAvailability {
  if (isRoot) return { available: false, reason: 'The root can be neither moved nor removed.' };
  const reasons = [
    children > 0 ? `${plural(children, 'child node is', 'child nodes are')} still below it.` : undefined,
    attached > 0
      ? `${plural(attached, 'project is', 'projects are')} still attached. Removing never deletes or re-homes work.`
      : undefined,
  ].filter((reason) => reason !== undefined);
  return reasons.length > 0 ? { available: false, reason: reasons.join(' ') } : { available: true };
}

function movement(isRoot: boolean, targets: readonly MoveTarget[]): ActionAvailability {
  if (isRoot) return { available: false, reason: 'The root can be neither moved nor removed.' };
  if (targets.length === 0) {
    return { available: false, reason: 'No other node can take it without a cycle or exceeding the depth limit.' };
  }
  return { available: true };
}

export function buildHierarchyView(
  tree: TreeState,
  labels: readonly NodeLabelEntry[],
  attachedCounts: ReadonlyMap<NodeId, number>,
): HierarchyView {
  const names = new Map(labels.map((entry) => [entry.nodeId, entry.name as string]));
  const attachedTo = (nodeId: NodeId) => attachedCounts.get(nodeId) ?? 0;

  const nodes = walk(tree).map<NodeView>((nodeId) => {
    const isRoot = nodeId === tree.rootId;
    const depth = depthOf(tree, nodeId);
    const children = childrenOf(tree, nodeId);
    const below = descendantsOf(tree, nodeId);
    const targets = moveTargets(tree, nodeId);
    const attached = attachedTo(nodeId);
    return {
      nodeId,
      name: names.get(nodeId) ?? (isRoot ? ORGANIZATION_FALLBACK_NAME : nodeId),
      parent: parentOf(tree, nodeId),
      path: pathTo(tree, nodeId),
      depth,
      word: wordForDepth(depth),
      children,
      descendants: below.length,
      attached,
      attachedBelow: below.reduce((sum, descendant) => sum + attachedTo(descendant), attached),
      moveTargets: targets,
      actions: {
        rename: { available: true },
        move: movement(isRoot, targets),
        remove: removal(isRoot, children.length, attached),
      },
    };
  });

  const workspaces = nodes.filter((node) => node.word === 'workspace');
  const root = nodes.find((node) => node.nodeId === tree.rootId);
  return {
    revision: tree.revision,
    rootId: tree.rootId,
    organization: {
      name: root ? names.get(root.nodeId) : undefined,
      revealed: workspaces.length >= 2,
    },
    nodes,
    workspaces,
    byId: new Map(nodes.map((node) => [node.nodeId, node])),
  };
}

export function organizationNameDefault(view: HierarchyView): string {
  return view.organization.name ?? view.workspaces[0]?.name ?? '';
}

export function revealsOrganization(view: HierarchyView, parent: NodeId | undefined): boolean {
  return parent !== undefined && parent === view.rootId && view.workspaces.length === 1;
}

export function organizationLabel(view: HierarchyView): string {
  return view.organization.revealed && view.organization.name ? view.organization.name : 'the organization';
}

export function nameOf(view: HierarchyView, nodeId: NodeId): string {
  if (nodeId === view.rootId) return organizationLabel(view);
  return view.byId.get(nodeId)?.name ?? nodeId;
}
