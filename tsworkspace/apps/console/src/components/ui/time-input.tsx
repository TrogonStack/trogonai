import { NumberField } from '@base-ui/react/number-field';

export interface TimeInputProps {
  id: string;
  label: string;
  value: string;
  onChange: (value: string) => void;
  seconds?: boolean;
  invalid?: boolean;
  describedBy?: string;
}
export function validTime(value: string, seconds = true): boolean {
  return (seconds ? /^(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d$/ : /^(?:[01]\d|2[0-3]):[0-5]\d$/).test(value);
}
export function TimeInput({ id, label, value, onChange, seconds = false, invalid, describedBy }: TimeInputProps) {
  const values = value.split(':');
  const segments = seconds ? ['hours', 'minutes', 'seconds'] : ['hours', 'minutes'];
  return (
    <div
      role="group"
      aria-label={label}
      className="border-input bg-card focus-within:border-ring focus-within:ring-ring/60 flex h-8 w-fit items-center gap-1 rounded-md border px-1.5 shadow-xs focus-within:ring-2"
      aria-invalid={invalid || undefined}
    >
      {segments.map((segment, index) => {
        const text = values[index] ?? '';
        const number = /^\d+$/.test(text) ? Number(text) : null;
        return (
          <NumberField.Root
            key={segment}
            id={index === 0 ? id : `${id}-${segment}`}
            value={number}
            min={0}
            max={index === 0 ? 23 : 59}
            step={1}
            smallStep={1}
            largeStep={index === 0 ? 1 : 5}
            allowOutOfRange
            locale="en-US"
            format={{ minimumIntegerDigits: 2, maximumFractionDigits: 0, useGrouping: false }}
            onValueChange={(next) => {
              const nextText = next === null ? '' : String(next).padStart(2, '0');
              if (nextText === text) return;
              const updated = segments.map((_, part) => values[part] ?? '');
              updated[index] = nextText;
              onChange(updated.join(':'));
            }}
            className="flex items-center gap-1"
          >
            {index > 0 ? (
              <span aria-hidden="true" className="text-muted-foreground font-mono text-xs">
                :
              </span>
            ) : null}
            <NumberField.Input
              aria-label={`${label} ${segment}`}
              aria-describedby={describedBy}
              aria-invalid={
                invalid || (number !== null && (number < 0 || number > (index === 0 ? 23 : 59))) || undefined
              }
              placeholder={index === 0 ? 'hh' : segment === 'minutes' ? 'mm' : 'ss'}
              className="w-7 bg-transparent p-0 text-center font-mono text-xs tabular-nums outline-none"
              onKeyDown={(event) => {
                if (event.key === 'Enter') event.preventDefault();
              }}
            />
          </NumberField.Root>
        );
      })}
    </div>
  );
}
