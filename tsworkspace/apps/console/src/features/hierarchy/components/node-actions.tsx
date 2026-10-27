import { useNavigate } from '@tanstack/react-router';
import { MoveIcon, PencilIcon, Trash2Icon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import { nodeWordLabels } from '../domain/tree';
import type { HierarchyView, NodeView } from '../domain/view';
import { useRemoveNode } from '../queries';
import { MoveNodeDialog } from './move-node-dialog';
import { RenameNodeDialog } from './rename-node-dialog';

export function NodeActions({ node, view }: { node: NodeView; view: HierarchyView }) {
  const remove = useRemoveNode();
  const notify = useNotify();
  const navigate = useNavigate();
  const [renaming, setRenaming] = useState(false);
  const [moving, setMoving] = useState(false);
  const [removing, setRemoving] = useState(false);

  const { actions } = node;
  const word = nodeWordLabels[node.word].toLowerCase();

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="outline" size="lg" onClick={() => setRenaming(true)}>
          <PencilIcon data-icon="inline-start" />
          Rename
        </Button>
        <Button
          type="button"
          variant="outline"
          size="lg"
          disabled={!actions.move.available}
          title={actions.move.reason}
          onClick={() => setMoving(true)}
        >
          <MoveIcon data-icon="inline-start" />
          Move
        </Button>
        <Button
          type="button"
          variant="destructive"
          size="lg"
          disabled={!actions.remove.available || remove.isPending}
          title={actions.remove.reason}
          onClick={() => setRemoving(true)}
        >
          <Trash2Icon data-icon="inline-start" />
          Remove
        </Button>
      </div>
      {actions.remove.reason ? (
        <p className="text-muted-foreground max-w-xs text-right text-xs">{actions.remove.reason}</p>
      ) : null}
      <FaultAlert error={remove.error} title="The removal was rejected" />
      <RenameNodeDialog node={node} open={renaming} onOpenChange={setRenaming} />
      <MoveNodeDialog node={node} view={view} open={moving} onOpenChange={setMoving} />
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        icon={<Trash2Icon />}
        title={`Remove ${node.name}?`}
        description={`This ${word} is empty. Removing it only changes the tree, and its history stays in the activity log.`}
        confirmLabel={`Remove ${word}`}
        onConfirm={() => {
          setRemoving(false);
          remove.mutate(
            { nodeId: node.nodeId, expectedRevision: view.revision },
            {
              onSuccess: () => {
                notify.success('Removed', node.name);
                if (node.parent && node.parent !== view.rootId) {
                  void navigate({ to: '/workspaces/$nodeId', params: { nodeId: node.parent } });
                } else {
                  void navigate({ to: '/workspaces' });
                }
              },
              onError: (error) => notify.failure('Removal failed', error.message),
            },
          );
        }}
      />
    </div>
  );
}
