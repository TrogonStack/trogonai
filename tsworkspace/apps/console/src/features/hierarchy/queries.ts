import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useMemo } from 'react';

import { projectKeys, useProjectsQuery } from '@/features/projects/queries';

import { useHierarchyClient } from './context';
import { hierarchyCommands } from './domain/commands';
import type { NodeId } from './domain/node-id';
import type { NodeName } from './domain/node-name';
import { buildHierarchyView, type HierarchyView } from './domain/view';

const root = ['hierarchy'] as const;

export const hierarchyKeys = {
  all: root,
  tree: () => [...root, 'tree'] as const,
  labels: () => [...root, 'labels'] as const,
  history: () => [...root, 'history'] as const,
};

export function useTreeQuery() {
  const client = useHierarchyClient();
  return useQuery({ queryKey: hierarchyKeys.tree(), queryFn: () => client.tree() });
}

export function useLabelsQuery() {
  const client = useHierarchyClient();
  return useQuery({ queryKey: hierarchyKeys.labels(), queryFn: () => client.labels() });
}

export function useHierarchyHistoryQuery() {
  const client = useHierarchyClient();
  return useQuery({ queryKey: hierarchyKeys.history(), queryFn: () => client.history() });
}

export interface HierarchyViewState {
  readonly view: HierarchyView | undefined;
  readonly isPending: boolean;
  readonly error: Error | null;
}

export function useHierarchyView(): HierarchyViewState {
  const tree = useTreeQuery();
  const labels = useLabelsQuery();
  const projects = useProjectsQuery();

  const view = useMemo(() => {
    if (!tree.data || !labels.data || !projects.data) return undefined;
    const counts = new Map<NodeId, number>();
    for (const project of projects.data) counts.set(project.parent, (counts.get(project.parent) ?? 0) + 1);
    return buildHierarchyView(tree.data, labels.data, counts);
  }, [tree.data, labels.data, projects.data]);

  return {
    view,
    isPending: tree.isPending || labels.isPending || projects.isPending,
    error: tree.error ?? labels.error ?? projects.error,
  };
}

function useInvalidatingMutation<TVariables, TResult>(mutationFn: (variables: TVariables) => Promise<TResult>) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: hierarchyKeys.all }),
        queryClient.invalidateQueries({ queryKey: projectKeys.parents() }),
      ]),
  });
}

export interface AddNodeInput {
  readonly nodeId: NodeId;
  readonly parent: NodeId;
  readonly name: NodeName;
  readonly organizationName?: NodeName;
  readonly expectedRevision: number;
  readonly rootId: NodeId;
}

export function useAddNode() {
  const client = useHierarchyClient();
  return useInvalidatingMutation(async (input: AddNodeInput) => {
    await client.dispatch(hierarchyCommands.add({ nodeId: input.nodeId, parent: input.parent }), {
      expectedRevision: input.expectedRevision,
    });
    await client.setLabel(input.nodeId, input.name);
    if (input.organizationName) await client.setLabel(input.rootId, input.organizationName);
  });
}

export interface MoveNodeInput {
  readonly nodeId: NodeId;
  readonly parent: NodeId;
  readonly organizationName?: NodeName;
  readonly expectedRevision: number;
  readonly rootId: NodeId;
}

export function useMoveNode() {
  const client = useHierarchyClient();
  return useInvalidatingMutation(async (input: MoveNodeInput) => {
    await client.dispatch(hierarchyCommands.move({ nodeId: input.nodeId, parent: input.parent }), {
      expectedRevision: input.expectedRevision,
    });
    if (input.organizationName) await client.setLabel(input.rootId, input.organizationName);
  });
}

export function useRemoveNode() {
  const client = useHierarchyClient();
  return useInvalidatingMutation(({ nodeId, expectedRevision }: { nodeId: NodeId; expectedRevision: number }) =>
    client.dispatch(hierarchyCommands.remove(nodeId), { expectedRevision }),
  );
}

export function useRenameNode() {
  const client = useHierarchyClient();
  return useInvalidatingMutation(({ nodeId, name }: { nodeId: NodeId; name: NodeName }) =>
    client.setLabel(nodeId, name),
  );
}
