import type { CreateSchedule, ScheduleProjection } from '@trogonai/proto';

import type { ProjectId } from '@/features/projects/domain/project-id';

export interface SchedulesClient {
  list(): Promise<readonly ScheduleProjection[]>;
  get(id: string): Promise<ScheduleProjection | undefined>;
  create(command: CreateSchedule): Promise<ScheduleProjection>;
  pause(id: string): Promise<void>;
  resume(id: string): Promise<void>;
  remove(id: string): Promise<void>;
}
export interface SchedulesClients {
  forProject(id: ProjectId): SchedulesClient;
}
