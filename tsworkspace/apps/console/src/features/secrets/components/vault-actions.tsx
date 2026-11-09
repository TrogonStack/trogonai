import { VaultLifecycle } from '@trogonai/proto';
import { PencilIcon, PowerIcon, PowerOffIcon, Trash2Icon } from 'lucide-react';
import { useState } from 'react';

import { ConfirmDialog } from '@/components/confirm-dialog';
import { FaultAlert } from '@/components/fault-alert';
import { Button } from '@/components/ui/button';
import { useNotify } from '@/components/ui/toaster';

import type { VaultTransition } from '../domain/commands';
import type { VaultView } from '../domain/view';
import { useRenameVault, useVaultTransition } from '../queries';
import { RenameDialog } from './rename-dialog';

export function VaultActions({ vault }: { vault: VaultView }) {
  const transition = useVaultTransition();
  const rename = useRenameVault();
  const notify = useNotify();
  const [renaming, setRenaming] = useState(false);
  const [destroying, setDestroying] = useState(false);

  const target = { vaultId: vault.vaultId, expectedRevision: vault.revision };
  const { actions } = vault;
  const destroyedVault = vault.lifecycle === VaultLifecycle.DESTROYED;

  const run = (next: VaultTransition, title: string) =>
    transition.mutate(
      { ...target, transition: next },
      {
        onSuccess: () => notify.success(title, vault.displayName),
        onError: (error) => notify.failure(`${title} failed`, error.message),
      },
    );

  const blockedReasons = [
    vault.lifecycle === VaultLifecycle.DESTROY_PENDING ? undefined : actions.scheduleDestruction.reason,
    vault.lifecycle === VaultLifecycle.DESTROY_PENDING ? actions.confirmDestroyed.reason : undefined,
  ].filter((reason): reason is string => reason !== undefined && !destroyedVault);

  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {actions.rename.available ? (
          <Button type="button" variant="outline" size="lg" onClick={() => setRenaming(true)}>
            <PencilIcon data-icon="inline-start" />
            Rename
          </Button>
        ) : null}
        {actions.disable.available ? (
          <Button
            type="button"
            variant="outline"
            size="lg"
            disabled={transition.isPending}
            onClick={() => run('DISABLE', 'Vault disabled')}
          >
            <PowerOffIcon data-icon="inline-start" />
            Disable
          </Button>
        ) : null}
        {actions.enable.available ? (
          <Button
            type="button"
            size="lg"
            disabled={transition.isPending}
            onClick={() => run('ENABLE', 'Vault enabled')}
          >
            <PowerIcon data-icon="inline-start" />
            Enable
          </Button>
        ) : null}
        {vault.lifecycle === VaultLifecycle.ACTIVE || vault.lifecycle === VaultLifecycle.DISABLED ? (
          <Button
            type="button"
            variant="destructive"
            size="lg"
            disabled={!actions.scheduleDestruction.available || transition.isPending}
            title={actions.scheduleDestruction.reason}
            onClick={() => run('SCHEDULE_DESTRUCTION', 'Destruction scheduled')}
          >
            <Trash2Icon data-icon="inline-start" />
            Schedule destruction
          </Button>
        ) : null}
        {vault.lifecycle === VaultLifecycle.DESTROY_PENDING ? (
          <Button
            type="button"
            variant="destructive"
            size="lg"
            disabled={!actions.confirmDestroyed.available || transition.isPending}
            title={actions.confirmDestroyed.reason}
            onClick={() => setDestroying(true)}
          >
            <Trash2Icon data-icon="inline-start" />
            Confirm destroyed
          </Button>
        ) : null}
      </div>
      {blockedReasons.map((reason) => (
        <p key={reason} className="text-muted-foreground max-w-xs text-right text-xs">
          {reason}
        </p>
      ))}
      <FaultAlert error={transition.error} />
      <RenameDialog
        title="Rename vault"
        description="The display name is the only part of a vault that can change. Its environment is immutable."
        label="Vault name"
        current={vault.displayName}
        open={renaming}
        onOpenChange={setRenaming}
        pending={rename.isPending}
        error={rename.error}
        onReset={() => rename.reset()}
        onSubmit={async (next) => {
          await rename.mutateAsync({ ...target, displayName: next });
          notify.success('Vault renamed', next.trim());
        }}
      />
      <ConfirmDialog
        open={destroying}
        onOpenChange={setDestroying}
        icon={<Trash2Icon />}
        title={`Confirm ${vault.displayName} destroyed?`}
        description="The vault is finished for good. It can no longer be renamed, enabled or filled."
        confirmLabel="Confirm destroyed"
        onConfirm={() => {
          run('CONFIRM_DESTROYED', 'Vault destroyed');
          setDestroying(false);
        }}
      />
    </div>
  );
}
