import { CalendarDaysIcon } from 'lucide-react';
import { useState } from 'react';
import { TZDate } from 'react-day-picker';
import { z } from 'zod';

import { Button } from './button';
import { Calendar } from './calendar';
import { Popover, PopoverContent, PopoverTrigger } from './popover';
import { TimeInput, validTime } from './time-input';

export interface DateTimePickerProps {
  id: string;
  label: string;
  /** Calendar fields without an offset: YYYY-MM-DDTHH:mm[:ss]. The caller owns timezone conversion. */
  value: string;
  onChange: (value: string) => void;
  zoneLabel: string;
  invalid?: boolean;
  describedBy?: string;
  autoFocus?: boolean;
}
export function validDateTimeFields(value: string): boolean {
  return (
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(?::\d{2})?$/.test(value) && z.iso.datetime({ local: true }).safeParse(value).success
  );
}
function calendarDate(value: string): Date | undefined {
  if (!validDateTimeFields(value)) return undefined;
  return new TZDate(`${value.slice(0, 10)}T00:00:00Z`, 'UTC');
}
export function DateTimePicker({
  id,
  label,
  value,
  onChange,
  zoneLabel,
  invalid,
  describedBy,
  autoFocus,
}: DateTimePickerProps) {
  const [open, setOpen] = useState(false);
  const [date, setDate] = useState<Date | undefined>();
  const [time, setTime] = useState('00:00:00');
  const initial = validDateTimeFields(value) ? value : '';
  const normalizedInitial = initial.length === 16 ? `${initial}:00` : initial;
  const staged =
    date && validTime(time)
      ? `${String(date.getUTCFullYear()).padStart(4, '0')}-${String(date.getUTCMonth() + 1).padStart(2, '0')}-${String(date.getUTCDate()).padStart(2, '0')}T${time}`
      : '';
  const changeOpen = (next: boolean) => {
    if (next) {
      setDate(calendarDate(value));
      setTime(normalizedInitial ? normalizedInitial.slice(11, 19) : '00:00:00');
    }
    setOpen(next);
  };
  const apply = () => {
    if (!staged || !validDateTimeFields(staged)) return;
    if (staged !== normalizedInitial) onChange(staged);
    setOpen(false);
  };
  const shown = initial
    ? `${initial.slice(0, 10)} · ${initial.slice(11, 19)} ${zoneLabel}`
    : `Choose date and time · ${zoneLabel}`;
  return (
    <Popover open={open} onOpenChange={changeOpen}>
      <PopoverTrigger
        autoFocus={autoFocus}
        id={id}
        aria-label={label}
        aria-invalid={invalid || Boolean(value && !initial) || undefined}
        aria-describedby={describedBy}
        render={
          <Button type="button" variant="outline" className="w-full justify-between gap-2 text-left font-normal" />
        }
      >
        <span className="min-w-0 truncate font-mono text-xs">{shown}</span>
        <CalendarDaysIcon className="size-3.5 shrink-0" aria-hidden="true" />
      </PopoverTrigger>
      <PopoverContent aria-label={`${label} calendar`} className="w-fit max-w-[calc(100vw-2rem)]" initialFocus={false}>
        <Calendar
          mode="single"
          selected={date}
          defaultMonth={date}
          onSelect={(selected) => setDate(selected)}
          autoFocus
          aria-label={`${label} dates`}
        />
        <div className="border-border flex flex-col gap-3 border-t p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-muted-foreground text-xs">Time · {zoneLabel}</span>
            <TimeInput
              id={`${id}-time`}
              label={`${label} time`}
              value={time}
              onChange={setTime}
              seconds
              invalid={!validTime(time)}
            />
          </div>
          <p className="text-muted-foreground text-[11px]">
            Calendar fields are interpreted in {zoneLabel}. Changes apply only when confirmed.
          </p>
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button type="button" size="sm" disabled={!staged || !validDateTimeFields(staged)} onClick={apply}>
              Apply date and time
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
