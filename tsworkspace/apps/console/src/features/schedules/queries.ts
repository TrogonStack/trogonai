import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { CreateSchedule } from '@trogonai/proto';

import { useCurrentProjectId } from '@/features/projects/current-project';

import { useSchedulesClient } from './context';
const root = ['schedules'] as const;
export function useSchedulesQuery() {
  const client = useSchedulesClient();
  const project = useCurrentProjectId();
  return useQuery({
    queryKey: [...root, project, 'list'],
    enabled: project !== undefined,
    queryFn: () => client.list(),
  });
}
export function useScheduleQuery(id: string) {
  const client = useSchedulesClient();
  const project = useCurrentProjectId();
  return useQuery({
    queryKey: [...root, project, 'detail', id],
    enabled: project !== undefined,
    queryFn: async () => (await client.get(id)) ?? null,
  });
}
export function useCreateSchedule() {
  const client = useSchedulesClient();
  const cache = useQueryClient();
  return useMutation({
    mutationFn: (command: CreateSchedule) => client.create(command),
    onSuccess: () => cache.invalidateQueries({ queryKey: root }),
  });
}
export function useScheduleAction() {
  const client = useSchedulesClient();
  const cache = useQueryClient();
  return useMutation({
    mutationFn: ({ action, id }: { action: 'pause' | 'resume' | 'remove'; id: string }) => client[action](id),
    onSuccess: () => cache.invalidateQueries({ queryKey: root }),
  });
}
