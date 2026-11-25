import { fromJson, toJson } from '@bufbuild/protobuf';
import { TimestampSchema } from '@bufbuild/protobuf/wkt';
import { z } from 'zod';

export interface ExceptionDateRow {
  readonly id: string;
  readonly raw: string;
}
export interface ExactInstant {
  readonly key: string;
  readonly seconds: bigint;
  readonly nanos: number;
  readonly iso: string;
}
export function exactInstant(raw: string): ExactInstant | undefined {
  const text = raw.trim();
  if (!z.iso.datetime({ offset: true }).safeParse(text).success || /\.\d{10,}/.test(text)) return undefined;
  try {
    const timestamp = fromJson(TimestampSchema, text);
    return {
      key: `${timestamp.seconds}:${timestamp.nanos}`,
      seconds: timestamp.seconds,
      nanos: timestamp.nanos,
      iso: String(toJson(TimestampSchema, timestamp)),
    };
  } catch {
    return undefined;
  }
}
export function exceptionLines(value: string): string[] {
  return value.split('\n').filter((line) => line.trim() !== '');
}
export function reconcileExceptionRows(value: string, previous: readonly ExceptionDateRow[] = []): ExceptionDateRow[] {
  const available = new Map<string, ExceptionDateRow[]>();
  for (const row of previous) {
    const group = available.get(row.raw) ?? [];
    group.push(row);
    available.set(row.raw, group);
  }
  return exceptionLines(value).map((raw) => available.get(raw)?.shift() ?? { id: crypto.randomUUID(), raw });
}
export function compareExceptionRows(left: ExceptionDateRow, right: ExceptionDateRow): number {
  const a = exactInstant(left.raw);
  const b = exactInstant(right.raw);
  if (!a || !b) return a ? -1 : b ? 1 : 0;
  return a.seconds < b.seconds ? -1 : a.seconds > b.seconds ? 1 : a.nanos - b.nanos;
}
export type ExceptionImport =
  | { readonly ok: true; readonly added: readonly string[]; readonly duplicates: number }
  | { readonly ok: false; readonly invalidLines: readonly number[] };
export function importExceptionDates(batch: string, existing: readonly ExceptionDateRow[]): ExceptionImport {
  const lines = batch
    .split('\n')
    .map((raw, index) => ({ raw: raw.trim(), line: index + 1 }))
    .filter(({ raw }) => raw !== '');
  const invalidLines = lines.filter(({ raw }) => !exactInstant(raw)).map(({ line }) => line);
  if (invalidLines.length) return { ok: false, invalidLines };
  const seen = new Set(existing.map((row) => exactInstant(row.raw)?.key).filter((key) => key !== undefined));
  const added: string[] = [];
  let duplicates = 0;
  for (const { raw } of lines) {
    const key = exactInstant(raw)!.key;
    if (seen.has(key)) duplicates++;
    else {
      seen.add(key);
      added.push(raw);
    }
  }
  return { ok: true, added, duplicates };
}
export function friendlyInstant(instant: ExactInstant): { date: string; time: string } {
  const date = new Date(instant.iso);
  return {
    date: new Intl.DateTimeFormat('en', { timeZone: 'UTC', year: 'numeric', month: 'short', day: 'numeric' }).format(
      date,
    ),
    time:
      new Intl.DateTimeFormat('en', {
        timeZone: 'UTC',
        hour: '2-digit',
        minute: '2-digit',
        second: '2-digit',
        hourCycle: 'h23',
      }).format(date) + ' UTC',
  };
}
