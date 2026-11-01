import { KeyBackendAdmission, KeyState } from '@trogonai/proto';

import { deriveRefId, seedIds } from '@/features/secrets/fake/seed';

import type { DeliveryRecord, HourlyDeliveries, RejectionReason } from '../domain/delivery';
import { HOURS_IN_WINDOW } from '../domain/delivery';
import type { CredentialSlot, IntegrationState, SlotState, WebhookRegistration } from '../domain/integration';
import { kindOf, mintIntegrationId, subjectPrefixOf } from '../domain/integration-id';
import { secretVersion } from '../domain/secret-version';
import type { SourceCustody } from '../domain/source-custody';
import { kindEvents, type SlotName } from '../domain/source-kind';

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;
const RECENT_DELIVERIES = 8;

function hash(value: string): number {
  let result = 2166136261;
  for (const character of value) result = Math.imul(result ^ character.charCodeAt(0), 16777619);
  return result >>> 0;
}

function noise(seed: number, index: number): number {
  return ((Math.imul(seed ^ (index + 1), 2654435761) >>> 0) % 1000) / 1000;
}

interface Traffic {
  readonly base: number;
  readonly rejectedPerHundred: number;
  readonly silentHours?: number;
  readonly lockedOutHours?: number;
}

function hourlyOf(id: string, traffic: Traffic): readonly HourlyDeliveries[] {
  const seed = hash(id);
  return Array.from({ length: HOURS_IN_WINDOW }, (_, index): HourlyDeliveries => {
    const hoursAgo = HOURS_IN_WINDOW - 1 - index;
    const load = Math.round(traffic.base * (0.4 + noise(seed, index) * 1.2));
    if (traffic.lockedOutHours !== undefined && hoursAgo < traffic.lockedOutHours) {
      return { accepted: 0, rejected: Math.max(1, Math.round(load / 3)) };
    }
    if (traffic.silentHours !== undefined && hoursAgo < traffic.silentHours) return { accepted: 0, rejected: 0 };
    const rejected = Math.round((load * traffic.rejectedPerHundred) / 100 + (noise(seed, index + 31) > 0.85 ? 1 : 0));
    return { accepted: Math.max(0, load - rejected), rejected };
  });
}

interface RecentSpec {
  readonly lastAgoMs: number;
  readonly stepMs: number;
  readonly rejection?: RejectionReason;
  readonly rejectEvery?: number;
}

function recentOf(id: string, now: Date, spec: RecentSpec): readonly DeliveryRecord[] {
  const events = kindEvents[kindOf(mintIntegrationId(id))];
  const seed = hash(id);
  return Array.from({ length: RECENT_DELIVERIES }, (_, index): DeliveryRecord => {
    const rejected =
      spec.rejection !== undefined && (spec.rejectEvery === undefined || (index + 1) % spec.rejectEvery === 0);
    return {
      id: `dlv_${seed.toString(16)}_${index}`,
      receivedAt: new Date(
        now.getTime() - spec.lastAgoMs - index * spec.stepMs - Math.round(noise(seed, index) * 9000),
      ),
      event: events[(seed + index) % events.length]!,
      outcome: rejected && spec.rejection ? { status: 'rejected', reason: spec.rejection } : { status: 'accepted' },
    };
  });
}

function runtime(
  id: string,
  name: SlotName,
  state: SlotState,
  version: number,
  secret = deriveRefId(`gateway:${id}:${name}`),
): CredentialSlot {
  return { name, resolution: { mode: 'runtime', secret, state, version: secretVersion(version) } };
}

function fromEnvironment(name: SlotName, envVar: string): CredentialSlot {
  return { name, resolution: { mode: 'static', envVar } };
}

interface Spec {
  readonly id: string;
  readonly displayName: string;
  readonly paused?: boolean;
  readonly slots: readonly CredentialSlot[];
  readonly registration: WebhookRegistration;
  readonly custody: SourceCustody;
  readonly traffic: Traffic;
  readonly recent: RecentSpec;
  readonly createdDaysAgo: number;
}

function build(now: Date, spec: Spec): IntegrationState {
  const id = mintIntegrationId(spec.id);
  return {
    id,
    revision: 1,
    kind: kindOf(id),
    displayName: spec.displayName,
    paused: spec.paused ?? false,
    subjectPrefix: subjectPrefixOf(id),
    slots: spec.slots,
    registration: spec.registration,
    custody: spec.custody,
    hourly: hourlyOf(spec.id, spec.traffic),
    recent: recentOf(spec.id, now, spec.recent),
    createdAt: new Date(now.getTime() - spec.createdDaysAgo * DAY),
  };
}

export const seedIntegrationIds = {
  github: mintIntegrationId('github/local'),
  slack: mintIntegrationId('slack/acme-workspace'),
  telegram: mintIntegrationId('telegram/support-bot'),
  discord: mintIntegrationId('discord/community'),
  linear: mintIntegrationId('linear/product'),
  sentry: mintIntegrationId('sentry/production'),
  gitlab: mintIntegrationId('gitlab/platform'),
  notion: mintIntegrationId('notion/handbook'),
  datadog: mintIntegrationId('datadog/monitors'),
  incidentio: mintIntegrationId('incidentio/on-call'),
} as const;

export function populateSeed(now: Date = new Date()): readonly IntegrationState[] {
  const registered = (slot: SlotName, version: number, hoursAgo: number): WebhookRegistration => ({
    status: 'registered',
    slot,
    registeredVersion: secretVersion(version),
    registeredAt: new Date(now.getTime() - hoursAgo * HOUR),
  });

  const github = seedIntegrationIds.github;
  const slack = seedIntegrationIds.slack;
  const telegram = seedIntegrationIds.telegram;
  const discord = seedIntegrationIds.discord;
  const linear = seedIntegrationIds.linear;
  const sentry = seedIntegrationIds.sentry;
  const gitlab = seedIntegrationIds.gitlab;
  const datadog = seedIntegrationIds.datadog;
  const incidentio = seedIntegrationIds.incidentio;

  return [
    build(now, {
      id: github,
      displayName: 'GitHub local',
      slots: [runtime(github, 'webhook_secret', 'active', 2, seedIds.credentials.github)],
      registration: registered('webhook_secret', 2, 52),
      custody: { tier: 'managed' },
      traffic: { base: 38, rejectedPerHundred: 1 },
      recent: { lastAgoMs: 40_000, stepMs: 6 * MINUTE },
      createdDaysAgo: 120,
    }),
    build(now, {
      id: slack,
      displayName: 'Slack Acme workspace',
      slots: [
        runtime(slack, 'signing_secret', 'active', 1, seedIds.credentials.slack),
        fromEnvironment('bot_token', 'TROGON_SLACK_ACME_BOT_TOKEN'),
      ],
      registration: { status: 'not_applicable' },
      custody: { tier: 'managed' },
      traffic: { base: 24, rejectedPerHundred: 2 },
      recent: { lastAgoMs: 2 * MINUTE, stepMs: 11 * MINUTE },
      createdDaysAgo: 87,
    }),
    build(now, {
      id: telegram,
      displayName: 'Telegram support bot',
      slots: [runtime(telegram, 'webhook_secret', 'pending_rotation', 3), runtime(telegram, 'bot_token', 'active', 1)],
      registration: registered('webhook_secret', 2, 30),
      custody: { tier: 'managed' },
      traffic: { base: 14, rejectedPerHundred: 9 },
      recent: { lastAgoMs: 5 * MINUTE, stepMs: 17 * MINUTE, rejection: 'invalid_signature', rejectEvery: 3 },
      createdDaysAgo: 41,
    }),
    build(now, {
      id: discord,
      displayName: 'Discord community',
      slots: [
        runtime(discord, 'bot_token', 'write_failed', 1),
        fromEnvironment('public_key', 'TROGON_DISCORD_PUBLIC_KEY'),
      ],
      registration: { status: 'not_applicable' },
      custody: { tier: 'managed' },
      traffic: { base: 9, rejectedPerHundred: 4 },
      recent: { lastAgoMs: 26 * MINUTE, stepMs: 48 * MINUTE },
      createdDaysAgo: 19,
    }),
    build(now, {
      id: linear,
      displayName: 'Linear product',
      paused: true,
      slots: [runtime(linear, 'webhook_secret', 'active', 1, seedIds.credentials.linear)],
      registration: registered('webhook_secret', 1, 24 * 9),
      custody: { tier: 'managed' },
      traffic: { base: 11, rejectedPerHundred: 0, silentHours: 20 },
      recent: { lastAgoMs: 20 * HOUR, stepMs: 25 * MINUTE },
      createdDaysAgo: 64,
    }),
    build(now, {
      id: sentry,
      displayName: 'Sentry production',
      slots: [runtime(sentry, 'client_secret', 'active', 2)],
      registration: { status: 'not_applicable' },
      custody: { tier: 'customer_managed', keyState: KeyState.DISABLED, admission: KeyBackendAdmission.ENABLED },
      traffic: { base: 18, rejectedPerHundred: 1, lockedOutHours: 5 },
      recent: { lastAgoMs: 3 * MINUTE, stepMs: 9 * MINUTE, rejection: 'key_unavailable' },
      createdDaysAgo: 210,
    }),
    build(now, {
      id: gitlab,
      displayName: 'GitLab platform',
      slots: [runtime(gitlab, 'signing_token', 'active', 4)],
      registration: registered('signing_token', 4, 24 * 3),
      custody: { tier: 'dedicated' },
      traffic: { base: 21, rejectedPerHundred: 1 },
      recent: { lastAgoMs: 90_000, stepMs: 8 * MINUTE },
      createdDaysAgo: 150,
    }),
    build(now, {
      id: seedIntegrationIds.notion,
      displayName: 'Notion handbook',
      slots: [fromEnvironment('verification_token', 'TROGON_NOTION_VERIFICATION_TOKEN')],
      registration: { status: 'not_applicable' },
      custody: { tier: 'managed' },
      traffic: { base: 5, rejectedPerHundred: 0 },
      recent: { lastAgoMs: 41 * MINUTE, stepMs: 2 * HOUR },
      createdDaysAgo: 33,
    }),
    build(now, {
      id: datadog,
      displayName: 'Datadog monitors',
      slots: [runtime(datadog, 'webhook_secret', 'active', 1)],
      registration: registered('webhook_secret', 1, 24 * 14),
      custody: { tier: 'customer_managed', keyState: KeyState.ACTIVE, admission: KeyBackendAdmission.PAUSED },
      traffic: { base: 7, rejectedPerHundred: 0, lockedOutHours: 12 },
      recent: { lastAgoMs: 12 * MINUTE, stepMs: 70 * MINUTE, rejection: 'key_unavailable' },
      createdDaysAgo: 72,
    }),
    build(now, {
      id: incidentio,
      displayName: 'incident.io on-call',
      slots: [runtime(incidentio, 'signing_secret', 'active', 1)],
      registration: {
        status: 'registration_failed',
        slot: 'signing_secret',
        reason: 'The provider rejected the endpoint URL with a 403 response.',
        failedAt: new Date(now.getTime() - 6 * HOUR),
      },
      custody: { tier: 'managed' },
      traffic: { base: 3, rejectedPerHundred: 0 },
      recent: { lastAgoMs: 7 * HOUR, stepMs: 3 * HOUR },
      createdDaysAgo: 8,
    }),
  ];
}
