import { CredentialLifecycle } from '@trogonai/proto';
import {
  CircleOffIcon,
  MoreHorizontalIcon,
  PencilIcon,
  PowerIcon,
  RefreshCwIcon,
  ShieldAlertIcon,
  Trash2Icon,
} from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useNotify } from '@/components/ui/toaster';

import type { CredentialView } from '../domain/view';
import { useConfirmDestruction, useDisableCredential, useEnableCredential, useRenameCredential } from '../queries';
import { RenameDialog } from './rename-dialog';
import { RevokeDialog } from './revoke-dialog';
import { RotateDialog } from './rotate-dialog';
import { ScheduleDestructionDialog } from './schedule-destruction-dialog';

export function CredentialActions({ credential }: { credential: CredentialView }) {
  const disable = useDisableCredential();
  const enable = useEnableCredential();
  const rename = useRenameCredential();
  const confirm = useConfirmDestruction();
  const notify = useNotify();
  const [rotating, setRotating] = useState(false);
  const [revoking, setRevoking] = useState(false);
  const [scheduling, setScheduling] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [confirming, setConfirming] = useState(false);

  const { actions } = credential;
  const target = { secret: credential.secret, expectedRevision: credential.revision };
  const name = credential.displayName;
  const reconnect = credential.lifecycle === CredentialLifecycle.RECONNECT_REQUIRED;
  const terminal = credential.lifecycle === CredentialLifecycle.DESTROYED;
  const destroyPending = credential.lifecycle === CredentialLifecycle.DESTROY_PENDING;

  const hints = [
    !terminal && !actions.rotate.available ? actions.rotate.reason : undefined,
    destroyPending && !actions.confirmDestruction.available ? actions.confirmDestruction.reason : undefined,
  ].filter((hint): hint is string => hint !== undefined);

  const hasMenu =
    actions.disable.available ||
    actions.enable.available ||
    actions.revoke.available ||
    actions.scheduleDestruction.available ||
    destroyPending ||
    actions.rename.available;

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {actions.rotate.available ? (
          <Button type="button" size="lg" onClick={() => setRotating(true)}>
            <RefreshCwIcon data-icon="inline-start" />
            {reconnect ? 'Reconnect' : 'Rotate'}
          </Button>
        ) : null}
        {destroyPending ? (
          <Button
            type="button"
            variant="destructive"
            size="lg"
            disabled={!actions.confirmDestruction.available || confirm.isPending}
            onClick={() => setConfirming(true)}
          >
            <Trash2Icon data-icon="inline-start" />
            Confirm destruction
          </Button>
        ) : null}
        {hasMenu ? (
          <DropdownMenu>
            <DropdownMenuTrigger
              render={<Button type="button" variant="outline" size="icon-lg" aria-label="Credential actions" />}
            >
              <MoreHorizontalIcon />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-56">
              {actions.rename.available ? (
                <DropdownMenuItem onClick={() => setRenaming(true)}>
                  <PencilIcon />
                  Rename
                </DropdownMenuItem>
              ) : null}
              {actions.disable.available ? (
                <DropdownMenuItem
                  onClick={() =>
                    disable.mutate(target, {
                      onSuccess: () => notify.success('Credential disabled', name),
                      onError: (error) => notify.failure('Disable failed', error.message),
                    })
                  }
                >
                  <CircleOffIcon />
                  Disable
                </DropdownMenuItem>
              ) : null}
              {actions.enable.available ? (
                <DropdownMenuItem
                  onClick={() =>
                    enable.mutate(target, {
                      onSuccess: () => notify.success('Credential enabled', name),
                      onError: (error) => notify.failure('Enable failed', error.message),
                    })
                  }
                >
                  <PowerIcon />
                  Enable
                </DropdownMenuItem>
              ) : null}
              {actions.revoke.available || actions.scheduleDestruction.available ? <DropdownMenuSeparator /> : null}
              {actions.revoke.available ? (
                <DropdownMenuItem variant="destructive" onClick={() => setRevoking(true)}>
                  <ShieldAlertIcon />
                  Revoke
                </DropdownMenuItem>
              ) : null}
              {actions.scheduleDestruction.available ? (
                <DropdownMenuItem variant="destructive" onClick={() => setScheduling(true)}>
                  <Trash2Icon />
                  Schedule destruction
                </DropdownMenuItem>
              ) : null}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : null}
      </div>
      {hints.map((hint) => (
        <p key={hint} className="text-muted-foreground max-w-xs text-right text-xs">
          {hint}
        </p>
      ))}
      <FaultAlert error={disable.error ?? enable.error ?? confirm.error} />
      <RotateDialog credential={credential} open={rotating} onOpenChange={setRotating} />
      <RevokeDialog credential={credential} open={revoking} onOpenChange={setRevoking} />
      <ScheduleDestructionDialog credential={credential} open={scheduling} onOpenChange={setScheduling} />
      <RenameDialog
        title="Rename credential"
        description="The display name is the only part of a credential that can change. Its purpose, kind and environment are immutable."
        label="Display name"
        current={name}
        open={renaming}
        onOpenChange={setRenaming}
        pending={rename.isPending}
        error={rename.error}
        onReset={() => rename.reset()}
        onSubmit={async (next) => {
          await rename.mutateAsync({ ...target, displayName: next });
          notify.success('Credential renamed', next.trim());
        }}
      />
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        icon={<Trash2Icon />}
        title={`Destroy ${name}?`}
        description="Every version is destroyed and the secret reference stops resolving. This cannot be undone."
        confirmLabel="Confirm destruction"
        onConfirm={() => {
          confirm.mutate(target, {
            onSuccess: () => notify.success('Credential destroyed', name),
            onError: (error) => notify.failure('Destruction failed', error.message),
          });
          setConfirming(false);
        }}
      />
    </div>
  );
}
