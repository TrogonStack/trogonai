import { PlusIcon } from 'lucide-react';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { TextField } from '@/components/field';
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

import { MAX_DESCRIPTION_LENGTH, MAX_DISPLAY_NAME_LENGTH } from '../domain/environment';
import {
  MAX_ENVIRONMENT_ID_LENGTH,
  environmentIdProblem,
  parseEnvironmentId,
  suggestEnvironmentId,
} from '../domain/environment-id';
import { useCreateEnvironment } from '../queries';
import { DescriptionField } from './description-field';

export function CreateEnvironmentDialog() {
  const create = useCreateEnvironment();
  const notify = useNotify();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [typedId, setTypedId] = useState<string | undefined>();
  const [description, setDescription] = useState('');
  const [attempted, setAttempted] = useState(false);

  const id = typedId ?? suggestEnvironmentId(name);
  const nameError =
    attempted && name.trim() === ''
      ? 'An environment name is required.'
      : name.trim().length > MAX_DISPLAY_NAME_LENGTH
        ? `A name is at most ${MAX_DISPLAY_NAME_LENGTH} characters.`
        : undefined;
  const idError = attempted ? environmentIdProblem(id) : undefined;
  const descriptionError =
    description.trim().length > MAX_DESCRIPTION_LENGTH
      ? `A description is at most ${MAX_DESCRIPTION_LENGTH} characters.`
      : undefined;

  const close = (next: boolean) => {
    if (!next) {
      create.reset();
      setAttempted(false);
      setName('');
      setTypedId(undefined);
      setDescription('');
    }
    setOpen(next);
  };

  const submit = async () => {
    setAttempted(true);
    const environmentId = parseEnvironmentId(id);
    if (name.trim() === '' || name.trim().length > MAX_DISPLAY_NAME_LENGTH || !environmentId || descriptionError)
      return;
    try {
      await create.mutateAsync({ environmentId, displayName: name, description });
      notify.success('Environment created', name.trim());
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogTrigger render={<Button type="button" size="lg" />}>
        <PlusIcon data-icon="inline-start" />
        New environment
      </DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>New environment</DialogTitle>
          <DialogDescription>
            Vaults pick an environment when they are created. The id is permanent, so choose it with care.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Create environment"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={create.error} title="The environment was rejected" />
          <TextField
            id="environment-name"
            label="Environment name"
            value={name}
            onChange={setName}
            error={nameError}
            placeholder="Load testing"
          />
          <TextField
            id="environment-id"
            label="Environment id"
            value={id}
            onChange={setTypedId}
            error={idError}
            placeholder="load-testing"
            hint={`Lowercase letters, digits and hyphens, up to ${MAX_ENVIRONMENT_ID_LENGTH} characters. It cannot change later.`}
            mono
          />
          <DescriptionField
            id="environment-description"
            value={description}
            onChange={setDescription}
            error={descriptionError}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending}>
              {create.isPending ? 'Creating...' : 'Create environment'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
