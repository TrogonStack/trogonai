import { useForm } from '@tanstack/react-form';

import { FaultAlert } from '@/components/fault-alert';
import { RadioCards, TextField, firstMessage } from '@/components/field';
import { Button } from '@/components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog';

import { webhookDraft, webhookDraftSchema, type WebhookDraft, type WebhookEndpoint } from '../domain/webhook-endpoint';

export function WebhookForm({
  endpoint,
  pending,
  error,
  onSubmit,
  onCancel,
}: {
  endpoint?: WebhookEndpoint;
  pending: boolean;
  error: unknown;
  onSubmit: (draft: WebhookDraft) => Promise<void>;
  onCancel: () => void;
}) {
  const form = useForm({
    defaultValues: webhookDraft(endpoint),
    validators: { onSubmit: webhookDraftSchema },
    onSubmit: async ({ value }) => onSubmit(value),
  });
  return (
    <form
      aria-label={endpoint ? 'Edit webhook endpoint' : 'Create webhook endpoint'}
      noValidate
      className="flex flex-col gap-5"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field name="displayName">
        {(field) => (
          <TextField
            id="webhook-name"
            label="Endpoint name"
            value={field.state.value}
            onChange={field.handleChange}
            onBlur={field.handleBlur}
            error={firstMessage(field.state.meta.errors)}
            placeholder="Orders service"
          />
        )}
      </form.Field>
      <form.Field name="authentication">
        {(field) =>
          endpoint ? (
            <p className="text-muted-foreground text-xs">
              Authentication: {endpoint.authentication.mode === 'hmac' ? 'HMAC-SHA256' : 'Bearer token'}. Authentication
              changes are not modeled in this preview.
            </p>
          ) : (
            <RadioCards
              name="webhook-auth"
              label="Authentication"
              value={field.state.value}
              onChange={field.handleChange}
              columns={2}
              options={[
                {
                  value: 'hmac',
                  label: 'HMAC-SHA256',
                  description: 'Sign the delivery ID, timestamp and original body.',
                },
                {
                  value: 'bearer',
                  label: 'Bearer token',
                  description: 'Send the shared token in the Authorization header.',
                },
              ]}
            />
          )
        }
      </form.Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <form.Field name="maxBodyBytes">
          {(field) => (
            <TextField
              id="webhook-limit"
              label="Maximum body in bytes"
              type="number"
              value={field.state.value}
              onChange={field.handleChange}
              error={firstMessage(field.state.meta.errors)}
              hint="1 to 1048576 bytes (1 MiB)."
            />
          )}
        </form.Field>
        <form.Subscribe selector={(state) => state.values.authentication}>
          {(authentication) =>
            authentication === 'hmac' ? (
              <form.Field name="timestampToleranceSeconds">
                {(field) => (
                  <TextField
                    id="webhook-tolerance"
                    label="Timestamp tolerance in seconds"
                    type="number"
                    value={field.state.value}
                    onChange={field.handleChange}
                    error={firstMessage(field.state.meta.errors)}
                    hint="1 to 3600 seconds, in either clock direction."
                  />
                )}
              </form.Field>
            ) : null
          }
        </form.Subscribe>
      </div>
      <p className="text-muted-foreground text-xs">
        In-memory preview. Changes reset when this website reloads. No real endpoint or credential is provisioned.
      </p>
      <FaultAlert error={error} title="Could not save endpoint" />
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" onClick={onCancel}>
          Cancel
        </Button>
        <form.Subscribe selector={(state) => state.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={pending || submitting}>
              {pending || submitting ? 'Saving...' : endpoint ? 'Save changes' : 'Create endpoint'}
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

export function WebhookFormDialog({
  open,
  onOpenChange,
  endpoint,
  pending,
  error,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  endpoint?: WebhookEndpoint;
  pending: boolean;
  error: unknown;
  onSubmit: (draft: WebhookDraft) => Promise<void>;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-xl">
        <DialogHeader>
          <DialogTitle>{endpoint ? 'Edit endpoint' : 'Create a generic webhook'}</DialogTitle>
          <DialogDescription>
            Receive events from your own system, without a provider-specific connector.
          </DialogDescription>
        </DialogHeader>
        {open ? (
          <WebhookForm
            key={endpoint?.id ?? 'new'}
            endpoint={endpoint}
            pending={pending}
            error={error}
            onSubmit={onSubmit}
            onCancel={() => onOpenChange(false)}
          />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
