import { useForm } from '@tanstack/react-form';
import { ProviderRoutingEligibility } from '@trogonai/proto';
import { SaveIcon } from 'lucide-react';
import { z } from 'zod';

import { FaultAlert } from '@/components/fault-alert';
import { FieldShell, TextField, firstMessage } from '@/components/field';
import { Button } from '@/components/ui/button';
import { Segmented, type SegmentedOption } from '@/components/ui/segmented';
import { Textarea } from '@/components/ui/textarea';
import { useNotify } from '@/components/ui/toaster';

import { eligibilityLabels } from '../domain/labels';
import type { ConnectionView } from '../domain/view';
import { useSetProfile, useSetRoutingPolicy } from '../queries';

const policySchema = z.object({
  preference: z
    .number({ error: 'Preference must be a whole number.' })
    .int('Preference must be a whole number.')
    .min(-(2 ** 31), 'Preference is out of range.')
    .max(2 ** 31 - 1, 'Preference is out of range.'),
  eligibility: z.number(),
});

const eligibilityOptions: readonly SegmentedOption<string>[] = [
  ProviderRoutingEligibility.ENABLED,
  ProviderRoutingEligibility.DISABLED,
].map((value) => ({ value: String(value), label: eligibilityLabels[value] }));

function SaveRow({ dirty, submitting, label }: { dirty: boolean; submitting: boolean; label: string }) {
  return (
    <div className="flex items-center justify-between gap-3">
      <span className="micro text-muted-foreground text-[10px]">{dirty ? 'Unsaved changes' : 'Up to date'}</span>
      <Button type="submit" size="sm" aria-label={label} disabled={!dirty || submitting}>
        <SaveIcon data-icon="inline-start" />
        {submitting ? 'Saving...' : 'Save'}
      </Button>
    </div>
  );
}

function Locked() {
  return <p className="micro text-muted-foreground text-[10px]">A revoked connection can no longer be edited.</p>;
}

export function ProfileForm({ connection, editable }: { connection: ConnectionView; editable: boolean }) {
  const setProfile = useSetProfile();
  const notify = useNotify();

  const form = useForm({
    defaultValues: { displayName: connection.displayName, description: connection.description },
    onSubmit: async ({ value, formApi }) => {
      setProfile.reset();
      try {
        await setProfile.mutateAsync({
          connectionId: connection.id,
          expectedRevision: connection.revision,
          profile: { displayName: value.displayName.trim(), description: value.description.trim() },
        });
        formApi.reset(value);
        notify.success('Profile saved');
      } catch (error) {
        notify.failure('Profile was rejected', error instanceof Error ? error.message : undefined);
      }
    },
  });

  return (
    <form
      aria-label="Profile"
      noValidate
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <FaultAlert error={setProfile.error} title="The profile was rejected" />
      <form.Field name="displayName">
        {(field) => (
          <TextField
            id="profile-display-name"
            label="Display name"
            value={field.state.value}
            onChange={field.handleChange}
            disabled={!editable}
          />
        )}
      </form.Field>
      <form.Field name="description">
        {(field) => (
          <FieldShell id="profile-description" label="Description">
            <Textarea
              id="profile-description"
              rows={3}
              value={field.state.value}
              disabled={!editable}
              onChange={(event) => field.handleChange(event.target.value)}
            />
          </FieldShell>
        )}
      </form.Field>
      {editable ? (
        <form.Subscribe selector={(state) => ({ dirty: state.isDirty, submitting: state.isSubmitting })}>
          {({ dirty, submitting }) => <SaveRow dirty={dirty} submitting={submitting} label="Save profile" />}
        </form.Subscribe>
      ) : (
        <Locked />
      )}
    </form>
  );
}

export function RoutingPolicyForm({ connection, editable }: { connection: ConnectionView; editable: boolean }) {
  const setPolicy = useSetRoutingPolicy();
  const notify = useNotify();

  const form = useForm({
    defaultValues: { preference: connection.preference, eligibility: connection.eligibility as number },
    validators: { onChange: policySchema },
    onSubmit: async ({ value, formApi }) => {
      setPolicy.reset();
      try {
        await setPolicy.mutateAsync({
          connectionId: connection.id,
          expectedRevision: connection.revision,
          policy: { preference: value.preference, eligibility: value.eligibility as ProviderRoutingEligibility },
        });
        formApi.reset(value);
        notify.success('Routing policy saved');
      } catch (error) {
        notify.failure('Routing policy was rejected', error instanceof Error ? error.message : undefined);
      }
    },
  });

  return (
    <form
      aria-label="Routing policy"
      noValidate
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <FaultAlert error={setPolicy.error} title="The routing policy was rejected" />
      <form.Field name="preference">
        {(field) => (
          <TextField
            id="policy-preference"
            label="Routing preference"
            type="number"
            value={field.state.value}
            disabled={!editable}
            hint="Higher values are preferred when several connections can serve a request."
            onChange={(value) => field.handleChange(value === '' ? Number.NaN : Number(value))}
            error={firstMessage(field.state.meta.errors)}
          />
        )}
      </form.Field>
      <form.Field name="eligibility">
        {(field) => (
          <div className="flex flex-col gap-1.5">
            <span className="micro text-muted-foreground">Routing eligibility</span>
            <Segmented
              label="Routing eligibility"
              value={String(field.state.value)}
              options={eligibilityOptions}
              disabled={!editable}
              className="w-fit"
              onChange={(value) => field.handleChange(Number(value))}
            />
          </div>
        )}
      </form.Field>
      {editable ? (
        <form.Subscribe
          selector={(state) => ({ dirty: state.isDirty, submitting: state.isSubmitting, valid: state.canSubmit })}
        >
          {({ dirty, submitting, valid }) => (
            <SaveRow dirty={dirty && valid} submitting={submitting} label="Save routing policy" />
          )}
        </form.Subscribe>
      ) : (
        <Locked />
      )}
    </form>
  );
}
