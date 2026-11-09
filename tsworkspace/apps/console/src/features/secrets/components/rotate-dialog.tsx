import { CredentialLifecycle } from '@trogonai/proto';
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

import { MAX_OVERLAP_HOURS, isWebhookPurpose } from '../domain/policy';
import type { CredentialView } from '../domain/view';
import {
  effectiveOrigin,
  initialMaterialDraft,
  materialError,
  toMaterialInput,
  toSimulation,
  type MaterialDraft,
} from '../forms';
import { useRotateCredential } from '../queries';
import { MaterialFields } from './material-fields';
import { SecretReveal } from './secret-reveal';

interface RevealedState {
  readonly value: string;
  readonly version: number;
}

function overlapError(raw: string): string | undefined {
  const hours = Number(raw);
  if (raw.trim() === '' || !Number.isFinite(hours) || hours < 0) return 'Enter a number of hours, 0 or more.';
  if (hours > MAX_OVERLAP_HOURS) return `An overlap lasts at most ${MAX_OVERLAP_HOURS} hours.`;
  return undefined;
}

interface RotateDialogProps {
  credential: CredentialView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function RotateDialog({ credential, open, onOpenChange }: RotateDialogProps) {
  const rotate = useRotateCredential();
  const notify = useNotify();
  const [material, setMaterial] = useState<MaterialDraft>(initialMaterialDraft);
  const [overlap, setOverlap] = useState('1');
  const [attempted, setAttempted] = useState(false);
  const [revealed, setRevealed] = useState<RevealedState | undefined>();

  const reconnect = credential.lifecycle === CredentialLifecycle.RECONNECT_REQUIRED;
  const webhook = isWebhookPurpose(credential.purpose) && !reconnect;
  const verb = reconnect ? 'Reconnect' : 'Rotate';
  const secretError = attempted ? materialError(material, credential.purpose, credential.kind) : undefined;
  const hoursError = attempted && webhook ? overlapError(overlap) : undefined;

  const close = (next: boolean) => {
    if (!next) {
      rotate.reset();
      setAttempted(false);
      setMaterial(initialMaterialDraft);
      setOverlap('1');
      setRevealed(undefined);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (materialError(material, credential.purpose, credential.kind)) return;
    if (webhook && overlapError(overlap)) return;
    const hours = webhook ? Number(overlap) : undefined;
    try {
      const result = await rotate.mutateAsync({
        secret: credential.secret,
        expectedRevision: credential.revision,
        material: toMaterialInput(material, credential.purpose),
        overlapHours: hours === 0 ? undefined : hours,
        simulation: toSimulation(material, credential.purpose),
      });
      notify.success(`${verb} started`, credential.displayName);
      if (result.revealed) {
        setRevealed({ value: result.revealed.value, version: result.revealed.version });
        return;
      }
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>
            {verb} {credential.displayName}
          </DialogTitle>
          <DialogDescription>
            A new version is written and activated. The secret reference stays the same, so nothing that points at it
            changes.
          </DialogDescription>
        </DialogHeader>
        {revealed ? (
          <SecretReveal value={revealed.value} version={revealed.version} onAcknowledge={() => close(false)} />
        ) : (
          <form
            noValidate
            aria-label={`${verb} credential`}
            className="flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void submit();
            }}
          >
            <FaultAlert error={rotate.error} title={`The ${verb.toLowerCase()} was rejected`} />
            <MaterialFields
              idPrefix="rotate"
              purpose={credential.purpose}
              value={material}
              onChange={setMaterial}
              error={secretError}
            />
            {webhook ? (
              <TextField
                id="rotate-overlap"
                label="Overlap window (hours)"
                type="number"
                value={overlap}
                onChange={setOverlap}
                error={hoursError}
                hint={`The previous version keeps verifying inbound webhooks for this long. 0 to ${MAX_OVERLAP_HOURS} hours, revocation overrides it.`}
                className="max-w-xs"
              />
            ) : null}
            {effectiveOrigin(material, credential.purpose) === 'generated' ? (
              <p className="text-muted-foreground text-xs">
                The generated secret is shown once, right after this step.
              </p>
            ) : null}
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => close(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={rotate.isPending}>
                {rotate.isPending ? 'Working...' : verb}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
