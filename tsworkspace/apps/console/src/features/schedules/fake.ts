import { clone, create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  CreateScheduleSchema,
  ScheduleProjectionSchema,
  type CreateSchedule,
  type ScheduleProjection,
} from '@trogonai/proto';

import type { SchedulesClient } from './client';
import { emptyScheduleDraft, type ScheduleDraft } from './draft';
import { commandFromDraft, projectionFromCommand } from './model';

export class ScheduleFault extends Error {
  constructor(
    readonly code: 'NOT_FOUND' | 'ALREADY_EXISTS' | 'INVALID_COMMAND',
    message: string,
  ) {
    super(message);
    this.name = 'ScheduleFault';
  }
}

export class InMemorySchedulesClient implements SchedulesClient {
  private readonly entries = new Map<string, ScheduleProjection>();
  private readonly latencyMs: number;
  constructor(options: { latencyMs?: number; seeded?: boolean; clock?: () => Date } = {}) {
    this.latencyMs = options.latencyMs ?? 0;
    if (options.seeded ?? true) {
      const now = (options.clock ?? (() => new Date()))();
      const ahead = (minutes: number) => new Date(now.getTime() + minutes * 60_000).toISOString();
      const presets: Partial<ScheduleDraft>[] = [
        { kind: 'every', everySeconds: '300', subject: 'ops.metrics.refresh', ttlSeconds: '60' },
        {
          kind: 'cron',
          cron: '0 0 9 * * MON-FRI',
          timezone: 'America/New_York',
          subject: 'reports.daily',
          sourceSubject: 'reports.latest',
        },
        { kind: 'at', at: ahead(90), subject: 'ops.maintenance.start', status: 'paused' },
        {
          kind: 'rrule',
          dtstart: ahead(60),
          rrule: 'FREQ=WEEKLY;BYDAY=MO,WE,FR;COUNT=12',
          subject: 'reports.weekly',
          rdate: ahead(120),
          exdate: ahead(180),
          headers: [{ name: 'X-Report', value: 'weekly' }],
        },
        { kind: 'rrule', dtstart: ahead(-1440), rrule: 'FREQ=DAILY;COUNT=1', subject: 'ops.archive.complete' },
      ];
      presets.forEach((preset, index) => {
        const command = commandFromDraft({
          ...emptyScheduleDraft(),
          ...preset,
          scheduleId: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
        });
        const row = projectionFromCommand(command);
        if (index === 4) {
          row.completed = true;
          row.lastOccurrenceAt = timestampFromDate(new Date(ahead(-1440)));
        } else if (row.status?.kind.case === 'scheduled')
          row.nextOccurrenceAt = timestampFromDate(new Date(ahead(index === 0 ? 5 : 60)));
        this.entries.set(row.scheduleId, row);
      });
    }
  }
  private async wait() {
    if (this.latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, this.latencyMs));
  }
  async list(): Promise<readonly ScheduleProjection[]> {
    await this.wait();
    return [...this.entries.values()].map((row) => clone(ScheduleProjectionSchema, row));
  }
  async get(id: string): Promise<ScheduleProjection | undefined> {
    await this.wait();
    const row = this.entries.get(id);
    return row ? clone(ScheduleProjectionSchema, row) : undefined;
  }
  async create(command: CreateSchedule): Promise<ScheduleProjection> {
    await this.wait();
    const safe = clone(CreateScheduleSchema, command);
    if (
      !safe.scheduleId ||
      !safe.schedule?.kind.case ||
      !safe.status?.kind.case ||
      !safe.delivery?.kind.case ||
      !safe.message?.content
    )
      throw new ScheduleFault('INVALID_COMMAND', 'Schedule definition, delivery, status and content are required.');
    if (this.entries.has(safe.scheduleId))
      throw new ScheduleFault('ALREADY_EXISTS', 'A schedule already uses this id.');
    const row = projectionFromCommand(safe);
    this.entries.set(row.scheduleId, row);
    return clone(ScheduleProjectionSchema, row);
  }
  private require(id: string): ScheduleProjection {
    const row = this.entries.get(id);
    if (!row) throw new ScheduleFault('NOT_FOUND', 'The schedule no longer exists.');
    return row;
  }
  async pause(id: string): Promise<void> {
    await this.wait();
    const row = this.require(id);
    row.status = create(ScheduleProjectionSchema, { status: { kind: { case: 'paused', value: {} } } }).status;
    row.nextOccurrenceAt = undefined;
  }
  async resume(id: string): Promise<void> {
    await this.wait();
    const row = this.require(id);
    row.status = create(ScheduleProjectionSchema, { status: { kind: { case: 'scheduled', value: {} } } }).status;
  }
  async remove(id: string): Promise<void> {
    await this.wait();
    this.require(id);
    this.entries.delete(id);
  }
}
