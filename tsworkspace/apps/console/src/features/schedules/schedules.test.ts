import { describe, expect, it } from 'vitest';

import { createFakeBackend } from '@/fake-backend';
import { mintProjectId } from '@/features/projects/domain/project-id';

import { emptyScheduleDraft, scheduleDraftSchema } from './draft';
import { InMemorySchedulesClient, ScheduleFault } from './fake';
import { commandFromDraft, durationFromSeconds } from './model';

function validDraft() {
  return { ...emptyScheduleDraft(), subject: 'jobs.refresh' };
}

describe('scheduling boundary', () => {
  it('converts all strategies into generated commands without losing recurrence exceptions', () => {
    const at = '2027-01-15T09:00:00Z';
    const base = validDraft();
    expect(commandFromDraft({ ...base, kind: 'at', at }).schedule?.kind.case).toBe('at');
    expect(commandFromDraft({ ...base, kind: 'every' }).schedule?.kind.case).toBe('every');
    expect(commandFromDraft({ ...base, kind: 'cron' }).schedule?.kind.case).toBe('cron');
    const command = commandFromDraft({
      ...base,
      kind: 'rrule',
      dtstart: at,
      rdate: `${at}\n2027-02-01T12:00:00+01:00`,
      exdate: at,
      sourceSubject: 'jobs.latest',
      ttlSeconds: '0.000000001',
      headers: [
        { name: 'X-Tag', value: 'one' },
        { name: 'X-Tag', value: 'two' },
      ],
    });
    expect(command.schedule?.kind.case).toBe('rrule');
    if (command.schedule?.kind.case === 'rrule') {
      expect(command.schedule.kind.value.rdate).toHaveLength(2);
      expect(command.schedule.kind.value.exdate).toHaveLength(1);
    }
    expect(command.delivery?.kind.value?.ttl).toMatchObject({ seconds: 0n, nanos: 1 });
    expect(command.delivery?.kind.value?.source?.kind.value?.subject).toBe('jobs.latest');
    expect(command.message?.headers).toHaveLength(2);
  });
  it('preserves nanosecond timestamps, durations and arbitrary bytes', () => {
    const precise = commandFromDraft({ ...validDraft(), kind: 'at', at: '2027-01-15T09:00:00.123456789Z' });
    if (precise.schedule?.kind.case === 'at') expect(precise.schedule.kind.value.at?.nanos).toBe(123456789);
    else throw new Error('Expected a one-shot schedule');
    expect(durationFromSeconds('123.456789123')).toEqual({ seconds: 123n, nanos: 456789123 });
    const command = commandFromDraft({ ...validDraft(), payloadEncoding: 'base64', payload: 'AP+A' });
    expect(command.message?.content?.data).toEqual(new Uint8Array([0, 255, 128]));
  });
  it.each([
    { everySeconds: '0' },
    { ttlSeconds: '0' },
    { subject: 'jobs.*' },
    { sourceSubject: 'jobs.>' },
    { subject: 'jobs..refresh' },
    { scheduleId: 'not-a-uuid' },
    { headers: [{ name: 'nAtS-sChEdUlE-Source', value: 'x' }] },
    { headers: [{ name: 'CONTENT-TYPE', value: 'text/plain' }] },
    { headers: [{ name: 'X-Tag', value: 'bad\r\nvalue' }] },
    { payloadEncoding: 'base64', payload: 'not base64' },
    { kind: 'cron', cron: '0 9 * * MON-FRI' },
    { kind: 'rrule', dtstart: '2027-01-01T00:00:00Z', rrule: 'RRULE:FREQ=DAILY' },
    { kind: 'rrule', dtstart: 'not a date' },
    { kind: 'at', at: '2027-02-30T09:00:00Z' },
  ])('rejects malformed edit values %o', (patch) => {
    expect(scheduleDraftSchema.safeParse({ ...validDraft(), ...patch }).success).toBe(false);
  });
  it('accepts six and seven cron fields and ignores hidden timezone for intervals', () => {
    expect(scheduleDraftSchema.safeParse({ ...validDraft(), kind: 'cron', cron: '0 0 9 * * MON-FRI' }).success).toBe(
      true,
    );
    expect(
      scheduleDraftSchema.safeParse({ ...validDraft(), kind: 'cron', cron: '0 0 9 * * MON-FRI 2027' }).success,
    ).toBe(true);
    expect(scheduleDraftSchema.safeParse({ ...validDraft(), timezone: 'not/a/timezone' }).success).toBe(true);
  });
});

describe('in-memory schedules', () => {
  it('creates, pauses, resumes and removes a definition while preserving its contents', async () => {
    const client = new InMemorySchedulesClient({ seeded: false });
    const command = commandFromDraft(validDraft());
    const created = await client.create(command);
    expect(created.completed).toBe(false);
    expect(created.nextOccurrenceAt).toBeUndefined();
    await client.pause(created.scheduleId);
    expect((await client.get(created.scheduleId))?.status?.kind.case).toBe('paused');
    await client.resume(created.scheduleId);
    const resumed = await client.get(created.scheduleId);
    expect(resumed?.status?.kind.case).toBe('scheduled');
    expect(resumed?.message?.content?.data).toEqual(command.message?.content?.data);
    expect(resumed?.nextOccurrenceAt).toBeUndefined();
    await expect(client.create(command)).rejects.toMatchObject({ code: 'ALREADY_EXISTS' });
    await client.remove(created.scheduleId);
    expect(await client.get(created.scheduleId)).toBeUndefined();
    await expect(client.resume(created.scheduleId)).rejects.toBeInstanceOf(ScheduleFault);
  });
  it('keeps completed independent of pause/resume and returns isolated snapshots', async () => {
    const client = new InMemorySchedulesClient();
    const rows = await client.list();
    const completed = rows.find((row) => row.completed)!;
    expect(completed.status?.kind.case).toBe('scheduled');
    await client.pause(completed.scheduleId);
    await client.resume(completed.scheduleId);
    expect((await client.get(completed.scheduleId))?.completed).toBe(true);
    rows[0]!.scheduleId = 'mutated';
    expect((await client.list())[0]!.scheduleId).not.toBe('mutated');
  });
  it('isolates changes by project and starts non-demo projects empty', async () => {
    const backend = createFakeBackend({ latencyMs: 0 });
    const demo = backend.schedules.forProject(mintProjectId('proj-demo'));
    const other = backend.schedules.forProject(mintProjectId('proj-other'));
    expect(await demo.list()).toHaveLength(5);
    expect(await other.list()).toHaveLength(0);
    const created = await other.create(commandFromDraft(validDraft()));
    expect(await demo.get(created.scheduleId)).toBeUndefined();
    expect(await other.list()).toHaveLength(1);
  });
});
