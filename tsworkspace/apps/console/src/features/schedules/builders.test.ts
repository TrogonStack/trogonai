import { describe, expect, it } from 'vitest';

import {
  basicCronValid,
  basicRecurrenceIssue,
  cronExpression,
  defaultCron,
  defaultRecurrence,
  parseCron,
  parseRecurrence,
  recurrenceExpression,
  secondsToUnit,
  unitToSeconds,
} from './builders';
import { emptyScheduleDraft, scheduleDraftSchema } from './draft';

describe('guided scheduling expressions', () => {
  it.each([
    ['0 */5 * * * *', 'minutes'],
    ['0 15 * * * *', 'hourly'],
    ['0 30 9 * * *', 'daily'],
    ['0 0 9 * * MON-FRI', 'weekdays'],
    ['0 45 10 * * FRI', 'weekly'],
    ['0 30 17 31 * *', 'monthly'],
  ])('round trips supported cron %s', (expression, pattern) => {
    const result = parseCron(expression)!;
    expect(result.pattern).toBe(pattern);
    expect(cronExpression(result)).toBe(expression);
  });
  it.each(['17 0 9 * * *', '0 0 9 * * * 2027', '0 0,30 9 * * *', '0 0 9 * JAN MON', '0 0 9 * * MON,WED'])(
    'preserves advanced cron outside guided subset %s',
    (expression) => {
      expect(parseCron(expression)).toBeUndefined();
      expect(basicCronValid(expression)).toBe(true);
    },
  );
  it.each(['0 */0 * * * *', '0 */60 * * * *', '0 */99 * * * *', '0 60 * * * *', '0 0 25 * * *', '0 0 9 0 * *'])(
    'rejects invalid cron %s',
    (expression) => expect(basicCronValid(expression)).toBe(false),
  );
  it('builds monthly positions and mutually exclusive end conditions', () => {
    const draft = {
      ...defaultRecurrence(),
      frequency: 'MONTHLY' as const,
      interval: '2',
      monthMode: 'ordinal' as const,
      ordinal: '-1',
      weekday: 'FR' as const,
      end: 'count' as const,
      count: '8',
    };
    const rule = recurrenceExpression(draft);
    expect(rule).toBe('FREQ=MONTHLY;INTERVAL=2;BYDAY=-1FR;COUNT=8');
    expect(recurrenceExpression(parseRecurrence(rule)!)).toBe(rule);
    expect(recurrenceExpression({ ...draft, end: 'until', until: '2027-12-31T23:59:59' })).toBe(
      'FREQ=MONTHLY;INTERVAL=2;BYDAY=-1FR;UNTIL=20271231T235959Z',
    );
  });
  it.each([
    'FREQ=DAILY',
    'FREQ=WEEKLY;BYDAY=MO,WE,FR',
    'FREQ=MONTHLY;BYMONTHDAY=-1',
    'FREQ=YEARLY;BYMONTH=2;BYMONTHDAY=28',
    'FREQ=YEARLY',
    'FREQ=DAILY;COUNT=10',
    'FREQ=DAILY;UNTIL=20271231T235959Z',
  ])('round trips supported recurrence %s', (rule) => expect(recurrenceExpression(parseRecurrence(rule)!)).toBe(rule));
  it.each([
    'FREQ=WEEKLY;BYDAY=MO;WKST=SU',
    'FREQ=MONTHLY;BYDAY=MO,TU;BYSETPOS=-1',
    'FREQ=YEARLY;BYMONTH=2',
    'FREQ=DAILY;BYHOUR=9',
    'FREQ=DAILY;COUNT=2;UNTIL=20271231T235959Z',
    'FREQ=DAILY;UNTIL=20270230T000000Z',
  ])('keeps unrepresented recurrence advanced %s', (rule) => expect(parseRecurrence(rule)).toBeUndefined());
  it.each([
    'FREQ=DAILY;INTERVAL=0',
    'FREQ=DAILY;COUNT=0',
    'FREQ=DAILY;INTERVAL=',
    'FREQ=DAILY;COUNT=2;UNTIL=20271231T235959Z',
    'FREQ=DAILY;UNTIL=20270230T000000Z',
  ])('blocks malformed recurrence submit %s', (rrule) => {
    expect(basicRecurrenceIssue(rrule)).toBeDefined();
    expect(
      scheduleDraftSchema.safeParse({
        ...emptyScheduleDraft(),
        kind: 'rrule',
        dtstart: '2027-01-01T00:00:00Z',
        subject: 'jobs.test',
        rrule,
      }).success,
    ).toBe(false);
  });
  it('converts interval units without floating point precision loss', () => {
    expect(secondsToUnit('300', '60')).toBe('5');
    expect(unitToSeconds('1.5', '60')).toBe('90');
    expect(unitToSeconds('0.000000001', '3600')).toBe('0.0000036');
    expect(secondsToUnit('0.000000001', '60')).toBe('');
    expect(secondsToUnit('0.000000001', '1')).toBe('0.000000001');
  });
  it('generated presets use exactly six cron fields', () =>
    expect(cronExpression(defaultCron()).split(' ')).toHaveLength(6));
});
