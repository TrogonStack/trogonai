import { environmentCommands } from '../domain/commands';
import { builtInEnvironmentIds, mintEnvironmentId, type EnvironmentId } from '../domain/environment-id';
import type { EnvironmentSeedWriter } from './in-memory-client';

const DAY = 86_400_000;

export const seedEnvironmentIds = {
  production: builtInEnvironmentIds.production,
  preview: builtInEnvironmentIds.preview,
  development: builtInEnvironmentIds.development,
  staging: mintEnvironmentId('staging'),
  qa: mintEnvironmentId('qa'),
} as const;

interface SeedEntry {
  readonly environmentId: EnvironmentId;
  readonly displayName: string;
  readonly description: string;
  readonly age: number;
}

function builtInEntries(age: number): readonly SeedEntry[] {
  return [
    {
      environmentId: seedEnvironmentIds.production,
      displayName: 'Production',
      description: 'Live traffic. Credentials here reach real customers and real bills.',
      age,
    },
    {
      environmentId: seedEnvironmentIds.preview,
      displayName: 'Preview',
      description: 'Short-lived deployments for review before anything ships.',
      age,
    },
    {
      environmentId: seedEnvironmentIds.development,
      displayName: 'Development',
      description: 'Local and shared development work. Safe to break.',
      age,
    },
  ];
}

async function write(writer: EnvironmentSeedWriter, now: Date, entries: readonly SeedEntry[]): Promise<void> {
  for (const entry of entries) {
    await writer.environment(
      environmentCommands.create({
        environmentId: entry.environmentId,
        displayName: entry.displayName,
        description: entry.description,
      }),
      new Date(now.getTime() - entry.age * DAY),
    );
  }
}

export async function populateBuiltInEnvironmentSeed(
  writer: EnvironmentSeedWriter,
  now: Date = new Date(),
): Promise<void> {
  await write(writer, now, builtInEntries(45));
}

export async function populateEnvironmentSeed(writer: EnvironmentSeedWriter, now: Date = new Date()): Promise<void> {
  await write(writer, now, [
    ...builtInEntries(120),
    {
      environmentId: seedEnvironmentIds.staging,
      displayName: 'Staging',
      description: 'A production lookalike for the final dress rehearsal.',
      age: 70,
    },
    {
      environmentId: seedEnvironmentIds.qa,
      displayName: 'QA',
      description: 'Scripted regression runs. Nothing lives here yet.',
      age: 20,
    },
  ]);
}
