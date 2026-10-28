import { InMemorySecretsClient, type InMemoryClientOptions, type SeedWriter } from './in-memory-client';
import { populateSeed } from './seed';

export function createFakeSecretsClient(
  options: InMemoryClientOptions & {
    seeded?: boolean;
    populate?: (writer: SeedWriter, now: Date) => Promise<void>;
  } = {},
): InMemorySecretsClient {
  const client = new InMemorySecretsClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    const populate = options.populate ?? populateSeed;
    client.seedWith((writer) => populate(writer, clock()));
  }
  return client;
}
