import type { LabelDirectory, NodeLabelEntry } from '../client';
import type { NodeId } from '../domain/node-id';
import type { NodeName } from '../domain/node-name';

export class InMemoryLabelDirectory implements LabelDirectory {
  private readonly names = new Map<NodeId, NodeName>();

  entries(): Promise<readonly NodeLabelEntry[]> {
    return Promise.resolve([...this.names].map(([nodeId, name]) => ({ nodeId, name })));
  }

  set(nodeId: NodeId, name: NodeName): Promise<void> {
    this.names.set(nodeId, name);
    return Promise.resolve();
  }
}
