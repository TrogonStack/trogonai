import { FieldShell, SelectField } from '@/components/field';
import { DateTimePicker } from '@/components/ui/date-time-picker';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';

import {
  defaultRecurrence,
  parseRecurrence,
  recurrenceExpression,
  recurrenceSummary,
  weekdays,
  type RecurrenceFrequency,
  type Weekday,
} from '../builders';
import { BuilderFrame, NumberControl, WeekdayPicker, useExpressionEditor } from './builder-controls';

export function RecurrenceBuilder({
  value,
  onChange,
  error,
}: {
  value: string;
  onChange: (value: string) => void;
  error?: string;
}) {
  const editor = useExpressionEditor(value, parseRecurrence, defaultRecurrence, onChange, recurrenceExpression);
  const { draft } = editor;
  const summaryDraft = editor.mode === 'guided' && editor.supported ? draft : parseRecurrence(value);
  return (
    <BuilderFrame
      id="rrule"
      title="Recurrence rule"
      mode={editor.mode}
      onModeChange={editor.changeMode}
      supported={editor.supported}
      onReplace={editor.replace}
      summary={
        editor.mode === 'guided' && editor.supported && !parseRecurrence(value)
          ? 'Incomplete pattern. Check the highlighted controls.'
          : summaryDraft
            ? recurrenceSummary(summaryDraft)
            : 'Custom recurrence rule. The scheduler validates its timing.'
      }
      error={error}
    >
      {editor.mode === 'guided' && editor.supported ? (
        <>
          <div className="grid gap-4 sm:grid-cols-2">
            <SelectField
              id="rrule-frequency"
              label="Frequency"
              value={draft.frequency}
              options={[
                { value: 'DAILY', label: 'Daily' },
                { value: 'WEEKLY', label: 'Weekly' },
                { value: 'MONTHLY', label: 'Monthly' },
                { value: 'YEARLY', label: 'Yearly' },
              ]}
              onChange={(frequency: RecurrenceFrequency) => editor.update({ frequency })}
            />
            <NumberControl
              id="rrule-interval"
              label="Repeat every"
              value={draft.interval}
              onChange={(interval) => editor.update({ interval })}
              hint={`Number of ${({ DAILY: 'days', WEEKLY: 'weeks', MONTHLY: 'months', YEARLY: 'years' } as const)[draft.frequency]} between repeats.`}
            />
          </div>
          {draft.frequency === 'WEEKLY' ? (
            <WeekdayPicker value={draft.days} onChange={(days) => editor.update({ days })} />
          ) : null}
          {draft.frequency === 'MONTHLY' || draft.frequency === 'YEARLY' ? (
            <>
              <SelectField
                id="rrule-month-mode"
                label="Repeat by"
                value={draft.monthMode}
                options={[
                  { value: 'start', label: 'Calendar day from start date' },
                  { value: 'day', label: 'Day of the month' },
                  { value: 'ordinal', label: 'Weekday position' },
                ]}
                onChange={(monthMode) => editor.update({ monthMode })}
              />
              <div className="grid gap-4 sm:grid-cols-2">
                {draft.frequency === 'YEARLY' && draft.monthMode !== 'start' ? (
                  <SelectField
                    id="rrule-month"
                    label="Month"
                    value={draft.month}
                    options={Array.from({ length: 12 }, (_, index) => ({
                      value: String(index + 1),
                      label: new Intl.DateTimeFormat('en', { month: 'long', timeZone: 'UTC' }).format(
                        new Date(Date.UTC(2027, index, 1)),
                      ),
                    }))}
                    onChange={(month) => editor.update({ month })}
                  />
                ) : null}
                {draft.monthMode === 'day' ? (
                  <SelectField
                    id="rrule-day"
                    label="Day of the month"
                    value={draft.day}
                    options={[
                      ...Array.from({ length: 31 }, (_, index) => ({
                        value: String(index + 1),
                        label: String(index + 1),
                      })),
                      { value: '-1', label: 'Last day' },
                    ]}
                    onChange={(day) => editor.update({ day })}
                  />
                ) : null}
                {draft.monthMode === 'ordinal' ? (
                  <>
                    <SelectField
                      id="rrule-position"
                      label="Position"
                      value={draft.ordinal}
                      options={[
                        { value: '1', label: 'First' },
                        { value: '2', label: 'Second' },
                        { value: '3', label: 'Third' },
                        { value: '4', label: 'Fourth' },
                        { value: '-1', label: 'Last' },
                      ]}
                      onChange={(ordinal) => editor.update({ ordinal })}
                    />
                    <SelectField
                      id="rrule-weekday"
                      label="Weekday"
                      value={draft.weekday}
                      options={weekdays.map((day) => ({ value: day.value, label: day.label }))}
                      onChange={(weekday: Weekday) => editor.update({ weekday })}
                    />
                  </>
                ) : null}
              </div>
              <p className="text-muted-foreground text-xs">
                Dates that do not exist in a month are skipped.{' '}
                {draft.monthMode === 'start'
                  ? 'The month and day are taken from the recurrence start when needed.'
                  : null}
              </p>
            </>
          ) : null}
          <div className="border-border grid gap-4 border-t pt-4 sm:grid-cols-2">
            <SelectField
              id="rrule-end"
              label="Ends"
              value={draft.end}
              options={[
                { value: 'never', label: 'Never' },
                { value: 'count', label: 'After a number of occurrences' },
                { value: 'until', label: 'On a date and time' },
              ]}
              onChange={(end) => editor.update({ end })}
            />
            {draft.end === 'count' ? (
              <NumberControl
                id="rrule-count"
                label="Occurrence count"
                value={draft.count}
                onChange={(count) => editor.update({ count })}
                hint="COUNT counts generated rule occurrences, including the start when it matches; extra dates are separate."
              />
            ) : null}
            {draft.end === 'until' ? (
              <FieldShell
                id="rrule-until"
                label="End at (UTC)"
                hint="Inclusive UTC cutoff, regardless of the recurrence timezone."
              >
                <DateTimePicker
                  id="rrule-until"
                  label="End at (UTC)"
                  value={draft.until}
                  onChange={(until) => editor.update({ until })}
                  zoneLabel="UTC"
                  describedBy="rrule-until-hint"
                />
              </FieldShell>
            ) : null}
          </div>
          <FieldShell
            id="rrule"
            label="Recurrence rule"
            hint="Generated RFC 5545 rule. Use Expression mode for additional clauses."
          >
            <Input id="rrule" readOnly value={value} className="font-mono text-xs" />
          </FieldShell>
        </>
      ) : editor.mode === 'advanced' ? (
        <>
          <FieldShell
            id="rrule"
            label="Recurrence rule"
            hint="RFC 5545 value without RRULE:. COUNT and UNTIL cannot appear together."
          >
            <Textarea
              id="rrule"
              value={value}
              onChange={(event) => onChange(event.target.value)}
              aria-invalid={Boolean(error)}
              aria-describedby={error ? 'rrule-error' : 'rrule-hint'}
              className="min-h-24 font-mono text-xs"
            />
          </FieldShell>
          <p className="text-muted-foreground text-xs">
            Use FREQ, INTERVAL and BYDAY to describe the pattern. End with COUNT or a UTC UNTIL such as
            20271231T235959Z. Additional clauses stay intact when switching editor modes.
          </p>
        </>
      ) : null}
    </BuilderFrame>
  );
}
