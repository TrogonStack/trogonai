import { AuthorizationScheme, BindingReplacementMode, ProviderAccessScope } from '@trogonai/proto';
import { useState } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { RadioCards, type RadioCardOption } from '@/components/field';
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

import { replacementModeLabels } from '../domain/labels';
import type { BindingView, ConnectionView } from '../domain/view';
import { toBindingDraft, type BindingFormValues } from '../forms/binding';
import { useReplaceBinding } from '../queries';
import { BindingFields, bindingErrors } from './binding-fields';

const modeOptions: readonly RadioCardOption<number>[] = [
  BindingReplacementMode.DRAIN,
  BindingReplacementMode.FAIL_IF_IN_USE,
].map((mode) => {
  const [label = '', description] = replacementModeLabels[mode].split(/:\s*/, 2);
  return { value: mode as number, label, description };
});

function initialValues(connection: ConnectionView): BindingFormValues {
  const gateway = connection.accessScope === ProviderAccessScope.GATEWAY_ONLY;
  const local = connection.endpoint.kind === 'deployment-local';
  return {
    bindingId: '',
    authenticationKind: gateway ? 'gateway' : local ? 'deployment-no-credential' : 'stored-api-key',
    secretRef: '',
    authorization: AuthorizationScheme.BEARER_TOKEN,
    gatewayMode: 'configured-key',
  };
}

interface ReplaceBindingDialogProps {
  connection: ConnectionView;
  previous: BindingView;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ReplaceBindingDialog({ connection, previous, open, onOpenChange }: ReplaceBindingDialogProps) {
  const replace = useReplaceBinding();
  const notify = useNotify();
  const [values, setValues] = useState<BindingFormValues>(() => initialValues(connection));
  const [mode, setMode] = useState<BindingReplacementMode>(BindingReplacementMode.DRAIN);
  const [attempted, setAttempted] = useState(false);

  const context = {
    accessScope: connection.accessScope,
    endpointKind: connection.endpoint.kind,
  };

  const close = (next: boolean) => {
    if (!next) {
      replace.reset();
      setAttempted(false);
      setValues(initialValues(connection));
      setMode(BindingReplacementMode.DRAIN);
    }
    onOpenChange(next);
  };

  const submit = async () => {
    setAttempted(true);
    if (Object.keys(bindingErrors(values, context)).length > 0) return;
    try {
      await replace.mutateAsync({
        connectionId: connection.id,
        expectedRevision: connection.revision,
        previousBindingId: previous.id,
        replacement: toBindingDraft(values),
        mode,
      });
      notify.success('Binding replaced', `${previous.id} is being replaced by ${values.bindingId}`);
      close(false);
    } catch {
      return;
    }
  };

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent className="max-h-[90vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Replace binding {previous.id}</DialogTitle>
          <DialogDescription>
            The replacement is validated against the provider, then becomes the active binding.
          </DialogDescription>
        </DialogHeader>
        <form
          noValidate
          aria-label="Replace credential binding"
          className="flex flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <FaultAlert error={replace.error} title="The replacement was rejected" />
          <BindingFields
            idPrefix="replace"
            value={values}
            context={context}
            onChange={setValues}
            showErrors={attempted}
          />
          <RadioCards
            name="replace-mode"
            label="Replacement mode"
            value={mode}
            options={modeOptions}
            columns={2}
            onChange={(next) => setMode(next as BindingReplacementMode)}
            hint={`${previous.pinnedSessions} Session(s) currently pin ${previous.id}.`}
          />
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={replace.isPending}>
              {replace.isPending ? 'Replacing...' : 'Replace binding'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
