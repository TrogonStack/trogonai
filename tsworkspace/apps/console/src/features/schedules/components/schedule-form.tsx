import { useForm } from '@tanstack/react-form';
import { PlusIcon, Trash2Icon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { FaultAlert } from '@/components/fault-alert';
import { FieldError, FieldShell, RadioCards, SelectField, TextField, firstMessage } from '@/components/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Textarea } from '@/components/ui/textarea';

import { emptyScheduleDraft, scheduleDraftSchema, strategyOptions, type ScheduleDraft } from '../draft';
import { CronBuilder } from './cron-builder';
import { RecurrenceBuilder } from './recurrence-builder';
import { InstantField, IntervalField, ExceptionDates, TimezoneField } from './timing-fields';

function Section({ title, description, children }: { title: string; description: string; children: ReactNode }) {
  return (
    <Card>
      <CardHeader className="flex-col items-start gap-1">
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">{children}</CardContent>
    </Card>
  );
}
export function ScheduleForm({
  onSubmit,
  error,
  pending,
}: {
  onSubmit: (draft: ScheduleDraft) => Promise<unknown>;
  error?: unknown;
  pending: boolean;
}) {
  const [defaults] = useState(emptyScheduleDraft);
  const form = useForm({
    defaultValues: defaults,
    validators: { onSubmit: scheduleDraftSchema },
    onSubmit: async ({ value }) => {
      await onSubmit(value);
    },
  });
  const text = (
    name: Exclude<keyof ScheduleDraft, 'headers' | 'kind' | 'status' | 'payloadEncoding'>,
    label: string,
    hint?: string,
    multiline = false,
  ) => (
    <form.Field name={name}>
      {(field) => {
        const error = firstMessage(field.state.meta.errors);
        return multiline ? (
          <FieldShell id={name} label={label} hint={hint} error={error}>
            <Textarea
              id={name}
              value={field.state.value}
              onChange={(event) => field.handleChange(event.target.value)}
              onBlur={field.handleBlur}
              aria-invalid={Boolean(error)}
              aria-describedby={error ? `${name}-error` : hint ? `${name}-hint` : undefined}
              className="min-h-24 font-mono text-xs"
            />
          </FieldShell>
        ) : (
          <TextField
            id={name}
            label={label}
            value={field.state.value}
            onChange={field.handleChange}
            onBlur={field.handleBlur}
            error={error}
            hint={hint}
            mono
          />
        );
      }}
    </form.Field>
  );
  return (
    <form
      className="grid items-start gap-5 lg:grid-cols-[minmax(0,1fr)_18rem]"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void form.handleSubmit();
      }}
    >
      <div className="flex min-w-0 flex-col gap-4">
        <Section title="Timing" description="Choose exactly one strategy. All instants include a timezone offset.">
          <form.Field name="kind">
            {(field) => (
              <RadioCards
                name="strategy"
                label="Schedule strategy"
                columns={2}
                options={strategyOptions}
                value={field.state.value}
                onChange={field.handleChange}
              />
            )}
          </form.Field>
          <form.Subscribe selector={(state) => state.values.kind}>
            {(kind) => (
              <>
                {kind === 'at' ? (
                  <form.Field name="at">
                    {(field) => (
                      <InstantField
                        id="at"
                        label="Fire at"
                        value={field.state.value}
                        onChange={field.handleChange}
                        error={firstMessage(field.state.meta.errors)}
                      />
                    )}
                  </form.Field>
                ) : null}
                {kind === 'every' ? (
                  <form.Field name="everySeconds">
                    {(field) => (
                      <IntervalField
                        value={field.state.value}
                        onChange={field.handleChange}
                        error={firstMessage(field.state.meta.errors)}
                      />
                    )}
                  </form.Field>
                ) : null}
                {kind === 'cron' || kind === 'rrule' ? (
                  <form.Field name="timezone">
                    {(field) => (
                      <TimezoneField
                        value={field.state.value}
                        onChange={field.handleChange}
                        error={firstMessage(field.state.meta.errors)}
                      />
                    )}
                  </form.Field>
                ) : null}
                {kind === 'cron' ? (
                  <form.Subscribe selector={(state) => state.values.timezone}>
                    {(timezone) => (
                      <form.Field name="cron">
                        {(field) => (
                          <CronBuilder
                            value={field.state.value}
                            onChange={field.handleChange}
                            timezone={timezone}
                            error={firstMessage(field.state.meta.errors)}
                          />
                        )}
                      </form.Field>
                    )}
                  </form.Subscribe>
                ) : null}
                {kind === 'rrule' ? (
                  <>
                    <form.Field name="dtstart">
                      {(field) => (
                        <InstantField
                          id="dtstart"
                          label="Recurrence start"
                          value={field.state.value}
                          onChange={field.handleChange}
                          error={firstMessage(field.state.meta.errors)}
                        />
                      )}
                    </form.Field>
                    <form.Field name="rrule">
                      {(field) => (
                        <RecurrenceBuilder
                          value={field.state.value}
                          onChange={field.handleChange}
                          error={firstMessage(field.state.meta.errors)}
                        />
                      )}
                    </form.Field>
                    <details className="border-border rounded-md border p-3">
                      <summary className="cursor-pointer text-sm font-medium">
                        Include or exclude specific dates
                      </summary>
                      <div className="grid gap-4 pt-4 sm:grid-cols-2">
                        <form.Subscribe selector={(state) => state.values.exdate}>
                          {(opposingValue) => (
                            <form.Field name="rdate">
                              {(field) => (
                                <ExceptionDates
                                  id="rdate"
                                  label="Included dates"
                                  value={field.state.value}
                                  opposingValue={opposingValue}
                                  onChange={field.handleChange}
                                  error={firstMessage(field.state.meta.errors)}
                                />
                              )}
                            </form.Field>
                          )}
                        </form.Subscribe>
                        <form.Subscribe selector={(state) => state.values.rdate}>
                          {(opposingValue) => (
                            <form.Field name="exdate">
                              {(field) => (
                                <ExceptionDates
                                  id="exdate"
                                  label="Excluded dates"
                                  value={field.state.value}
                                  opposingValue={opposingValue}
                                  onChange={field.handleChange}
                                  error={firstMessage(field.state.meta.errors)}
                                />
                              )}
                            </form.Field>
                          )}
                        </form.Subscribe>
                      </div>
                    </details>
                  </>
                ) : null}
                {kind === 'cron' || kind === 'rrule' ? (
                  <details>
                    <summary className="text-muted-foreground cursor-pointer text-xs">
                      Advanced timezone settings
                    </summary>
                    <div className="pt-3">
                      {text(
                        'timezoneVersion',
                        'Timezone database version',
                        'Optional tzdb version recorded with the definition.',
                      )}
                    </div>
                  </details>
                ) : null}
              </>
            )}
          </form.Subscribe>
          <p className="text-muted-foreground text-xs">
            This mock checks basic syntax only. Cron and recurrence expansion require the scheduler; no fire-time
            preview is calculated here.
          </p>
        </Section>
        <Section title="Delivery" description="Publish to a concrete NATS subject when the schedule fires.">
          {text('subject', 'Destination subject', 'No wildcard tokens or whitespace.')}
          <div className="grid gap-4 sm:grid-cols-2">
            {text('ttlSeconds', 'Message TTL in seconds', 'Optional. Leave empty for no explicit TTL.')}
            {text(
              'sourceSubject',
              'Sample latest from subject',
              'Optional. Uses the latest payload on this subject instead of static content.',
            )}
          </div>
        </Section>
        <Section
          title="Message"
          description="Static content and additional headers recorded in the schedule definition."
        >
          <form.Subscribe selector={(state) => state.values.sourceSubject}>
            {(source) =>
              source ? (
                <p
                  role="status"
                  className="border-warning/30 bg-warning-soft text-warning-ink rounded-md border p-3 text-xs"
                >
                  Latest-message sampling is configured. Static content remains part of the definition; the sampled
                  payload is used at fire time.
                </p>
              ) : null
            }
          </form.Subscribe>
          <div className="grid gap-4 sm:grid-cols-2">
            {text('contentType', 'Content type', 'For example application/json or application/octet-stream.')}
            <form.Field name="payloadEncoding">
              {(field) => (
                <SelectField
                  id="payloadEncoding"
                  label="Payload encoding"
                  value={field.state.value}
                  options={[
                    { value: 'utf8', label: 'UTF-8 text' },
                    { value: 'base64', label: 'Base64 bytes' },
                  ]}
                  onChange={field.handleChange}
                />
              )}
            </form.Field>
          </div>
          {text('payload', 'Payload', 'Opaque content. JSON text is not interpreted by the scheduler.', true)}
          <form.Field name="headers" mode="array">
            {(field) => (
              <div className="flex flex-col gap-3">
                <div className="flex items-center justify-between gap-2">
                  <span className="text-sm font-medium">Headers</span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => field.pushValue({ name: '', value: '' })}
                  >
                    <PlusIcon />
                    Add header
                  </Button>
                </div>
                <p className="text-muted-foreground text-xs">
                  Content-Type and Nats-Schedule-* are reserved. Duplicate names remain separate entries.
                </p>
                {field.state.value.map((_, index) => (
                  <div key={index} className="grid items-start gap-2 sm:grid-cols-[1fr_1fr_auto]">
                    <form.Field name={`headers[${index}].name`}>
                      {(header) => (
                        <TextField
                          id={`header-${index}-name`}
                          label={`Header ${index + 1} name`}
                          value={header.state.value}
                          onChange={header.handleChange}
                          error={firstMessage(header.state.meta.errors)}
                        />
                      )}
                    </form.Field>
                    <form.Field name={`headers[${index}].value`}>
                      {(header) => (
                        <TextField
                          id={`header-${index}-value`}
                          label={`Header ${index + 1} value`}
                          value={header.state.value}
                          onChange={header.handleChange}
                          error={firstMessage(header.state.meta.errors)}
                        />
                      )}
                    </form.Field>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      aria-label={`Remove header ${index + 1}`}
                      onClick={() => field.removeValue(index)}
                    >
                      <Trash2Icon />
                    </Button>
                  </div>
                ))}
              </div>
            )}
          </form.Field>
        </Section>
      </div>
      <aside className="flex flex-col gap-4 lg:sticky lg:top-20">
        <Section title="Create schedule" description="A definition persists until it is removed.">
          {text('scheduleId', 'Schedule id', 'Generated UUID. You can replace it with another UUID.')}
          <form.Field name="status">
            {(field) => (
              <SelectField
                id="initial-status"
                label="Initial status"
                value={field.state.value}
                options={[
                  { value: 'scheduled', label: 'Scheduled' },
                  { value: 'paused', label: 'Paused' },
                ]}
                onChange={field.handleChange}
              />
            )}
          </form.Field>
          <p className="text-muted-foreground text-xs">
            In-memory demo. Reloading resets your changes. Creating a schedule does not publish a message.
          </p>
          {error ? <FaultAlert error={error} title="Could not create schedule" /> : null}
          <form.Subscribe selector={(state) => [state.isSubmitting, state.isSubmitted, state.isValid] as const}>
            {([submitting, submitted, valid]) => (
              <>
                {submitted && !valid ? (
                  <FieldError>Check the highlighted fields before creating the schedule.</FieldError>
                ) : null}
                <Button type="submit" disabled={pending || submitting}>
                  {pending || submitting ? 'Creating…' : 'Create schedule'}
                </Button>
              </>
            )}
          </form.Subscribe>
        </Section>
      </aside>
    </form>
  );
}
