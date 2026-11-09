import { InMemoryProviderConnectionsClient, type InMemoryClientOptions } from './in-memory-client';
import { populateSeed } from './seed';

export function createFakeProviderConnectionsClient(
  options: InMemoryClientOptions & { seeded?: boolean } = {},
): InMemoryProviderConnectionsClient {
  const client = new InMemoryProviderConnectionsClient(options);
  if (options.seeded ?? true) client.seedWith((writer) => populateSeed(writer));
  return client;
}
