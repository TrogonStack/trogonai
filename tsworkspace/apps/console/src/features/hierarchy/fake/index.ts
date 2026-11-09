import { InMemoryHierarchyClient, type InMemoryHierarchyOptions } from './in-memory-client';
import { populateHierarchySeed } from './seed';

export function createFakeHierarchyClient(
  options: InMemoryHierarchyOptions & { seeded?: boolean } = {},
): InMemoryHierarchyClient {
  const client = new InMemoryHierarchyClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    client.seedWith((writer) => populateHierarchySeed(writer, clock()));
  }
  return client;
}
