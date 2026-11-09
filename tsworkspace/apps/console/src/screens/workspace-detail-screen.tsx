import { FolderKanbanIcon, FoldersIcon } from 'lucide-react';
import { useCallback } from 'react';

import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Stat, StatStrip } from '@/components/stat-strip';
import { Skeleton } from '@/components/ui/skeleton';
import { AddFolderDialog } from '@/features/hierarchy/components/add-folder-dialog';
import { Ancestry } from '@/features/hierarchy/components/ancestry';
import { HierarchyTimeline } from '@/features/hierarchy/components/hierarchy-timeline';
import { NodeActions } from '@/features/hierarchy/components/node-actions';
import { AttachedProjects, ChildFolders } from '@/features/hierarchy/components/node-lists';
import { NodeWordBadge } from '@/features/hierarchy/components/word-badge';
import { nodeIdFromStored, type NodeId } from '@/features/hierarchy/domain/node-id';
import { MAX_TREE_DEPTH } from '@/features/hierarchy/domain/tree';
import { organizationLabel } from '@/features/hierarchy/domain/view';
import { useHierarchyHistoryQuery, useHierarchyView, useLabelsQuery } from '@/features/hierarchy/queries';
import { useProjectsQuery } from '@/features/projects/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading node</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <Skeleton className="h-32 w-full" />
    </div>
  );
}

export function WorkspaceDetailScreen({ nodeId }: { nodeId: string }) {
  const { view, error } = useHierarchyView();
  const history = useHierarchyHistoryQuery();
  const labels = useLabelsQuery();
  const projects = useProjectsQuery();

  const names = useCallback(
    (id: NodeId) =>
      view && id === view.rootId
        ? organizationLabel(view)
        : (labels.data?.find((entry) => entry.nodeId === id)?.name ?? id),
    [labels.data, view],
  );

  const back = <BackLink to="/workspaces">All workspaces</BackLink>;

  if (error) return <FaultAlert error={error} title="Could not load the node" />;
  if (!view) return <DetailSkeleton />;

  const node = view.byId.get(nodeIdFromStored(nodeId));
  if (!node || node.nodeId === view.rootId) {
    return (
      <NotFoundState back={back} title="Node not found">
        No workspace or folder has the id <span className="font-mono">{nodeId}</span>.
      </NotFoundState>
    );
  }

  const attached = (projects.data ?? []).filter((project) => project.parent === node.nodeId);

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <Ancestry node={node} view={view} />
      <DetailHeader
        title={node.name}
        status={<NodeWordBadge word={node.word} />}
        identifier={<span className="font-mono">{node.nodeId}</span>}
        copyValue={node.nodeId}
        copyLabel="node id"
        facts={[`Depth ${node.depth} of ${MAX_TREE_DEPTH}`, `Revision ${view.revision}`]}
        actions={<NodeActions node={node} view={view} />}
      />
      <StatStrip label="Node summary">
        <Stat
          label="Folders"
          value={node.children.length}
          caption={`${node.descendants} in the whole subtree`}
          icon={FoldersIcon}
          led="[--led:var(--primary)]"
        />
        <Stat
          label="Projects"
          value={node.attached}
          caption={`${node.attachedBelow} in the whole subtree`}
          icon={FolderKanbanIcon}
          led="[--led:var(--success)]"
        />
      </StatStrip>
      <ChildFolders node={node} view={view} action={<AddFolderDialog parent={node} view={view} />} />
      <AttachedProjects projects={attached} />
      <section aria-label="Activity" className="flex flex-col gap-3">
        <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
          <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
          Activity
        </h2>
        <HierarchyTimeline events={history.data ?? []} nodeId={node.nodeId} nameOf={names} />
      </section>
    </div>
  );
}
