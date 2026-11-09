import { ProjectAccessPreset } from '@trogonai/proto';

import { seedNodeIds } from '@/features/hierarchy/fake/seed';

import { projectCommands } from '../domain/commands';
import { mintProjectId } from '../domain/project-id';
import type { ProjectSeedWriter } from './in-memory-client';

const DAY = 86_400_000;

export const seedProjectIds = {
  demo: mintProjectId('proj-demo'),
  beacon: mintProjectId('proj-beacon'),
  atlas: mintProjectId('proj-atlas'),
} as const;

export async function populateProjectSeed(writer: ProjectSeedWriter, now: Date = new Date()): Promise<void> {
  const at = (days: number) => new Date(now.getTime() - days * DAY);

  await writer.dispatch(
    projectCommands.create({
      projectId: seedProjectIds.atlas,
      displayName: 'Atlas migration',
      goal: 'Move the legacy tenants off the old billing stack.',
      parent: seedNodeIds.research,
      initialAccessPreset: ProjectAccessPreset.INHERITED,
    }),
    at(200),
  );
  await writer.dispatch(
    projectCommands.create({
      projectId: seedProjectIds.demo,
      displayName: 'Demo portal',
      goal: 'Ship the customer portal.',
      parent: seedNodeIds.platform,
      initialAccessPreset: ProjectAccessPreset.INHERITED,
    }),
    at(120),
  );
  await writer.dispatch(
    projectCommands.restateGoal(seedProjectIds.demo, 'Ship the customer portal and keep its credentials rotated.'),
    at(60),
  );
  await writer.dispatch(projectCommands.rename(seedProjectIds.demo, 'Customer portal'), at(59));
  await writer.dispatch(
    projectCommands.create({
      projectId: seedProjectIds.beacon,
      displayName: 'Beacon',
      goal: 'Stand up the edge status beacon for partner integrations.',
      parent: seedNodeIds.workspace,
      initialAccessPreset: ProjectAccessPreset.MEMBERS_ONLY,
    }),
    at(45),
  );
  await writer.dispatch(projectCommands.archive(seedProjectIds.atlas), at(30));
}
