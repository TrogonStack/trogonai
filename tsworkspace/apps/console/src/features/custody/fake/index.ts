import { InMemoryTenantCustodyClient, type InMemoryTenantCustodyOptions } from './in-memory-client';
import { populateCustodySeed } from './seed';

export { fakePurposeCatalogue } from './purpose-catalogue';
export { InMemoryTenantCustodyClient } from './in-memory-client';

export function createFakeTenantCustodyClient(
  options: InMemoryTenantCustodyOptions & { seeded?: boolean } = {},
): InMemoryTenantCustodyClient {
  const client = new InMemoryTenantCustodyClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    client.seedWith(() => populateCustodySeed(clock()));
  }
  return client;
}
