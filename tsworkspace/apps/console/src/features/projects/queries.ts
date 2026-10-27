import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import type { ProjectAccessPreset } from '@trogonai/proto';

import { useProjectsClient } from './context';
import { projectCommands } from './domain/commands';
import type { NodeId } from './domain/node-id';
import type { ProjectId } from './domain/project-id';
import { toProjectView, type ProjectView } from './domain/view';

const root = ['projects'] as const;

export const projectKeys = {
  all: root,
  list: () => [...root, 'list'] as const,
  detail: (id: string) => [...root, 'detail', id] as const,
  history: (id: string) => [...root, 'history', id] as const,
  resources: (id: string) => [...root, 'resources', id] as const,
  parents: () => [...root, 'parents'] as const,
};

export function useProjectsQuery() {
  const client = useProjectsClient();
  return useQuery({
    queryKey: projectKeys.list(),
    queryFn: async (): Promise<readonly ProjectView[]> => (await client.list()).map(toProjectView),
  });
}

export function useProjectQuery(projectId: string) {
  const client = useProjectsClient();
  return useQuery({
    queryKey: projectKeys.detail(projectId),
    queryFn: async (): Promise<ProjectView | null> => {
      const record = await client.get(projectId);
      return record ? toProjectView(record) : null;
    },
  });
}

export function useProjectHistoryQuery(projectId: string) {
  const client = useProjectsClient();
  return useQuery({ queryKey: projectKeys.history(projectId), queryFn: () => client.history(projectId) });
}

export function useProjectResourcesQuery(projectId: ProjectId) {
  const client = useProjectsClient();
  return useQuery({ queryKey: projectKeys.resources(projectId), queryFn: () => client.resources(projectId) });
}

export function useParentCandidatesQuery() {
  const client = useProjectsClient();
  return useQuery({ queryKey: projectKeys.parents(), queryFn: () => client.parentCandidates() });
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: projectKeys.all }),
  });
}

export interface ProjectTarget {
  readonly projectId: ProjectId;
  readonly expectedRevision: number;
}

export interface CreateProjectInput {
  readonly projectId: ProjectId;
  readonly displayName: string;
  readonly goal: string;
  readonly parent: NodeId;
  readonly initialAccessPreset: ProjectAccessPreset;
}

export function useCreateProject() {
  const client = useProjectsClient();
  return useInvalidatingMutation((input: CreateProjectInput) => client.dispatch(projectCommands.create(input)));
}

export function useRenameProject() {
  const client = useProjectsClient();
  return useInvalidatingMutation(
    ({ projectId, expectedRevision, displayName }: ProjectTarget & { displayName: string }) =>
      client.dispatch(projectCommands.rename(projectId, displayName), { expectedRevision }),
  );
}

export function useRestateProjectGoal() {
  const client = useProjectsClient();
  return useInvalidatingMutation(({ projectId, expectedRevision, goal }: ProjectTarget & { goal: string }) =>
    client.dispatch(projectCommands.restateGoal(projectId, goal), { expectedRevision }),
  );
}

export function useArchiveProject() {
  const client = useProjectsClient();
  return useInvalidatingMutation(({ projectId, expectedRevision }: ProjectTarget) =>
    client.dispatch(projectCommands.archive(projectId), { expectedRevision }),
  );
}

export function useUnarchiveProject() {
  const client = useProjectsClient();
  return useInvalidatingMutation(({ projectId, expectedRevision }: ProjectTarget) =>
    client.dispatch(projectCommands.unarchive(projectId), { expectedRevision }),
  );
}
