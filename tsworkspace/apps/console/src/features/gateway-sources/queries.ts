import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { useCurrentProjectId } from '@/features/projects/current-project';
import type { ProjectId } from '@/features/projects/domain/project-id';

import { useSourcesClient } from './context';
import { sourceCommands } from './domain/commands';
import type { IntegrationId } from './domain/integration-id';
import type { SlotName } from './domain/source-kind';
import { toSourceView, type SourceView } from './domain/view';

const root = ['gateway-sources'] as const;
const SETTLING_POLL_MS = 400;

export const sourceKeys = {
  all: root,
  list: (project: ProjectId | undefined) => [...root, project, 'list'] as const,
  detail: (project: ProjectId | undefined, id: string) => [...root, project, 'detail', id] as const,
};

export function useSourcesQuery() {
  const client = useSourcesClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: sourceKeys.list(project),
    queryFn: async (): Promise<readonly SourceView[]> => (await client.list()).map(toSourceView),
    refetchInterval: (query) => (query.state.data?.some((source) => source.settling) ? SETTLING_POLL_MS : false),
  });
}

export function useSourceQuery(integrationId: string) {
  const client = useSourcesClient();
  const project = useCurrentProjectId();
  return useQuery({
    enabled: project !== undefined,
    queryKey: sourceKeys.detail(project, integrationId),
    queryFn: async (): Promise<SourceView | null> => {
      const state = await client.get(integrationId);
      return state ? toSourceView(state) : null;
    },
    refetchInterval: (query) => (query.state.data?.settling ? SETTLING_POLL_MS : false),
  });
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () => queryClient.invalidateQueries({ queryKey: sourceKeys.all }),
  });
}

export interface SourceTarget {
  readonly integrationId: IntegrationId;
  readonly expectedRevision: number;
}

export function usePauseSource() {
  const client = useSourcesClient();
  return useInvalidatingMutation(({ integrationId, expectedRevision }: SourceTarget) =>
    client.dispatch(sourceCommands.pause(integrationId), { expectedRevision }),
  );
}

export function useResumeSource() {
  const client = useSourcesClient();
  return useInvalidatingMutation(({ integrationId, expectedRevision }: SourceTarget) =>
    client.dispatch(sourceCommands.resume(integrationId), { expectedRevision }),
  );
}

export function useRotateSlot() {
  const client = useSourcesClient();
  return useInvalidatingMutation(({ integrationId, expectedRevision, slot }: SourceTarget & { slot: SlotName }) =>
    client.dispatch(sourceCommands.rotateSlot(integrationId, slot), { expectedRevision }),
  );
}

export function useReregisterWebhook() {
  const client = useSourcesClient();
  return useInvalidatingMutation(({ integrationId, expectedRevision }: SourceTarget) =>
    client.dispatch(sourceCommands.reregisterWebhook(integrationId), { expectedRevision }),
  );
}
