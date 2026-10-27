import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';

import { useCurrentProjectId } from '@/features/projects/current-project';
import type { ProjectId } from '@/features/projects/domain/project-id';

import { useEnvironmentsClient } from './context';
import { environmentCommands } from './domain/commands';
import type { EnvironmentState } from './domain/environment';
import type { EnvironmentId } from './domain/environment-id';

const root = ['environments'] as const;

export const environmentKeys = {
  all: root,
  list: (project: ProjectId | undefined) => [...root, project, 'list'] as const,
  detail: (project: ProjectId | undefined, id: string) => [...root, project, 'detail', id] as const,
  history: (project: ProjectId | undefined, id: string) => [...root, project, 'history', id] as const,
};

export function useEnvironmentsQuery() {
  const client = useEnvironmentsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: environmentKeys.list(project),
    queryFn: () => client.list(),
  });
}

export function useEnvironmentQuery(environmentId: string) {
  const client = useEnvironmentsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: environmentKeys.detail(project, environmentId),
    queryFn: async (): Promise<EnvironmentState | null> => (await client.get(environmentId)) ?? null,
  });
}

export function useEnvironmentHistoryQuery(environmentId: string) {
  const client = useEnvironmentsClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: environmentKeys.history(project, environmentId),
    queryFn: () => client.history(environmentId),
  });
}

export function useEnvironmentNames(): ReadonlyMap<string, string> {
  const { data } = useEnvironmentsQuery();
  return useMemo(
    () => new Map((data ?? []).map((environment) => [environment.environmentId, environment.displayName])),
    [data],
  );
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: environmentKeys.all }),
  });
}

export interface EnvironmentTarget {
  readonly environmentId: EnvironmentId;
  readonly expectedRevision: number;
}

export function useCreateEnvironment() {
  const client = useEnvironmentsClient();
  return useInvalidatingMutation((input: { environmentId: EnvironmentId; displayName: string; description: string }) =>
    client.dispatch(environmentCommands.create(input)),
  );
}

export function useUpdateEnvironment() {
  const client = useEnvironmentsClient();
  return useInvalidatingMutation(
    ({ environmentId, expectedRevision, ...input }: EnvironmentTarget & { displayName: string; description: string }) =>
      client.dispatch(environmentCommands.update(environmentId, input), { expectedRevision }),
  );
}

export function useDeleteEnvironment() {
  const client = useEnvironmentsClient();
  return useInvalidatingMutation(({ environmentId, expectedRevision }: EnvironmentTarget) =>
    client.dispatch(environmentCommands.delete(environmentId), { expectedRevision }),
  );
}
