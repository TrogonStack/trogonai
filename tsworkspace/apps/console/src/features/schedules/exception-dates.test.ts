import { describe, expect, it } from 'vitest';

import { compareExceptionRows, exactInstant, importExceptionDates, reconcileExceptionRows } from './exception-dates';

describe('exact exception date operations', () => {
  it('recognizes equivalent offsets without collapsing nanosecond-separated instants', () => {
    expect(exactInstant('2027-01-01T09:00:00.000000001Z')?.key).toBe(
      exactInstant('2027-01-01T10:00:00.000000001+01:00')?.key,
    );
    expect(exactInstant('2027-01-01T09:00:00.000000001Z')?.key).not.toBe(
      exactInstant('2027-01-01T09:00:00.000000002Z')?.key,
    );
  });
  it.each(['not a date', '2027-02-30T09:00:00Z', '2027-01-01T09:00:00.1234567890Z', '2027-01-01T09:00:00'])(
    'rejects invalid instant %s',
    (value) => expect(exactInstant(value)).toBeUndefined(),
  );
  it('sorts by full precision, preserves original spelling and stable IDs on removal', () => {
    const rows = reconcileExceptionRows('2027-01-01T09:00:00.000000002Z\n2027-01-01T10:00:00.000000001+01:00\ninvalid');
    expect([...rows].sort(compareExceptionRows).map((row) => row.raw)).toEqual([rows[1]!.raw, rows[0]!.raw, 'invalid']);
    expect(reconcileExceptionRows(`${rows[1]!.raw}\ninvalid`, rows).map((row) => row.id)).toEqual([
      rows[1]!.id,
      rows[2]!.id,
    ]);
  });
  it('validates an entire import atomically and skips exact duplicates', () => {
    const rows = reconcileExceptionRows('2027-01-01T09:00:00.000000001Z');
    expect(importExceptionDates('2027-01-02T09:00:00Z\ninvalid', rows)).toEqual({ ok: false, invalidLines: [2] });
    expect(
      importExceptionDates(
        '2027-01-01T10:00:00.000000001+01:00\n2027-01-01T09:00:00.000000002Z\n2027-01-01T09:00:00.000000002Z',
        rows,
      ),
    ).toEqual({ ok: true, added: ['2027-01-01T09:00:00.000000002Z'], duplicates: 2 });
  });
});
