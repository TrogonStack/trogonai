import { FieldShell, SelectField } from '@/components/field';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';

import { cronExpression, cronSummary, defaultCron, parseCron, weekdays, type CronPattern } from '../builders';
import { BuilderFrame, NumberControl, TimeControl, useExpressionEditor } from './builder-controls';
const patterns: { value: CronPattern; label: string }[] = [
  { value: 'minutes', label: 'Every few minutes' },
  { value: 'hourly', label: 'Hourly' },
  { value: 'daily', label: 'Daily' },
  { value: 'weekdays', label: 'Weekdays' },
  { value: 'weekly', label: 'Weekly' },
  { value: 'monthly', label: 'Monthly' },
];
export function CronBuilder({
  value,
  onChange,
  error,
  timezone,
}: {
  value: string;
  onChange: (value: string) => void;
  error?: string;
  timezone: string;
}) {
  const editor = useExpressionEditor(value, parseCron, defaultCron, onChange, cronExpression);
  const { draft } = editor;
  const summaryDraft = editor.mode === 'guided' && editor.supported ? draft : parseCron(value);
  return (
    <BuilderFrame
      id="cron"
      title="Cron schedule"
      mode={editor.mode}
      onModeChange={editor.changeMode}
      supported={editor.supported}
      onReplace={editor.replace}
      summary={
        editor.mode === 'guided' && editor.supported && !parseCron(value)
          ? 'Incomplete pattern. Check the highlighted controls.'
          : summaryDraft
            ? `${cronSummary(summaryDraft)} (${timezone || 'scheduler timezone'})`
            : 'Custom cron expression. The scheduler validates its timing.'
      }
      error={error}
    >
      {editor.mode === 'guided' && editor.supported ? (
        <>
          <SelectField
            id="cron-pattern"
            label="Pattern"
            value={draft.pattern}
            options={patterns}
            onChange={(pattern) => editor.update({ pattern })}
          />
          {draft.pattern === 'minutes' ? (
            <NumberControl
              id="cron-every"
              label="Every how many minutes"
              min={1}
              max={59}
              value={draft.every}
              onChange={(every) => editor.update({ every })}
            />
          ) : null}
          {draft.pattern === 'hourly' ? (
            <NumberControl
              id="cron-minute"
              label="Minute of the hour"
              min={0}
              max={59}
              value={draft.minute}
              onChange={(minute) => editor.update({ minute })}
            />
          ) : null}
          {!['minutes', 'hourly'].includes(draft.pattern) ? (
            <div className="grid gap-4 sm:grid-cols-2">
              <TimeControl
                id="cron-time"
                label="Time of day"
                value={draft.time}
                onChange={(time) => editor.update({ time })}
              />
              {draft.pattern === 'weekly' ? (
                <SelectField
                  id="cron-weekday"
                  label="Day of the week"
                  value={draft.weekday}
                  options={weekdays.map((day) => ({ value: day.cron, label: day.label }))}
                  onChange={(weekday) => editor.update({ weekday })}
                />
              ) : null}
              {draft.pattern === 'monthly' ? (
                <NumberControl
                  id="cron-day"
                  label="Day of the month"
                  min={1}
                  max={31}
                  value={draft.day}
                  onChange={(day) => editor.update({ day })}
                  hint="Months without this date are skipped."
                />
              ) : null}
            </div>
          ) : null}
          <FieldShell
            id="cron"
            label="Cron expression"
            hint="Generated with seconds first. Use Expression mode to edit any field."
          >
            <Input id="cron" value={value} readOnly className="font-mono text-xs" />
          </FieldShell>
        </>
      ) : editor.mode === 'advanced' ? (
        <>
          <FieldShell
            id="cron"
            label="Cron expression"
            hint="Six fields with seconds first; a seventh year field is optional."
            error={undefined}
          >
            <Textarea
              id="cron"
              value={value}
              onChange={(event) => onChange(event.target.value)}
              aria-invalid={Boolean(error)}
              aria-describedby={error ? 'cron-error' : 'cron-hint'}
              className="min-h-20 font-mono text-xs"
            />
          </FieldShell>
          <div className="grid grid-cols-3 gap-2 sm:grid-cols-6">
            {[
              ['Second', '0–59'],
              ['Minute', '0–59'],
              ['Hour', '0–23'],
              ['Day', '1–31'],
              ['Month', '1–12'],
              ['Weekday', 'MON–SUN'],
            ].map(([name, range]) => (
              <div key={name} className="border-border rounded-md border p-2 text-center">
                <p className="text-muted-foreground text-[10px]">{name}</p>
                <p className="font-mono text-xs">{range}</p>
              </div>
            ))}
          </div>
          <p className="text-muted-foreground text-xs">
            * means every value, / is a step, commas list values, and hyphens define a range. Custom seconds, lists and
            years remain in Expression mode.
          </p>
        </>
      ) : null}
    </BuilderFrame>
  );
}
