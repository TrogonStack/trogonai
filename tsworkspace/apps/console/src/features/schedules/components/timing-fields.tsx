import { useId, useState } from 'react';

import { FieldShell, SelectField, TextField } from '@/components/field';
import { Button } from '@/components/ui/button';
import { DateTimePicker } from '@/components/ui/date-time-picker';
import { Input } from '@/components/ui/input';

import { intervalUnits, secondsToUnit, unitToSeconds } from '../builders';

export function InstantField({
  id,
  label,
  value,
  onChange,
  error,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  error?: string;
}) {
  const [precise, setPrecise] = useState(() => value !== '' && (!value.endsWith('Z') || /\.\d+/.test(value)));
  const pickerValue = value && Number.isFinite(Date.parse(value)) ? new Date(value).toISOString().slice(0, 19) : '';
  return (
    <div className="flex flex-col gap-2">
      <FieldShell
        id={id}
        label={label}
        error={error}
        hint={
          precise
            ? 'ISO 8601 with Z or an offset. Fractional seconds are preserved.'
            : 'Choose the instant in UTC. This picker does not use your browser or recurrence timezone.'
        }
      >
        {precise ? (
          <Input
            id={id}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            aria-invalid={Boolean(error)}
            aria-describedby={error ? `${id}-error` : `${id}-hint`}
          />
        ) : (
          <DateTimePicker
            id={id}
            label={label}
            value={pickerValue}
            onChange={(fields) => onChange(`${fields}Z`)}
            zoneLabel="UTC"
            invalid={Boolean(error)}
            describedBy={error ? `${id}-error` : `${id}-hint`}
          />
        )}
      </FieldShell>
      <Button type="button" variant="ghost" size="sm" className="w-fit" onClick={() => setPrecise(!precise)}>
        {precise ? 'Use UTC date picker' : 'Use precise ISO input'}
      </Button>
    </div>
  );
}
export function IntervalField({
  value,
  onChange,
  error,
}: {
  value: string;
  onChange: (value: string) => void;
  error?: string;
}) {
  const [unit, setUnit] = useState(
    () =>
      [...intervalUnits].reverse().find((entry) => {
        const converted = secondsToUnit(value, entry.value);
        return converted !== '' && !converted.includes('.');
      })?.value ?? '1',
  );
  const amount = secondsToUnit(value, unit);
  return (
    <div className="flex flex-col gap-3">
      <div className="grid gap-4 sm:grid-cols-2">
        <FieldShell id="interval-amount" label="Repeat every" error={error}>
          <Input
            id="interval-amount"
            type="number"
            step="any"
            min="0"
            value={amount}
            onChange={(event) => onChange(unitToSeconds(event.target.value, unit))}
            aria-invalid={Boolean(error)}
          />
        </FieldShell>
        <SelectField
          id="interval-unit"
          label="Interval unit"
          value={unit}
          options={intervalUnits}
          onChange={(next) => {
            if (secondsToUnit(value, next) !== '') setUnit(next);
          }}
          hint="Changing units preserves the exact duration."
        />
      </div>
      <p role="status" className="text-muted-foreground font-mono text-xs">
        {value || '…'} seconds between fires. Fixed durations do not follow daylight saving changes.
      </p>
      <details>
        <summary className="text-muted-foreground cursor-pointer text-xs">Precise seconds</summary>
        <div className="pt-2">
          <TextField
            id="everySeconds"
            label="Interval in seconds"
            value={value}
            onChange={(seconds) => {
              setUnit('1');
              onChange(seconds);
            }}
            error={error}
            hint="Positive seconds with up to nine decimal places."
            mono
          />
        </div>
      </details>
    </div>
  );
}
export { ExceptionDates } from './exception-dates';
export function TimezoneField({
  value,
  onChange,
  error,
}: {
  value: string;
  onChange: (value: string) => void;
  error?: string;
}) {
  const listId = useId();
  return (
    <FieldShell
      id="timezone"
      label="Timezone"
      error={error}
      hint="Calendar patterns follow this zone’s wall clock, including daylight saving changes. Start and exception dates are explicit instants."
    >
      <Input
        id="timezone"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        list={listId}
        placeholder="UTC"
        aria-invalid={Boolean(error)}
        aria-describedby={error ? 'timezone-error' : 'timezone-hint'}
      />
      <datalist id={listId}>
        {[
          'UTC',
          'America/New_York',
          'America/Chicago',
          'America/Los_Angeles',
          'Europe/London',
          'Europe/Paris',
          'Asia/Dubai',
          'Asia/Tokyo',
          'Australia/Sydney',
        ].map((zone) => (
          <option key={zone} value={zone} />
        ))}
      </datalist>
    </FieldShell>
  );
}
