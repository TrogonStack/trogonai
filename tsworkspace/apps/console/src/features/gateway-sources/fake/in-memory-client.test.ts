import { describe, expect, it } from 'vitest';

import { sourceCommands } from '../domain/commands';
import { SourceFault } from '../domain/faults';
import { toSourceView } from '../domain/view';
import { createFakeSourcesClient } from './index';
import { populateSeed, seedIntegrationIds } from './seed';

const MINUTE = 60_000;
const start = new Date('2026-03-01T12:00:00Z');

function newClient(settleMs = 0) {
  let now = start;
  const client = createFakeSourcesClient({ latencyMs: 0, settleMs, clock: () => now });
  return {
    client,
    advance: (milliseconds: number) => {
      now = new Date(now.getTime() + milliseconds);
    },
  };
}

async function rejection(promise: Promise<unknown>): Promise<SourceFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof SourceFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

describe('seed', () => {
  it('is deterministic for a fixed clock', () => {
    expect(populateSeed(start)).toEqual(populateSeed(start));
  });

  it('covers the requested scenarios', async () => {
    const { client } = newClient();
    const views = new Map((await client.list()).map((state) => [state.id, toSourceView(state)]));

    expect(views.size).toBeGreaterThanOrEqual(8);
    expect(views.get(seedIntegrationIds.github)?.status).toBe('receiving');
    expect(views.get(seedIntegrationIds.slack)?.slots.map((slot) => slot.resolution)).toEqual(['runtime', 'static']);
    expect(views.get(seedIntegrationIds.telegram)?.registration.mismatch).toBe(true);
    expect(views.get(seedIntegrationIds.discord)?.slots[0]?.state).toBe('write_failed');
    expect(views.get(seedIntegrationIds.linear)?.status).toBe('paused');
    expect(views.get(seedIntegrationIds.sentry)).toMatchObject({ status: 'misconfigured', tier: 'customer_managed' });
    expect(views.get(seedIntegrationIds.datadog)).toMatchObject({ status: 'misconfigured', tier: 'customer_managed' });
    expect(views.get(seedIntegrationIds.gitlab)?.tier).toBe('dedicated');
  });
});

describe('pause and resume', () => {
  it('pauses and resumes an integration', async () => {
    const { client } = newClient();
    const github = (await client.get(seedIntegrationIds.github))!;

    const paused = await client.dispatch(sourceCommands.pause(seedIntegrationIds.github), {
      expectedRevision: github.revision,
    });
    expect(toSourceView(paused).status).toBe('paused');

    const resumed = await client.dispatch(sourceCommands.resume(seedIntegrationIds.github), {
      expectedRevision: paused.revision,
    });
    expect(toSourceView(resumed).status).toBe('receiving');
  });

  it('refuses to pause twice and to resume what is running', async () => {
    const { client } = newClient();
    const linear = (await client.get(seedIntegrationIds.linear))!;
    const github = (await client.get(seedIntegrationIds.github))!;

    expect(
      (
        await rejection(
          client.dispatch(sourceCommands.pause(seedIntegrationIds.linear), { expectedRevision: linear.revision }),
        )
      ).reason,
    ).toBe('ALREADY_PAUSED');
    expect(
      (
        await rejection(
          client.dispatch(sourceCommands.resume(seedIntegrationIds.github), { expectedRevision: github.revision }),
        )
      ).reason,
    ).toBe('NOT_PAUSED');
  });

  it('rejects a stale revision', async () => {
    const { client } = newClient();
    const fault = await rejection(
      client.dispatch(sourceCommands.pause(seedIntegrationIds.github), { expectedRevision: 99 }),
    );
    expect(fault.reason).toBe('REVISION_CONFLICT');
  });
});

describe('rotating a credential slot', () => {
  it('goes pending then active at the next version, registering the new version', async () => {
    const { client, advance } = newClient(MINUTE);
    const before = (await client.get(seedIntegrationIds.github))!;

    const pending = toSourceView(
      await client.dispatch(sourceCommands.rotateSlot(seedIntegrationIds.github, 'webhook_secret'), {
        expectedRevision: before.revision,
      }),
    );
    expect(pending.slots[0]).toMatchObject({ state: 'pending_rotation', version: 2 });
    expect(pending.registration.status).toBe('registration_pending');
    expect(pending.settling).toBe(true);

    expect(toSourceView((await client.get(seedIntegrationIds.github))!).slots[0]?.state).toBe('pending_rotation');

    advance(MINUTE);
    const settled = toSourceView((await client.get(seedIntegrationIds.github))!);
    expect(settled.slots[0]).toMatchObject({ state: 'active', version: 3 });
    expect(settled.registration).toMatchObject({ status: 'registered', registeredVersion: 3, mismatch: false });
    expect(settled.settling).toBe(false);
  });

  it('refuses a second change while one is settling', async () => {
    const { client } = newClient(MINUTE);
    const before = (await client.get(seedIntegrationIds.github))!;
    const pending = await client.dispatch(sourceCommands.rotateSlot(seedIntegrationIds.github, 'webhook_secret'), {
      expectedRevision: before.revision,
    });

    const fault = await rejection(
      client.dispatch(sourceCommands.rotateSlot(seedIntegrationIds.github, 'webhook_secret'), {
        expectedRevision: pending.revision,
      }),
    );
    expect(fault.reason).toBe('OPERATION_IN_PROGRESS');
  });

  it('refuses to rotate a static slot', async () => {
    const { client } = newClient();
    const slack = (await client.get(seedIntegrationIds.slack))!;
    const fault = await rejection(
      client.dispatch(sourceCommands.rotateSlot(seedIntegrationIds.slack, 'bot_token'), {
        expectedRevision: slack.revision,
      }),
    );
    expect(fault.reason).toBe('SLOT_NOT_ROTATABLE');
  });

  it('fails closed when the customer disabled the key', async () => {
    const { client } = newClient();
    const sentry = (await client.get(seedIntegrationIds.sentry))!;
    const fault = await rejection(
      client.dispatch(sourceCommands.rotateSlot(seedIntegrationIds.sentry, 'client_secret'), {
        expectedRevision: sentry.revision,
      }),
    );
    expect(fault.reason).toBe('KEY_UNAVAILABLE');
  });
});

describe('re-registering the webhook', () => {
  it('clears a superseded registration', async () => {
    const { client } = newClient();
    const before = (await client.get(seedIntegrationIds.telegram))!;
    expect(toSourceView(before).registration.mismatch).toBe(true);

    const pending = await client.dispatch(sourceCommands.reregisterWebhook(seedIntegrationIds.telegram), {
      expectedRevision: before.revision,
    });
    expect(toSourceView(pending).registration.status).toBe('registration_pending');

    const settled = toSourceView((await client.get(seedIntegrationIds.telegram))!);
    expect(settled.registration).toMatchObject({ status: 'registered', registeredVersion: 3, mismatch: false });
    expect(settled.slots[0]?.state).toBe('active');
    expect(settled.status).toBe('receiving');
  });

  it('refuses a source without a webhook registration', async () => {
    const { client } = newClient();
    const slack = (await client.get(seedIntegrationIds.slack))!;
    const fault = await rejection(
      client.dispatch(sourceCommands.reregisterWebhook(seedIntegrationIds.slack), {
        expectedRevision: slack.revision,
      }),
    );
    expect(fault.reason).toBe('REGISTRATION_NOT_APPLICABLE');
  });
});


it('rejects Datadog deliveries while its custody backend is paused', () => {
  const source = populateSeed(start).find((entry) => entry.id === seedIntegrationIds.datadog)!;
  expect(source.hourly.slice(-12).every((hour) => hour.accepted === 0 && hour.rejected > 0)).toBe(true);
  expect(source.recent.every((delivery) => delivery.outcome.status === 'rejected')).toBe(true);
});
