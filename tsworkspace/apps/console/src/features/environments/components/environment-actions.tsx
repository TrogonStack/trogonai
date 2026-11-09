import { useNavigate } from '@tanstack/react-router';
import { PencilIcon, Trash2Icon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import type { EnvironmentView } from '../domain/view';
import { useDeleteEnvironment } from '../queries';
import { EditEnvironmentDialog } from './edit-environment-dialog';

export function EnvironmentActions({ environment }: { environment: EnvironmentView }) {
  const remove = useDeleteEnvironment();
  const notify = useNotify();
  const navigate = useNavigate();
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const { actions } = environment;

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant="outline" size="lg" onClick={() => setEditing(true)}>
          <PencilIcon data-icon="inline-start" />
          Edit
        </Button>
        <Button
          type="button"
          variant="destructive"
          size="lg"
          disabled={!actions.delete.available || remove.isPending}
          title={actions.delete.reason}
          onClick={() => setDeleting(true)}
        >
          <Trash2Icon data-icon="inline-start" />
          Delete
        </Button>
      </div>
      {actions.delete.reason ? (
        <p className="text-muted-foreground max-w-xs text-right text-xs">{actions.delete.reason}</p>
      ) : null}
      <FaultAlert error={remove.error} />
      <EditEnvironmentDialog environment={environment} open={editing} onOpenChange={setEditing} />
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        icon={<Trash2Icon />}
        title={`Delete ${environment.displayName}?`}
        description="No vault uses this environment. Once it is gone, new vaults can no longer be placed in it."
        confirmLabel="Delete environment"
        onConfirm={() => {
          setDeleting(false);
          remove.mutate(
            { environmentId: environment.environmentId, expectedRevision: environment.revision },
            {
              onSuccess: () => {
                notify.success('Environment deleted', environment.displayName);
                void navigate({ to: '/environments' });
              },
              onError: (error) => notify.failure('Delete failed', error.message),
            },
          );
        }}
      />
    </div>
  );
}
