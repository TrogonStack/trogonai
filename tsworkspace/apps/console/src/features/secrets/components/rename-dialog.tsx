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
import { useOnOpen } from '@/components/use-on-open';

interface RenameDialogProps {
  title: string;
  description: string;
  label: string;
  current: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  pending: boolean;
  error: unknown;
  onSubmit: (name: string) => Promise<void>;
  onReset: () => void;
}

export function RenameDialog({
  title,
  description,
  label,
  current,
  open,
  onOpenChange,
  pending,
  error,
  onSubmit,
  onReset,
}: RenameDialogProps) {
  const [name, setName] = useState(current);
  const [attempted, setAttempted] = useState(false);

  useOnOpen(open, () => setName(current));

  const invalid = attempted && name.trim() === '' ? `${label} is required.` : undefined;

  const close = (next: boolean) => {
    if (!next) {
      onReset();
      setAttempted(false);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (name.trim() === '') return;
    try {
      await onSubmit(name);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label={title}
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={error} title="The rename was rejected" />
          <TextField id="rename-name" label={label} value={name} onChange={setName} error={invalid} />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending}>
              {pending ? 'Saving...' : 'Save name'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
