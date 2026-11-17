import { create, fromJson, toJson } from '@bufbuild/protobuf';
import { TimestampSchema, type Duration, type Timestamp } from '@bufbuild/protobuf/wkt';
import {
  CreateScheduleSchema,
  ScheduleDefinitionSchema,
  ScheduleProjectionSchema,
  type CreateSchedule,
  type ScheduleDefinition,
  type ScheduleProjection,
} from '@trogonai/proto';

import { dateLines, scheduleDraftSchema, type ScheduleDraft, type StrategyKind } from './draft';

export function durationFromSeconds(value: string): { seconds: bigint; nanos: number } {
  const [seconds, fraction = ''] = value.split('.');
  return { seconds: BigInt(seconds!), nanos: Number(fraction.padEnd(9, '0')) };
}
export function durationLabel(value: Duration | undefined): string {
  if (!value) return 'Not set';
  return `${value.seconds}${value.nanos ? `.${String(value.nanos).padStart(9, '0').replace(/0+$/, '')}` : ''} seconds`;
}
export function instantLabel(value: Timestamp | undefined): string {
  return value ? String(toJson(TimestampSchema, value)) : 'Not recorded';
}
export function bytesFromDraft(draft: ScheduleDraft): Uint8Array {
  return draft.payloadEncoding === 'base64'
    ? Uint8Array.from(atob(draft.payload), (char) => char.charCodeAt(0))
    : new Uint8Array(new TextEncoder().encode(draft.payload));
}
export function base64Of(bytes: Uint8Array): string {
  return btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(''));
}

export function commandFromDraft(input: ScheduleDraft): CreateSchedule {
  const draft = scheduleDraftSchema.parse(input);
  const timestamp = (value: string) => fromJson(TimestampSchema, value);
  const timezone = { id: draft.timezone, version: draft.timezoneVersion };
  let definition: ScheduleDefinition;
  switch (draft.kind) {
    case 'at':
      definition = create(ScheduleDefinitionSchema, { kind: { case: 'at', value: { at: timestamp(draft.at) } } });
      break;
    case 'every':
      definition = create(ScheduleDefinitionSchema, {
        kind: { case: 'every', value: { every: durationFromSeconds(draft.everySeconds) } },
      });
      break;
    case 'cron':
      definition = create(ScheduleDefinitionSchema, { kind: { case: 'cron', value: { expr: draft.cron, timezone } } });
      break;
    case 'rrule':
      definition = create(ScheduleDefinitionSchema, {
        kind: {
          case: 'rrule',
          value: {
            dtstart: timestamp(draft.dtstart),
            rrule: draft.rrule,
            timezone,
            rdate: dateLines(draft.rdate).map(timestamp),
            exdate: dateLines(draft.exdate).map(timestamp),
          },
        },
      });
      break;
  }
  return create(CreateScheduleSchema, {
    scheduleId: draft.scheduleId,
    status: { kind: draft.status === 'scheduled' ? { case: 'scheduled', value: {} } : { case: 'paused', value: {} } },
    schedule: definition,
    delivery: {
      kind: {
        case: 'natsMessage',
        value: {
          subject: draft.subject,
          ...(draft.ttlSeconds ? { ttl: durationFromSeconds(draft.ttlSeconds) } : {}),
          ...(draft.sourceSubject
            ? { source: { kind: { case: 'latestFromSubject', value: { subject: draft.sourceSubject } } } }
            : {}),
        },
      },
    },
    message: { content: { contentType: draft.contentType, data: bytesFromDraft(draft) }, headers: draft.headers },
  });
}

export function projectionFromCommand(command: CreateSchedule): ScheduleProjection {
  const json = toJson(CreateScheduleSchema, command);
  return fromJson(ScheduleProjectionSchema, json);
}
export function scheduleKind(schedule: ScheduleProjection): StrategyKind {
  return schedule.schedule!.kind.case!;
}
export function definitionSummary(schedule: ScheduleProjection): string {
  const definition = schedule.schedule!.kind;
  switch (definition.case) {
    case 'at':
      return instantLabel(definition.value.at);
    case 'every':
      return `Every ${durationLabel(definition.value.every)}`;
    case 'cron':
      return definition.value.expr;
    case 'rrule':
      return definition.value.rrule;
    default:
      return 'No definition';
  }
}
export function deliverySubject(schedule: ScheduleProjection): string {
  return schedule.delivery?.kind.value?.subject ?? '';
}
export function statusLabel(schedule: ScheduleProjection): string {
  return schedule.status?.kind.case === 'paused' ? 'Paused' : 'Scheduled';
}
