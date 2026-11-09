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

import type { CredentialView } from '../domain/view';
import { useScheduleDestruction } from '../queries';

const HOUR = 3_600_000;

const delays = [
  { value: 'now', label: 'Immediately', milliseconds: 0 },
  { value: '1h', label: 'In 1 hour', milliseconds: HOUR },
  { value: '24h', label: 'In 24 hours', milliseconds: 24 * HOUR },
  { value: '7d', label: 'In 7 days', milliseconds: 7 * 24 * HOUR },
] as const;

interface ScheduleDestructionDialogProps {
  credential: CredentialView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ScheduleDestructionDialog({ credential, open, onOpenChange }: ScheduleDestructionDialogProps) {
  const schedule = useScheduleDestruction();
  const notify = useNotify();
  const [delay, setDelay] = useState<string>('7d');

  const close = (next: boolean) => {
    if (!next) {
      schedule.reset();
      setDelay('7d');
    }
    onOpenChange(next);
  };

  const submit = () => {
    const chosen = delays.find((entry) => entry.value === delay) ?? delays[0];
    schedule.mutate(
      {
        secret: credential.secret,
        expectedRevision: credential.revision,
        executeAfter: new Date(Date.now() + chosen.milliseconds),
      },
      {
        onSuccess: () => {
          notify.success('Destruction scheduled', credential.displayName);
          close(false);
        },
      },
    );
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Schedule destruction</DialogTitle>
          <DialogDescription>
            The credential is already denied for every use. The delay only postpones the physical removal of its
            versions, and you confirm it once it is due.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Schedule destruction"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <FaultAlert error={schedule.error} title="The destruction was rejected" />
          <SelectField
            id="destroy-after"
            label="Execute after"
            value={delay}
            options={delays.map(({ value, label }) => ({ value, label }))}
            onChange={setDelay}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" variant="destructive" disabled={schedule.isPending}>
              {schedule.isPending ? 'Scheduling...' : 'Schedule destruction'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
