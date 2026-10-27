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
} from '@/components/ui/dialog';
import { useNotify } from '@/components/ui/toaster';

import { MAX_DESCRIPTION_LENGTH, MAX_DISPLAY_NAME_LENGTH } from '../domain/environment';
import type { EnvironmentView } from '../domain/view';
import { useUpdateEnvironment } from '../queries';
import { DescriptionField } from './description-field';

interface EditEnvironmentDialogProps {
  environment: EnvironmentView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function EditEnvironmentDialog({ environment, open, onOpenChange }: EditEnvironmentDialogProps) {
  const update = useUpdateEnvironment();
  const notify = useNotify();
  const [name, setName] = useState(environment.displayName);
  const [description, setDescription] = useState(environment.description);
  const [attempted, setAttempted] = useState(false);

  const nameError =
    attempted && name.trim() === ''
      ? 'An environment name is required.'
      : name.trim().length > MAX_DISPLAY_NAME_LENGTH
        ? `A name is at most ${MAX_DISPLAY_NAME_LENGTH} characters.`
        : undefined;
  const descriptionError =
    description.trim().length > MAX_DESCRIPTION_LENGTH
      ? `A description is at most ${MAX_DESCRIPTION_LENGTH} characters.`
      : undefined;

  const close = (next: boolean) => {
    if (!next) {
      update.reset();
      setAttempted(false);
    } else {
      setName(environment.displayName);
      setDescription(environment.description);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (name.trim() === '' || name.trim().length > MAX_DISPLAY_NAME_LENGTH || descriptionError) return;
    try {
      await update.mutateAsync({
        environmentId: environment.environmentId,
        expectedRevision: environment.revision,
        displayName: name,
        description,
      });
      notify.success('Environment updated', name.trim());
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Edit environment</DialogTitle>
          <DialogDescription>
            The name and description can change. The id is permanent because vaults and credentials refer to it.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Edit environment"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={update.error} title="The update was rejected" />
          <TextField
            id="edit-environment-name"
            label="Environment name"
            value={name}
            onChange={setName}
            error={nameError}
          />
          <TextField
            id="edit-environment-id"
            label="Environment id"
            value={environment.environmentId}
            onChange={() => undefined}
            disabled
            mono
          />
          <DescriptionField
            id="edit-environment-description"
            value={description}
            onChange={setDescription}
            error={descriptionError}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={update.isPending}>
              {update.isPending ? 'Saving...' : 'Save changes'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
