import type { NodeId } from './node-id';

export type HierarchyFact =
  | { readonly case: 'nodeAdded'; readonly nodeId: NodeId; readonly parent: NodeId | undefined }
  | { readonly case: 'nodeMoved'; readonly nodeId: NodeId; readonly from: NodeId; readonly to: NodeId }
  | { readonly case: 'nodeRemoved'; readonly nodeId: NodeId; readonly parent: NodeId };

export interface HierarchyEvent {
  readonly eventId: string;
  readonly revision: number;
  readonly occurredAt: Date;
  readonly fact: HierarchyFact;
}

export function factTouches(fact: HierarchyFact, nodeId: NodeId): boolean {
  switch (fact.case) {
    case 'nodeAdded':
      return fact.nodeId === nodeId || fact.parent === nodeId;
    case 'nodeMoved':
      return fact.nodeId === nodeId || fact.from === nodeId || fact.to === nodeId;
    case 'nodeRemoved':
      return fact.nodeId === nodeId || fact.parent === nodeId;
  }
}
