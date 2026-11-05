import { z } from 'zod';

import { basicCronValid, basicRecurrenceIssue } from './builders';

export const strategyOptions = [
  { value: 'at', label: 'Once', description: 'Publish at a specific instant.' },
  { value: 'every', label: 'Interval', description: 'Repeat after a fixed duration.' },
  { value: 'cron', label: 'Cron', description: 'Follow a calendar expression.' },
  { value: 'rrule', label: 'Recurrence rule', description: 'RFC 5545 with included and excluded dates.' },
] as const;

export type StrategyKind = (typeof strategyOptions)[number]['value'];
export const strategyLabels: Record<StrategyKind, string> = {
  at: 'Once',
  every: 'Interval',
  cron: 'Cron',
  rrule: 'Recurrence rule',
};

export interface ScheduleDraft {
  scheduleId: string;
  kind: StrategyKind;
  status: 'scheduled' | 'paused';
  at: string;
  everySeconds: string;
  cron: string;
  timezone: string;
  timezoneVersion: string;
  dtstart: string;
  rrule: string;
  rdate: string;
  exdate: string;
  subject: string;
  ttlSeconds: string;
  sourceSubject: string;
  contentType: string;
  payload: string;
  payloadEncoding: 'utf8' | 'base64';
  headers: { name: string; value: string }[];
}

export const emptyScheduleDraft = (): ScheduleDraft => ({
  scheduleId: crypto.randomUUID(),
  kind: 'every',
  status: 'scheduled',
  at: '',
  everySeconds: '300',
  cron: '0 0 9 * * MON-FRI',
  timezone: 'UTC',
  timezoneVersion: '',
  dtstart: '',
  rrule: 'FREQ=WEEKLY;BYDAY=MO,WE,FR',
  rdate: '',
  exdate: '',
  subject: '',
  ttlSeconds: '',
  sourceSubject: '',
  contentType: 'application/json',
  payload: '{}',
  payloadEncoding: 'utf8',
  headers: [],
});

const positiveSeconds = z
  .string()
  .refine(
    (value) => /^\d+(?:\.\d{1,9})?$/.test(value) && Number(value) > 0 && Number(value) <= 315576000000,
    'Use a positive duration in seconds (up to 9 decimal places).',
  );
function hasControl(value: string): boolean {
  return Array.from(value).some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127);
}
export const concreteSubject = z
  .string()
  .refine(
    (value) => value.length > 0 && !/[\s*>]/.test(value) && !hasControl(value) && value.split('.').every(Boolean),
    'Use a concrete NATS subject without wildcards, empty tokens or whitespace.',
  );
const instant = z
  .string()
  .refine(
    (value) => z.iso.datetime({ offset: true }).safeParse(value).success && Number.isFinite(Date.parse(value)),
    'Use an ISO 8601 instant with Z or a UTC offset.',
  );
const timezone = z.string().refine((value) => {
  if (!value) return true;
  try {
    new Intl.DateTimeFormat('en', { timeZone: value });
    return true;
  } catch {
    return false;
  }
}, 'Use an IANA timezone, such as America/New_York.');
export function dateLines(value: string): string[] {
  return value
    .split(/\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export const scheduleDraftSchema = z
  .object({
    scheduleId: z.uuid('Use a UUID for the schedule id.'),
    kind: z.enum(['at', 'every', 'cron', 'rrule']),
    status: z.enum(['scheduled', 'paused']),
    at: z.string(),
    everySeconds: z.string(),
    cron: z.string(),
    timezone: z.string(),
    timezoneVersion: z.string(),
    dtstart: z.string(),
    rrule: z.string(),
    rdate: z.string(),
    exdate: z.string(),
    subject: concreteSubject,
    ttlSeconds: z.union([z.literal(''), positiveSeconds]),
    sourceSubject: z.union([z.literal(''), concreteSubject]),
    contentType: z
      .string()
      .regex(
        /^[a-zA-Z0-9!#$&^_.+-]+\/[a-zA-Z0-9!#$&^_.+-]+(?:\s*;[^\r\n]+)?$/,
        'Use a media type, such as application/json.',
      )
      .refine((value) => !hasControl(value), 'Media types cannot contain controls.'),
    payload: z.string(),
    payloadEncoding: z.enum(['utf8', 'base64']),
    headers: z.array(
      z.object({
        name: z
          .string()
          .refine(
            (value) => value.length > 0 && !/[\s:]/.test(value) && !hasControl(value),
            'Header names cannot contain whitespace, controls or colons.',
          )
          .refine(
            (value) => !/^nats-schedule-/i.test(value) && value.toLowerCase() !== 'content-type',
            'Scheduler headers and Content-Type are reserved.',
          ),
        value: z
          .string()
          .refine(
            (value) => !value.includes('\r') && !value.includes('\n') && !value.includes(String.fromCharCode(0)),
            'Header values cannot contain CR, LF or NUL.',
          ),
      }),
    ),
  })
  .superRefine((value, ctx) => {
    const validate = (field: keyof ScheduleDraft, schema: z.ZodType, input: unknown) => {
      const result = schema.safeParse(input);
      if (!result.success)
        for (const issue of result.error.issues)
          ctx.addIssue({ code: 'custom', path: [field], message: issue.message });
    };
    if (value.kind === 'at') validate('at', instant, value.at);
    if (value.kind === 'every') validate('everySeconds', positiveSeconds, value.everySeconds);
    if (value.kind === 'cron')
      validate(
        'cron',
        z
          .string()
          .refine(
            basicCronValid,
            'Use six cron fields (seconds first) within their ranges, with an optional seventh year field.',
          ),
        value.cron,
      );
    if (value.kind === 'cron' || value.kind === 'rrule') validate('timezone', timezone, value.timezone);
    if (value.kind === 'rrule') {
      validate('dtstart', instant, value.dtstart);
      validate(
        'rrule',
        z
          .string()
          .refine(
            (rule) =>
              /(?:^|;)FREQ=(?:SECONDLY|MINUTELY|HOURLY|DAILY|WEEKLY|MONTHLY|YEARLY)(?:;|$)/.test(rule) &&
              !/^RRULE:/i.test(rule) &&
              !/\s/.test(rule),
            'Include FREQ and omit the RRULE: prefix.',
          ),
        value.rrule,
      );
      const ruleIssue = basicRecurrenceIssue(value.rrule);
      if (ruleIssue) ctx.addIssue({ code: 'custom', path: ['rrule'], message: ruleIssue });
      for (const field of ['rdate', 'exdate'] as const)
        for (const date of dateLines(value[field])) validate(field, instant, date);
    }
    if (
      value.payloadEncoding === 'base64' &&
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value.payload)
    )
      ctx.addIssue({ code: 'custom', path: ['payload'], message: 'Use padded base64 to represent arbitrary bytes.' });
  });
