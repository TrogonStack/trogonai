import { InMemorySourcesClient, type InMemoryClientOptions } from './in-memory-client';
import { populateSeed } from './seed';

export function createFakeSourcesClient(
  options: InMemoryClientOptions & {
    seeded?: boolean;
    populate?: typeof populateSeed;
  } = {},
): InMemorySourcesClient {
  const client = new InMemorySourcesClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    const populate = options.populate ?? populateSeed;
    client.seedWith(() => populate(clock()));
  }
  return client;
}
