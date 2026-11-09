import {
  InMemoryEnvironmentsClient,
  type EnvironmentSeedWriter,
  type InMemoryEnvironmentsOptions,
} from './in-memory-client';
import { populateEnvironmentSeed } from './seed';

export function createFakeEnvironmentsClient(
  options: InMemoryEnvironmentsOptions & {
    seeded?: boolean;
    populate?: (writer: EnvironmentSeedWriter, now: Date) => Promise<void>;
  } = {},
): InMemoryEnvironmentsClient {
  const client = new InMemoryEnvironmentsClient(options);
  if (options.seeded ?? true) {
    const clock = options.clock ?? (() => new Date());
    const populate = options.populate ?? populateEnvironmentSeed;
    client.seedWith((writer) => populate(writer, clock()));
  }
  return client;
}
