import { InMemoryProjectsClient, type InMemoryProjectsOptions } from './in-memory-client';
import { populateProjectSeed } from './seed';

export function createFakeProjectsClient(
  options: InMemoryProjectsOptions & { seeded?: boolean } = {},
): InMemoryProjectsClient {
  const client = new InMemoryProjectsClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    client.seedWith((writer) => populateProjectSeed(writer, clock()));
  }
  return client;
}
