import { PlusIcon } from 'lucide-react';
import { useMemo, useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { SelectField, TextField } from '@/components/field';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { useNotify } from '@/components/ui/toaster';
import { builtInEnvironmentIds, type EnvironmentId } from '@/features/environments/domain/environment-id';
import { useEnvironmentsQuery } from '@/features/environments/queries';

import { useCreateVault } from '../queries';

export function CreateVaultDialog() {
  const create = useCreateVault();
  const notify = useNotify();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const registry = useEnvironmentsQuery();
  const [chosen, setChosen] = useState<EnvironmentId | undefined>();
  const [attempted, setAttempted] = useState(false);

  const options = useMemo(
    () =>
      (registry.data ?? []).map((environment) => ({
        value: environment.environmentId,
        label: environment.displayName,
      })),
    [registry.data],
  );
  const environment =
    chosen ??
    (options.some((option) => option.value === builtInEnvironmentIds.production)
      ? builtInEnvironmentIds.production
      : options[0]?.value);

  const error = attempted && name.trim() === '' ? 'A vault name is required.' : undefined;

  const close = (next: boolean) => {
    if (!next) {
      create.reset();
      setAttempted(false);
      setName('');
      setChosen(undefined);
    }
    setOpen(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (name.trim() === '' || environment === undefined) return;
    try {
      await create.mutateAsync({ displayName: name, environment });
      notify.success('Vault created', name.trim());
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger render={<Button type="button" variant="outline" size="lg" />}>
        <PlusIcon data-icon="inline-start" />
        New vault
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New vault</DialogTitle>
          <DialogDescription>
            A vault groups credentials of one environment. The environment cannot change afterwards.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Create vault"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={create.error} title="The vault was rejected" />
          <TextField
            id="vault-name"
            label="Vault name"
            value={name}
            onChange={setName}
            error={error}
            placeholder="Payments"
          />
          <SelectField
            id="vault-environment"
            label="Environment"
            value={environment ?? ''}
            options={options}
            onChange={setChosen}
            disabled={options.length === 0}
            hint="Every credential in the vault inherits this environment."
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || environment === undefined}>
              {create.isPending ? 'Creating...' : 'Create vault'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
