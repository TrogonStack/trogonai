import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { SelectField } from '@/components/field';
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

import { formatVersion } from '../domain/secret-version';
import type { SlotName } from '../domain/source-kind';
import type { SourceView } from '../domain/view';
import { useRotateSlot } from '../queries';

interface RotateSlotDialogProps {
  source: SourceView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RotateSlotDialog({ source, open, onOpenChange }: RotateSlotDialogProps) {
  const rotate = useRotateSlot();
  const notify = useNotify();
  const rotatable = source.slots.filter((slot) => slot.rotatable);
  const [chosen, setChosen] = useState<SlotName | undefined>();
  const slot = rotatable.find((candidate) => candidate.name === chosen) ?? rotatable[0];

  const close = (next: boolean) => {
    if (!next) {
      rotate.reset();
      setChosen(undefined);
    }
    onOpenChange(next);
  };

  const submit = () => {
    if (!slot) return;
    rotate.mutate(
      { integrationId: source.id, expectedRevision: source.revision, slot: slot.name },
      {
        onSuccess: () => {
          notify.success('Rotation started', `${source.displayName}: ${slot.label}`);
          close(false);
        },
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Rotate a credential</DialogTitle>
          <DialogDescription>
            A new version is written and activated. If the webhook is registered with this secret, it is registered
            again with the new version.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Rotate credential"
          className="flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <FaultAlert error={rotate.error} title="The rotation was rejected" />
          {slot ? (
            <SelectField
              id="rotate-slot"
              label="Credential slot"
              value={slot.name}
              options={rotatable.map((candidate) => ({
                value: candidate.name,
                label: `${candidate.label}${candidate.version !== undefined ? ` (${formatVersion(candidate.version)})` : ''}`,
              }))}
              onChange={setChosen}
              hint="Only runtime slots can be rotated. Static slots come from the gateway environment."
            />
          ) : null}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={rotate.isPending || !slot}>
              {rotate.isPending ? 'Working...' : 'Rotate'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
