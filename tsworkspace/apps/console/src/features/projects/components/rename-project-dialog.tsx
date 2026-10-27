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
import { useOnOpen } from '@/components/use-on-open';

import { MAX_DISPLAY_NAME_LENGTH, displayNameProblem } from '../domain/decider';
import type { ProjectView } from '../domain/view';
import { useRenameProject } from '../queries';

interface RenameProjectDialogProps {
  project: ProjectView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RenameProjectDialog({ project, open, onOpenChange }: RenameProjectDialogProps) {
  const rename = useRenameProject();
  const notify = useNotify();
  const [name, setName] = useState(project.displayName);
  const [attempted, setAttempted] = useState(false);

  useOnOpen(open, () => setName(project.displayName));

  const nameError = attempted ? displayNameProblem(name) : undefined;

  const close = (next: boolean) => {
    if (!next) {
      rename.reset();
      setAttempted(false);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (displayNameProblem(name) !== undefined) return;
    try {
      await rename.mutateAsync({
        projectId: project.projectId,
        expectedRevision: project.revision,
        displayName: name,
      });
      notify.success('Project renamed', name.trim());
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rename project</DialogTitle>
          <DialogDescription>The name is only a label. The id stays the same.</DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Rename project"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={rename.error} title="The rename was rejected" />
          <TextField
            id="rename-project-name"
            label="Project name"
            value={name}
            onChange={setName}
            error={nameError}
            hint={`Up to ${MAX_DISPLAY_NAME_LENGTH} characters.`}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={rename.isPending}>
              {rename.isPending ? 'Saving...' : 'Save name'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
