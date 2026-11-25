import { CalendarClockIcon, Code2Icon, SlidersHorizontalIcon } from 'lucide-react';
import { useState, type ReactNode } from 'react';

import { FieldError, FieldShell } from '@/components/field';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Segmented } from '@/components/ui/segmented';
import { TimeInput } from '@/components/ui/time-input';

import { weekdays, type Weekday } from '../builders';

export function useExpressionEditor<T>(
  expression: string,
  parse: (value: string) => T | undefined,
  defaults: () => T,
  onChange: (value: string) => void,
  serialize: (value: T) => string,
) {
  const [mode, setMode] = useState<'guided' | 'advanced'>(() => (parse(expression) ? 'guided' : 'advanced'));
  const [draft, setDraft] = useState<T>(() => parse(expression) ?? defaults());
  const [supported, setSupported] = useState(() => parse(expression) !== undefined);
  const update = (patch: Partial<T>) => {
    const next = { ...draft, ...patch };
    setDraft(next);
    onChange(serialize(next));
  };
  const changeMode = (next: 'guided' | 'advanced') => {
    if (next === 'guided') {
      const parsed = parse(expression);
      if (parsed) setDraft(parsed);
      setSupported(parsed !== undefined);
    }
    setMode(next);
  };
  const replace = () => {
    const next = defaults();
    setSupported(true);
    setDraft(next);
    onChange(serialize(next));
  };
  return { mode, draft, update, changeMode, replace, supported };
}
export function BuilderFrame({
  id,
  title,
  mode,
  onModeChange,
  supported,
  onReplace,
  summary,
  error,
  children,
}: {
  id: string;
  title: string;
  mode: 'guided' | 'advanced';
  onModeChange: (value: 'guided' | 'advanced') => void;
  supported: boolean;
  onReplace: () => void;
  summary: string;
  error?: string;
  children: ReactNode;
}) {
  return (
    <div className="border-border bg-surface-deep/30 flex min-w-0 flex-col gap-4 rounded-lg border p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <span className="flex items-center gap-2 text-sm font-medium">
          <CalendarClockIcon className="text-primary-ink size-4" />
          {title}
        </span>
        <Segmented
          label={`${title} editor`}
          value={mode}
          onChange={onModeChange}
          options={[
            { value: 'guided', label: 'Builder', icon: <SlidersHorizontalIcon /> },
            { value: 'advanced', label: 'Expression', icon: <Code2Icon /> },
          ]}
        />
      </div>
      {mode === 'guided' && !supported ? (
        <div className="border-warning/30 bg-warning-soft text-warning-ink flex flex-col gap-3 rounded-md border p-3 text-xs">
          <p>
            This expression uses options outside this builder. It is preserved exactly. Continue in Expression mode, or
            explicitly replace it with a starter pattern.
          </p>
          <Button type="button" variant="outline" size="sm" className="w-fit" onClick={onReplace}>
            Replace with starter pattern
          </Button>
        </div>
      ) : null}
      {children}
      <div
        role="status"
        aria-live="polite"
        className="border-border bg-card flex flex-col gap-1 rounded-md border px-3 py-2.5"
      >
        <span className="micro text-primary-ink text-[10px]">Schedule summary</span>
        <p className="text-sm leading-5">{summary}</p>
      </div>
      {error ? <FieldError id={`${id}-error`}>{error}</FieldError> : null}
    </div>
  );
}
export function NumberControl({
  id,
  label,
  value,
  onChange,
  min = 1,
  max = 2147483647,
  hint,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  min?: number;
  max?: number;
  hint?: string;
}) {
  const invalid = value === '' || !/^\d+$/.test(value) || Number(value) < min || Number(value) > max;
  return (
    <FieldShell
      id={id}
      label={label}
      hint={hint}
      error={invalid ? `Use a whole number from ${min} to ${max}.` : undefined}
    >
      <Input
        id={id}
        type="number"
        min={min}
        max={max}
        step="1"
        value={value}
        onChange={(event) => onChange(event.target.value)}
        aria-invalid={invalid || undefined}
        aria-describedby={hint ? `${id}-hint` : undefined}
      />
    </FieldShell>
  );
}
export function TimeControl({
  id,
  label,
  value,
  onChange,
}: {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <FieldShell id={id} label={label}>
      <TimeInput id={id} label={label} value={value} onChange={onChange} />
    </FieldShell>
  );
}
export function WeekdayPicker({
  value,
  onChange,
}: {
  value: readonly Weekday[];
  onChange: (value: Weekday[]) => void;
}) {
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="text-sm font-medium">Repeat on</legend>
      <div className="flex flex-wrap gap-1.5">
        {weekdays.map((day) => (
          <Button
            key={day.value}
            type="button"
            size="sm"
            variant={value.includes(day.value) ? 'default' : 'outline'}
            aria-label={day.label}
            aria-pressed={value.includes(day.value)}
            onClick={() =>
              onChange(
                value.includes(day.value)
                  ? value.filter((entry) => entry !== day.value)
                  : weekdays
                      .filter((entry) => entry.value === day.value || value.includes(entry.value))
                      .map((entry) => entry.value),
              )
            }
          >
            {day.short}
          </Button>
        ))}
      </div>
      <p className="text-muted-foreground text-xs">Leave all days unselected to use the start date’s weekday.</p>
    </fieldset>
  );
}
