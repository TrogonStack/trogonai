import type { HierarchyCommand } from './domain/commands';
import type { HierarchyEvent } from './domain/events';
import type { NodeId } from './domain/node-id';
import type { NodeName } from './domain/node-name';
import type { TreeState } from './domain/tree';

export interface StoredHierarchyEvent {
  readonly revision: number;
  readonly event: HierarchyEvent;
  readonly occurredAt: Date;
  readonly actor: string;
  readonly commandId: string;
}

export interface DispatchOptions {
  readonly expectedRevision?: number;
  readonly commandId?: string;
}

export interface NodeLabelEntry {
  readonly nodeId: NodeId;
  readonly name: NodeName;
}

export interface LabelDirectory {
  entries(): Promise<readonly NodeLabelEntry[]>;
  set(nodeId: NodeId, name: NodeName): Promise<void>;
}

export interface NodeAttachments {
  attachedTo(nodeId: NodeId): Promise<number>;
}

export interface HierarchyClient {
  tree(): Promise<TreeState>;
  history(): Promise<readonly StoredHierarchyEvent[]>;
  labels(): Promise<readonly NodeLabelEntry[]>;
  setLabel(nodeId: NodeId, name: NodeName): Promise<void>;
  dispatch(command: HierarchyCommand, options?: DispatchOptions): Promise<TreeState>;
}
