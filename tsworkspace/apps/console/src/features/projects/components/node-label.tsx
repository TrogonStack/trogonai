import { useCallback } from 'react';

import type { NodeId } from '@/features/hierarchy/domain/node-id';

import { useParentCandidatesQuery } from '../queries';

export function useNodeLabel(): (nodeId: NodeId) => string {
  const { data } = useParentCandidatesQuery();
  return useCallback((nodeId) => data?.find((node) => node.nodeId === nodeId)?.displayName ?? nodeId, [data]);
}

export function NodeLabel({ nodeId }: { nodeId: NodeId }) {
  const nodeLabel = useNodeLabel();
  return <span className="text-[13px]">{nodeLabel(nodeId)}</span>;
}
