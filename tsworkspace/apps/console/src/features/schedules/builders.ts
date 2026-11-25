import { z } from 'zod';

export const weekdays = [
  { value: 'MO', label: 'Monday', short: 'Mon', cron: 'MON' },
  { value: 'TU', label: 'Tuesday', short: 'Tue', cron: 'TUE' },
  { value: 'WE', label: 'Wednesday', short: 'Wed', cron: 'WED' },
  { value: 'TH', label: 'Thursday', short: 'Thu', cron: 'THU' },
  { value: 'FR', label: 'Friday', short: 'Fri', cron: 'FRI' },
  { value: 'SA', label: 'Saturday', short: 'Sat', cron: 'SAT' },
  { value: 'SU', label: 'Sunday', short: 'Sun', cron: 'SUN' },
] as const;
export type Weekday = (typeof weekdays)[number]['value'];
export type CronPattern = 'minutes' | 'hourly' | 'daily' | 'weekdays' | 'weekly' | 'monthly';
export interface CronBuilder {
  pattern: CronPattern;
  every: string;
  minute: string;
  time: string;
  weekday: string;
  day: string;
}
export const defaultCron = (): CronBuilder => ({
  pattern: 'weekdays',
  every: '5',
  minute: '0',
  time: '09:00',
  weekday: 'MON',
  day: '1',
});
const integer = (value: string, min: number, max: number) =>
  /^\d+$/.test(value) && Number(value) >= min && Number(value) <= max;
export function cronExpression(draft: CronBuilder): string {
  const [hour = '', minute = ''] = draft.time.split(':').map((part) => (part === '' ? '' : String(Number(part))));
  switch (draft.pattern) {
    case 'minutes':
      return `0 */${draft.every} * * * *`;
    case 'hourly':
      return `0 ${draft.minute} * * * *`;
    case 'daily':
      return `0 ${minute} ${hour} * * *`;
    case 'weekdays':
      return `0 ${minute} ${hour} * * MON-FRI`;
    case 'weekly':
      return `0 ${minute} ${hour} * * ${draft.weekday}`;
    case 'monthly':
      return `0 ${minute} ${hour} ${draft.day} * *`;
  }
}
export function parseCron(expression: string): CronBuilder | undefined {
  const parts = expression.trim().split(/\s+/);
  if (parts.length !== 6 || parts[0] !== '0') return undefined;
  const [, minute, hour, day, month, weekday] = parts as [string, string, string, string, string, string];
  if (month !== '*') return undefined;
  const result = defaultCron();
  if (hour === '*' && day === '*' && weekday === '*') {
    if (minute.startsWith('*/') && integer(minute.slice(2), 1, 59))
      return { ...result, pattern: 'minutes', every: minute.slice(2) };
    if (integer(minute, 0, 59)) return { ...result, pattern: 'hourly', minute };
    return undefined;
  }
  if (!integer(hour, 0, 23) || !integer(minute, 0, 59)) return undefined;
  result.time = `${hour.padStart(2, '0')}:${minute.padStart(2, '0')}`;
  if (day === '*' && weekday === '*') return { ...result, pattern: 'daily' };
  if (day === '*' && weekday === 'MON-FRI') return { ...result, pattern: 'weekdays' };
  if (day === '*' && weekdays.some((entry) => entry.cron === weekday)) return { ...result, pattern: 'weekly', weekday };
  if (integer(day, 1, 31) && weekday === '*') return { ...result, pattern: 'monthly', day };
  return undefined;
}
export function cronSummary(draft: CronBuilder): string {
  switch (draft.pattern) {
    case 'minutes':
      return `At minute steps of ${draft.every || '…'}, restarting each hour`;
    case 'hourly':
      return `Every hour at minute ${draft.minute || '…'}`;
    case 'daily':
      return `Every day at ${draft.time || '…'}`;
    case 'weekdays':
      return `Monday through Friday at ${draft.time || '…'}`;
    case 'weekly':
      return `Every ${weekdays.find((entry) => entry.cron === draft.weekday)?.label} at ${draft.time || '…'}`;
    case 'monthly':
      return `Day ${draft.day || '…'} of every month at ${draft.time || '…'}`;
  }
}
export type RecurrenceFrequency = 'DAILY' | 'WEEKLY' | 'MONTHLY' | 'YEARLY';
export interface RecurrenceBuilder {
  frequency: RecurrenceFrequency;
  interval: string;
  days: Weekday[];
  monthMode: 'start' | 'day' | 'ordinal';
  day: string;
  ordinal: string;
  weekday: Weekday;
  month: string;
  end: 'never' | 'count' | 'until';
  count: string;
  until: string;
}
export const defaultRecurrence = (): RecurrenceBuilder => ({
  frequency: 'WEEKLY',
  interval: '1',
  days: ['MO', 'WE', 'FR'],
  monthMode: 'start',
  day: '1',
  ordinal: '1',
  weekday: 'MO',
  month: '1',
  end: 'never',
  count: '10',
  until: '',
});
export function untilFromInput(value: string): string {
  return value.replace(/[-:]/g, '').replace(/\.\d+$/, '') + (value.length === 16 ? '00Z' : 'Z');
}
export function untilToInput(value: string): string | undefined {
  const match = /^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/.exec(value);
  return match ? `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}` : undefined;
}
export function recurrenceExpression(draft: RecurrenceBuilder): string {
  const parts = [`FREQ=${draft.frequency}`];
  if (draft.interval !== '1') parts.push(`INTERVAL=${draft.interval}`);
  if (draft.frequency === 'WEEKLY' && draft.days.length) parts.push(`BYDAY=${draft.days.join(',')}`);
  if (draft.frequency === 'MONTHLY' || draft.frequency === 'YEARLY') {
    if (draft.frequency === 'YEARLY' && draft.monthMode !== 'start') parts.push(`BYMONTH=${draft.month}`);
    if (draft.monthMode === 'day') parts.push(`BYMONTHDAY=${draft.day}`);
    if (draft.monthMode === 'ordinal') parts.push(`BYDAY=${draft.ordinal}${draft.weekday}`);
  }
  if (draft.end === 'count') parts.push(`COUNT=${draft.count}`);
  if (draft.end === 'until') parts.push(`UNTIL=${draft.until ? untilFromInput(draft.until) : ''}`);
  return parts.join(';');
}
export function parseRecurrence(rule: string): RecurrenceBuilder | undefined {
  const fields = rule.split(';').map((part) => part.split('='));
  if (fields.some((field) => field.length !== 2) || new Set(fields.map(([key]) => key)).size !== fields.length)
    return undefined;
  const values = Object.fromEntries(fields) as Record<string, string>;
  if (
    Object.keys(values).some(
      (key) => !['FREQ', 'INTERVAL', 'BYDAY', 'BYMONTHDAY', 'BYMONTH', 'COUNT', 'UNTIL'].includes(key),
    )
  )
    return undefined;
  if (!['DAILY', 'WEEKLY', 'MONTHLY', 'YEARLY'].includes(values.FREQ ?? '')) return undefined;
  const result = { ...defaultRecurrence(), frequency: values.FREQ as RecurrenceFrequency, days: [] as Weekday[] };
  if (values.INTERVAL !== undefined) {
    if (!integer(values.INTERVAL, 1, 2147483647)) return undefined;
    result.interval = values.INTERVAL;
  }
  if (values.COUNT && values.UNTIL) return undefined;
  if (values.COUNT !== undefined) {
    if (!integer(values.COUNT, 1, 2147483647)) return undefined;
    result.end = 'count';
    result.count = values.COUNT;
  }
  if (values.UNTIL !== undefined) {
    const until = untilToInput(values.UNTIL);
    if (!until || !z.iso.datetime().safeParse(`${until}Z`).success) return undefined;
    result.end = 'until';
    result.until = until;
  }
  if (result.frequency === 'WEEKLY') {
    if (values.BYMONTHDAY || values.BYMONTH) return undefined;
    if (values.BYDAY !== undefined) {
      const days = values.BYDAY.split(',');
      if (!days.every((day) => weekdays.some((entry) => entry.value === day)) || new Set(days).size !== days.length)
        return undefined;
      result.days = days as Weekday[];
    }
  } else if (result.frequency === 'MONTHLY' || result.frequency === 'YEARLY') {
    if (values.BYMONTH !== undefined) {
      if (result.frequency !== 'YEARLY' || !integer(values.BYMONTH, 1, 12)) return undefined;
      result.month = values.BYMONTH;
    }
    if (values.BYMONTHDAY !== undefined && values.BYDAY !== undefined) return undefined;
    if (values.BYMONTHDAY !== undefined) {
      if (!integer(values.BYMONTHDAY, 1, 31) && values.BYMONTHDAY !== '-1') return undefined;
      result.monthMode = 'day';
      result.day = values.BYMONTHDAY;
    }
    if (values.BYDAY !== undefined) {
      const match = /^([1-4]|-1)(MO|TU|WE|TH|FR|SA|SU)$/.exec(values.BYDAY);
      if (!match) return undefined;
      result.monthMode = 'ordinal';
      result.ordinal = match[1]!;
      result.weekday = match[2] as Weekday;
    }
    if (result.frequency === 'YEARLY' && (result.monthMode !== 'start') !== (values.BYMONTH !== undefined))
      return undefined;
  } else if (values.BYDAY !== undefined || values.BYMONTHDAY !== undefined || values.BYMONTH !== undefined)
    return undefined;
  return result;
}
const ordinals: Record<string, string> = { '1': 'first', '2': 'second', '3': 'third', '4': 'fourth', '-1': 'last' };
export function recurrenceSummary(draft: RecurrenceBuilder): string {
  const unit = { DAILY: 'day', WEEKLY: 'week', MONTHLY: 'month', YEARLY: 'year' }[draft.frequency];
  let summary = draft.interval === '1' ? `Every ${unit}` : `Every ${draft.interval || '…'} ${unit}s`;
  if (draft.frequency === 'WEEKLY')
    summary += draft.days.length
      ? ` on ${draft.days.map((day) => weekdays.find((entry) => entry.value === day)?.short).join(', ')}`
      : ' on the start date’s weekday';
  if (draft.frequency === 'MONTHLY' || draft.frequency === 'YEARLY') {
    if (draft.monthMode === 'start') summary += ' on the start date’s calendar day';
    if (draft.monthMode === 'day') summary += draft.day === '-1' ? ', on the last day' : `, on day ${draft.day || '…'}`;
    if (draft.monthMode === 'ordinal')
      summary += `, on the ${ordinals[draft.ordinal]} ${weekdays.find((entry) => entry.value === draft.weekday)?.label}`;
    if (draft.frequency === 'YEARLY' && draft.monthMode !== 'start')
      summary += ` of ${new Intl.DateTimeFormat('en', { month: 'long', timeZone: 'UTC' }).format(new Date(Date.UTC(2027, Number(draft.month) - 1, 1)))}`;
  }
  if (draft.end === 'count') summary += `, for ${draft.count || '…'} occurrences`;
  if (draft.end === 'until') summary += `, through ${draft.until.replace('T', ' ') || '…'} UTC (inclusive)`;
  return summary;
}
export const intervalUnits = [
  { value: '1', label: 'Seconds' },
  { value: '60', label: 'Minutes' },
  { value: '3600', label: 'Hours' },
  { value: '86400', label: 'Days' },
] as const;
export function secondsToUnit(seconds: string, multiplier: string): string {
  const match = /^(\d+)(?:\.(\d{1,9}))?$/.exec(seconds);
  if (!match) return '';
  const nanos = BigInt(match[1]!) * 1_000_000_000n + BigInt((match[2] ?? '').padEnd(9, '0'));
  const divisor = BigInt(multiplier);
  if (nanos % divisor !== 0n) return '';
  const units = nanos / divisor;
  return `${units / 1_000_000_000n}${
    units % 1_000_000_000n
      ? `.${String(units % 1_000_000_000n)
          .padStart(9, '0')
          .replace(/0+$/, '')}`
      : ''
  }`;
}
export function unitToSeconds(value: string, multiplier: string): string {
  const match = /^(\d+)(?:\.(\d{1,9}))?$/.exec(value);
  if (!match) return value;
  const nanos = (BigInt(match[1]!) * 1_000_000_000n + BigInt((match[2] ?? '').padEnd(9, '0'))) * BigInt(multiplier);
  return `${nanos / 1_000_000_000n}${
    nanos % 1_000_000_000n
      ? `.${String(nanos % 1_000_000_000n)
          .padStart(9, '0')
          .replace(/0+$/, '')}`
      : ''
  }`;
}

export function basicCronValid(expression: string): boolean {
  const fields = expression.trim().split(/\s+/);
  if (![6, 7].includes(fields.length)) return false;
  const bounds = [
    [0, 59],
    [0, 59],
    [0, 23],
    [1, 31],
    [1, 12],
    [0, 7],
  ] as const;
  return fields.every((field, index) => {
    if (index === 6) return /^[\d*,/-]+$/.test(field);
    const [min, max] = bounds[index]!;
    const names =
      index === 4
        ? ['JAN', 'FEB', 'MAR', 'APR', 'MAY', 'JUN', 'JUL', 'AUG', 'SEP', 'OCT', 'NOV', 'DEC']
        : index === 5
          ? weekdays.map((entry) => entry.cron)
          : [];
    const atom = (value: string) => integer(value, min, max) || names.includes(value as never);
    return field.split(',').every((part) => {
      const tokens = part.split('/');
      if (tokens.length > 2) return false;
      const [range, step] = tokens;
      if (step !== undefined && !integer(step, 1, max)) return false;
      if (range === '*' || range === '?') return true;
      const endpoints = range!.split('-');
      return endpoints.length >= 1 && endpoints.length <= 2 && endpoints.every(atom);
    });
  });
}
export function basicRecurrenceIssue(rule: string): string | undefined {
  const parts = rule.split(';').map((part) => part.split('='));
  if (
    parts.some((part) => part.length !== 2 || !part[0] || !part[1]) ||
    new Set(parts.map(([key]) => key)).size !== parts.length
  )
    return 'Use unique KEY=value recurrence clauses.';
  const values = Object.fromEntries(parts) as Record<string, string>;
  if (values.COUNT !== undefined && values.UNTIL !== undefined) return 'Choose COUNT or UNTIL, never both.';
  for (const key of ['INTERVAL', 'COUNT'])
    if (values[key] !== undefined && !integer(values[key], 1, 2147483647))
      return `${key} must be a positive whole number up to 2147483647.`;
  if (values.UNTIL !== undefined) {
    const until = untilToInput(values.UNTIL);
    if (!until || !z.iso.datetime().safeParse(`${until}Z`).success)
      return 'Use an inclusive UTC UNTIL in YYYYMMDDTHHMMSSZ format.';
  }
  return undefined;
}
