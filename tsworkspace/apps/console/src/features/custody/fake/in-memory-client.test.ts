import {
  KeyBackendAdmission,
  KeyBackendFailureReason,
  KeyCapability,
  KeyProvider,
  KeyState,
  KeyUseDenialReason,
  ProviderRequestAbsence,
} from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { pauseCopy } from '../domain/admission';
import { KeyFault } from '../domain/faults';
import { migrationId, parsePurposeId, purposeId } from '../domain/identifiers';
import type { BindingInput, RegistrationInput } from '../domain/registration';
import { providerRequestAbsenceLabels, providerRequestIdNote } from '../domain/usage';
import { toCustodyView } from '../domain/view';
import { createFakeTenantCustodyClient } from './index';
import { populateCustodySeed } from './seed';
import { seedBackendIds, seedKeyRefs } from './seed-ids';

const MINUTE = 60_000;
const start = new Date('2026-03-01T12:00:00Z');

const messages = purposeId('session_messages');
const artifacts = purposeId('session_artifacts');
const interactions = purposeId('workflow_run_interactions');

function newClient() {
  let now = start;
  const client = createFakeTenantCustodyClient({ latencyMs: 0, clock: () => now });
  return {
    client,
    advance: async (milliseconds: number) => {
      await client.load();
      now = new Date(now.getTime() + milliseconds);
      await client.load();
    },
    view: async () => toCustodyView(await client.load(), now),
  };
}

async function rejection(promise: Promise<unknown>): Promise<KeyFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof KeyFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

const awsInput: RegistrationInput = {
  provider: KeyProvider.AWS_KMS,
  displayName: 'Acme AWS staging',
  region: 'eu-west-1',
  roleArn: 'arn:aws:iam::381204965517:role/trogon-key-access',
};

const openBaoInput: RegistrationInput = {
  provider: KeyProvider.OPENBAO_TRANSIT,
  displayName: 'Acme edge OpenBao',
  endpoint: 'https://bao-edge.acme.example:8200',
  namespace: '',
  transitMount: 'transit-edge',
  authMethod: 'jwt',
  authRole: 'trogon-key-access',
  trustRootPem: '-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUQ0\n-----END CERTIFICATE-----',
  version: '2.6.1',
};

const newAwsKey = 'arn:aws:kms:eu-west-1:512049177301:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee';

type Client = ReturnType<typeof newClient>['client'];

async function bindAndValidate(client: Client, input: BindingInput) {
  const plan = await client.prepareBinding(input);
  for (const check of plan.checks) await client.runCheck(plan.attemptId, check.id);
  return client.completeBinding(plan.attemptId);
}

describe('seed', () => {
  it('is deterministic for a fixed clock', () => {
    expect(populateCustodySeed(start)).toEqual(populateCustodySeed(start));
  });

  it('holds at least two platform-owned registrations internally', async () => {
    const { client } = newClient();
    expect(await client.platformBackendCount()).toBeGreaterThanOrEqual(2);
  });

  it('covers every tier and every key state a tenant can see', async () => {
    const { view } = newClient();
    const data = await view();

    expect(data.purposes.map((entry) => entry.tier).sort()).toEqual([
      'customer_managed',
      'customer_managed',
      'dedicated',
    ]);
    expect(new Set(data.keys.map((key) => key.state))).toEqual(
      new Set([KeyState.ACTIVE, KeyState.DECRYPT_ONLY, KeyState.DISABLED]),
    );
    expect(data.migrations.map((entry) => entry.migration.status).sort()).toEqual([
      'completed',
      'completed',
      'running',
    ]);
  });
});

describe('the tenant boundary', () => {
  it('never exposes a platform registration, its identity or its operations', async () => {
    const { client } = newClient();
    const state = await client.load();
    const json = JSON.stringify(state, (_key, value) => (typeof value === 'bigint' ? value.toString() : value));

    expect(state.backends.every((record) => record.registration.configuration?.owner?.owner.case === 'tenant')).toBe(
      true,
    );
    expect(json).not.toContain('platform_primary');
    expect(json).not.toContain('platform_secondary');
    expect(state.operations.every((record) => record.keyRef !== undefined)).toBe(true);
  });

  it('gives a dedicated key no provider, backend or locator', async () => {
    const { view } = newClient();
    const dedicated = (await view()).keys.find((key) => key.keyRef === seedKeyRefs.dedicatedArtifacts);

    expect(dedicated?.tier).toBe('dedicated');
    expect(dedicated?.customer).toBeUndefined();
    expect(dedicated?.records).toEqual([]);
  });

  it('reads back the locator only for customer managed bindings', async () => {
    const { view } = newClient();
    const keys = (await view()).keys;

    for (const key of keys) {
      if (key.tier === 'customer_managed') expect(key.customer?.locator).toBeTruthy();
      else expect(key.customer).toBeUndefined();
    }
  });

  it('keeps managed custody free of any key identity', async () => {
    const { client, view } = newClient();
    await client.startMigration({ purpose: artifacts, target: { tier: 'managed' } });
    const entry = (await view()).purposes.find((candidate) => candidate.purpose.id === artifacts);

    expect(entry?.tier).toBe('managed');
    expect(entry?.key).toBeUndefined();
  });
});

describe('purposes', () => {
  it('come from a catalogue with an owning service and no credential or secret purposes', async () => {
    const { view } = newClient();
    const catalogue = (await view()).catalogue;

    expect(catalogue.length).toBeGreaterThan(0);
    for (const purpose of catalogue) {
      expect(purpose.owningService).not.toBe('');
      expect(purpose.id).not.toMatch(/credential|secret/);
    }
  });

  it('reject identifiers that are not purposes', () => {
    expect(parsePurposeId('Not A Purpose')).toBeUndefined();
  });
});

describe('registering a backend', () => {
  it('generates an external ID for AWS, then registers once every check passes', async () => {
    const { client, view } = newClient();
    const plan = await client.prepareRegistration(awsInput);

    expect(plan.externalId).toMatch(/^trogon-ext-/);
    expect(plan.instructions.length).toBeGreaterThan(0);

    for (const check of plan.checks) expect((await client.runCheck(plan.attemptId, check.id)).outcome).toBe('pass');
    const record = await client.completeRegistration(plan.attemptId);

    expect(record.displayName).toBe('Acme AWS staging');
    const created = (await view()).backends.find((backend) => backend.id === record.registration.keyBackendId?.value);
    expect(created?.paused).toBe(false);
    expect(created?.connection.kind).toBe('connected');
  });

  it('refuses to complete before every check ran', async () => {
    const { client } = newClient();
    const plan = await client.prepareRegistration(awsInput);
    await client.runCheck(plan.attemptId, plan.checks[0]!.id);

    expect((await rejection(client.completeRegistration(plan.attemptId))).reason).toBe('VALIDATION_INCOMPLETE');
  });

  it('refuses a second registration for the same boundary', async () => {
    const { client } = newClient();
    const plan = await client.prepareRegistration(awsInput);
    for (const check of plan.checks) await client.runCheck(plan.attemptId, check.id);
    await client.completeRegistration(plan.attemptId);

    expect((await rejection(client.prepareRegistration(awsInput))).reason).toBe('BACKEND_ALREADY_REGISTERED');
  });

  it('fails the OpenBao capability check once, with a typed reason', async () => {
    const { client } = newClient();
    const plan = await client.prepareRegistration(openBaoInput);
    const results = [];
    for (const check of plan.checks) results.push(await client.runCheck(plan.attemptId, check.id));
    const failed = results.find((result) => result.outcome === 'fail');

    expect(failed?.checkId).toBe('disable_upsert');
    expect(failed?.reason).toBe(KeyBackendFailureReason.CAPABILITY_MISSING);
    expect((await client.runCheck(plan.attemptId, 'disable_upsert')).outcome).toBe('pass');
  });

  it('rejects invalid input', async () => {
    const { client } = newClient();
    expect((await rejection(client.prepareRegistration({ ...awsInput, roleArn: 'nope' }))).reason).toBe(
      'COMMAND_INVALID',
    );
  });
});

describe('the three independent dimensions', () => {
  it('keeps health out of admission', async () => {
    const { view } = newClient();
    const openbao = (await view()).backends.find((backend) => backend.id === seedBackendIds.openbao);

    expect(openbao?.connection.kind).toBe('failed');
    expect(openbao?.paused).toBe(false);
    expect(openbao?.pause.available).toBe(true);
  });

  it('keeps absent expiry signals absent and warns only about the ones present', async () => {
    const { view } = newClient();
    const backends = (await view()).backends;
    const openbao = backends.find((backend) => backend.id === seedBackendIds.openbao);
    const aws = backends.find((backend) => backend.id === seedBackendIds.aws);

    expect(aws?.expiry).toEqual([]);
    const signals = openbao?.expiry.map((warning) => warning.signal) ?? [];
    expect(signals).toContain('server_certificate');
    expect(signals).not.toContain('client_certificate');
  });
});

describe('pausing a backend', () => {
  it('uses the exact owner facing copy', () => {
    expect(pauseCopy).toBe(
      'Pausing stops the platform from calling your provider. To revoke access, disable the key or revoke access in your provider.',
    );
  });

  it('pauses and resumes without touching key state', async () => {
    const { client, view } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    let backend = (await view()).backends.find((candidate) => candidate.id === seedBackendIds.aws);
    expect(backend?.admission).toBe(KeyBackendAdmission.PAUSED);
    expect(backend?.keys.find((key) => key.keyRef === seedKeyRefs.awsMessages)?.state).toBe(KeyState.ACTIVE);

    await client.resumeBackend(seedBackendIds.aws);
    backend = (await view()).backends.find((candidate) => candidate.id === seedBackendIds.aws);
    expect(backend?.admission).toBe(KeyBackendAdmission.ENABLED);
  });

  it('rejects pausing twice and resuming what is not paused', async () => {
    const { client } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    expect((await rejection(client.pauseBackend(seedBackendIds.aws))).reason).toBe('BACKEND_ALREADY_PAUSED');
    expect((await rejection(client.resumeBackend(seedBackendIds.gcp))).reason).toBe('BACKEND_NOT_PAUSED');
  });

  it('denies a bind while paused with BackendPaused', async () => {
    const { client } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    const fault = await rejection(
      client.prepareBinding({
        backendId: seedBackendIds.aws,
        locator: newAwsKey,
        purpose: messages,
        capabilities: [KeyCapability.ENVELOPE_WRAPPING],
      }),
    );

    expect(fault.reason).toBe('KEY_USE_DENIED');
    expect(fault.denial).toBe(KeyUseDenialReason.BACKEND_PAUSED);
  });

  it('denies set default while paused', async () => {
    const { client } = newClient();
    await client.pauseBackend(seedBackendIds.openbao);
    const fault = await rejection(client.setDefaultKey(seedKeyRefs.openbaoArtifacts));

    expect(fault.denial).toBe(KeyUseDenialReason.BACKEND_PAUSED);
  });

  it('denies a migration whose source is paused', async () => {
    const { client, advance } = newClient();
    await advance(60 * MINUTE);
    await client.pauseBackend(seedBackendIds.aws);
    const fault = await rejection(client.startMigration({ purpose: messages, target: { tier: 'managed' } }));

    expect(fault.denial).toBe(KeyUseDenialReason.BACKEND_PAUSED);
  });

  it('denies a migration whose target is paused', async () => {
    const { client } = newClient();
    await client.pauseBackend(seedBackendIds.openbao);
    const fault = await rejection(
      client.startMigration({
        purpose: artifacts,
        target: { tier: 'customer_managed', keyRef: seedKeyRefs.openbaoArtifacts },
      }),
    );

    expect(fault.denial).toBe(KeyUseDenialReason.BACKEND_PAUSED);
  });

  it('makes no probe while paused', async () => {
    const { client } = newClient();
    await client.pauseBackend(seedBackendIds.aws);

    expect((await rejection(client.validateBackend(seedBackendIds.aws))).denial).toBe(
      KeyUseDenialReason.BACKEND_PAUSED,
    );
  });

  it('revalidates on resume and stays paused when validation fails', async () => {
    const { client, view } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    await client.simulateProviderOutage(seedBackendIds.aws, KeyBackendFailureReason.ACCESS_DENIED);

    expect((await rejection(client.resumeBackend(seedBackendIds.aws))).reason).toBe('RESUME_VALIDATION_FAILED');
    const backend = (await view()).backends.find((candidate) => candidate.id === seedBackendIds.aws);
    expect(backend?.paused).toBe(true);
    expect(backend?.connection.kind).toBe('failed');

    await client.simulateProviderOutage(seedBackendIds.aws, undefined);
    await client.resumeBackend(seedBackendIds.aws);
    expect((await view()).backends.find((candidate) => candidate.id === seedBackendIds.aws)?.paused).toBe(false);
  });

  it('does not advance a running migration while an end is paused', async () => {
    const { client, advance, view } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    await advance(10 * MINUTE);
    const running = (await view()).migrations.find((entry) => entry.migration.status === 'running');

    expect(running?.migration.rewrapped).toBe(29);
    expect(running?.blocked).toBeDefined();
  });
});

describe('key states', () => {
  it('has exactly the five states', () => {
    expect(
      [KeyState.ACTIVE, KeyState.DECRYPT_ONLY, KeyState.DISABLED, KeyState.PENDING_DESTRUCTION, KeyState.DESTROYED]
        .length,
    ).toBe(5);
    expect(Object.values(KeyState).filter((value) => typeof value === 'number')).toHaveLength(6);
  });

  it('denies wrap on a decrypt only key but not unwrap', async () => {
    const { view } = newClient();
    const key = (await view()).keys.find((candidate) => candidate.keyRef === seedKeyRefs.awsLegacyMessages);

    expect(key?.wrapDenial).toBe(KeyUseDenialReason.KEY_STATE);
    expect(key?.unwrapDenial).toBeUndefined();
  });

  it('denies both operations once a key is pending destruction or destroyed', async () => {
    const { client, view } = newClient();
    for (const state of [KeyState.PENDING_DESTRUCTION, KeyState.DESTROYED]) {
      await client.simulateKeyState(seedKeyRefs.awsMessages, state);
      const key = (await view()).keys.find((candidate) => candidate.keyRef === seedKeyRefs.awsMessages);
      expect(key?.wrapDenial).toBe(KeyUseDenialReason.KEY_STATE);
      expect(key?.unwrapDenial).toBe(KeyUseDenialReason.KEY_STATE);
    }
  });

  it('puts the pause denial ahead of the key state denial', async () => {
    const { client, view } = newClient();
    await client.pauseBackend(seedBackendIds.aws);
    const key = (await view()).keys.find((candidate) => candidate.keyRef === seedKeyRefs.awsLegacyMessages);

    expect(key?.wrapDenial).toBe(KeyUseDenialReason.BACKEND_PAUSED);
  });
});

describe('usage records', () => {
  it('list provider attempts with the identifier the provider returned or an absence reason', async () => {
    const { view } = newClient();
    const records = (await view()).records;
    const attempts = records.flatMap((record) => record.attempts);

    expect(attempts.some((attempt) => attempt.providerRequest.kind === 'id')).toBe(true);
    const reasons = attempts.flatMap((attempt) =>
      attempt.providerRequest.kind === 'absent' ? [attempt.providerRequest.reason] : [],
    );
    expect(reasons.length).toBeGreaterThan(0);
    expect(providerRequestAbsenceLabels[ProviderRequestAbsence.NOT_PROVIDED_BY_PROVIDER]).toBe(
      'Not provided by provider',
    );
    expect(providerRequestAbsenceLabels[ProviderRequestAbsence.NO_RESPONSE]).toBe('No response');
  });

  it('never claims a join with the provider audit log', () => {
    expect(providerRequestIdNote).toBe('The identifier the provider returned for this attempt.');
    expect(providerRequestIdNote).not.toMatch(/CloudTrail|audit log/i);
  });

  it('records denials without any provider attempt', async () => {
    const { view } = newClient();
    const denied = (await view()).records.filter(
      (record) => record.outcome.kind === 'failed' && record.attempts.length === 0,
    );

    expect(denied.length).toBeGreaterThan(0);
  });

  it('are limited to customer managed keys', async () => {
    const { view } = newClient();
    const data = await view();
    const dedicated = new Set(data.keys.filter((key) => key.tier === 'dedicated').map((key) => key.keyRef));

    expect(data.records.every((record) => !dedicated.has(record.keyRef))).toBe(true);
  });
});

describe('binding a key', () => {
  it('binds a new customer managed key and exposes its locator to the owner', async () => {
    const { client, view } = newClient();
    const bound = await bindAndValidate(client, {
      backendId: seedBackendIds.aws,
      locator: newAwsKey,
      purpose: messages,
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
    });
    const key = (await view()).keys.find((candidate) => candidate.keyRef === bound.keyRef);

    expect(key?.customer?.locator).toBe(newAwsKey);
    expect(key?.isDefault).toBe(false);
  });

  it('refuses a key that is already bound', async () => {
    const { client } = newClient();
    const fault = await rejection(
      client.prepareBinding({
        backendId: seedBackendIds.aws,
        locator: 'arn:aws:kms:eu-west-1:512049177301:key/9b1d6c0e-52f4-4a7e-b3a1-7c2f0d8e41aa',
        purpose: messages,
        capabilities: [KeyCapability.ENVELOPE_WRAPPING],
      }),
    );

    expect(fault.reason).toBe('KEY_ALREADY_BOUND');
  });
});

describe('migrations', () => {
  it('moves from managed or dedicated into customer managed custody', async () => {
    const { client, view } = newClient();
    const result = await client.startMigration({
      purpose: artifacts,
      target: { tier: 'customer_managed', keyRef: seedKeyRefs.openbaoArtifacts },
    });

    expect(result.migration?.status).toBe('running');
    const entry = (await view()).migrations.find((candidate) => candidate.migration.id === result.migration?.id);
    expect(entry?.direction).toBe('into_customer_managed');
  });

  it('moves out of customer managed custody into dedicated without exposing a locator', async () => {
    const { client, advance, view } = newClient();
    await advance(60 * MINUTE);
    const result = await client.startMigration({ purpose: messages, target: { tier: 'dedicated' } });
    const data = await view();
    const entry = data.migrations.find((candidate) => candidate.migration.id === result.migration?.id);

    expect(entry?.direction).toBe('out_of_customer_managed');
    expect(entry?.to.tier).toBe('dedicated');
    expect(entry?.to.key?.customer).toBeUndefined();
  });

  it('moves out of customer managed custody back to managed', async () => {
    const { client, advance, view } = newClient();
    await advance(60 * MINUTE);
    const result = await client.startMigration({ purpose: messages, target: { tier: 'managed' } });
    const entry = (await view()).migrations.find((candidate) => candidate.migration.id === result.migration?.id);

    expect(result.custody.custody.case).toBe('managed');
    expect(entry?.direction).toBe('out_of_customer_managed');
    expect(entry?.to.key).toBeUndefined();
  });

  it('moves between platform tiers', async () => {
    const { client, view } = newClient();
    const result = await client.startMigration({ purpose: artifacts, target: { tier: 'managed' } });
    const entry = (await view()).migrations.find((candidate) => candidate.migration.id === result.migration?.id);

    expect(entry?.direction).toBe('within_platform');
  });

  it('cannot leave a custody whose key is disabled', async () => {
    const { client } = newClient();
    const fault = await rejection(client.startMigration({ purpose: interactions, target: { tier: 'managed' } }));

    expect(fault.denial).toBe(KeyUseDenialReason.KEY_STATE);
  });

  it('rejects the custody the purpose already has', async () => {
    const { client } = newClient();

    expect((await rejection(client.startMigration({ purpose: artifacts, target: { tier: 'dedicated' } }))).reason).toBe(
      'SAME_CUSTODY',
    );
  });

  it('rejects a second migration while one is in flight', async () => {
    const { client } = newClient();
    const fault = await rejection(client.startMigration({ purpose: messages, target: { tier: 'managed' } }));

    expect(fault.reason).toBe('MIGRATION_IN_FLIGHT');
  });

  it('rejects a key bound for another purpose', async () => {
    const { client } = newClient();
    const fault = await rejection(
      client.startMigration({
        purpose: artifacts,
        target: { tier: 'customer_managed', keyRef: seedKeyRefs.gcpInteractions },
      }),
    );

    expect(fault.reason).toBe('PURPOSE_MISMATCH');
  });

  it('settles over time and moves the payloads to the target key', async () => {
    const { client, advance, view } = newClient();
    await advance(30 * MINUTE);
    const data = await view();
    const done = data.migrations.find((entry) => entry.purpose === messages && entry.migration.status === 'completed');

    expect(done).toBeDefined();
    expect(data.purposes.find((entry) => entry.purpose.id === messages)?.key?.keyRef).toBe(seedKeyRefs.awsMessages);
    expect(client).toBeDefined();
  });

  it('pauses and resumes a running migration', async () => {
    const { client, view } = newClient();
    const running = (await view()).migrations.find((entry) => entry.migration.status === 'running')!;
    const paused = await client.pauseMigration(running.migration.id);
    expect(paused.status).toBe('paused');
    expect((await client.resumeMigration(running.migration.id)).status).toBe('running');
    expect((await rejection(client.pauseMigration(migrationId('mig_00000000')))).reason).toBe('MIGRATION_NOT_FOUND');
  });
});


describe('review regressions', () => {
  it('blocks default switches during a migration', async () => {
    const { client } = newClient();
    const bound = await bindAndValidate(client, {
      backendId: seedBackendIds.aws, locator: newAwsKey, purpose: messages,
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
    });
    expect((await rejection(client.setDefaultKey(bound.keyRef))).reason).toBe('MIGRATION_IN_FLIGHT');
  });

  it('allows the same relative path on a different OpenBao backend', async () => {
    const { client } = newClient();
    const plan = await client.prepareRegistration({ ...openBaoInput, transitMount: 'transit' });
    for (const check of plan.checks) {
      if ((await client.runCheck(plan.attemptId, check.id)).outcome !== 'pass') await client.runCheck(plan.attemptId, check.id);
    }
    const backend = await client.completeRegistration(plan.attemptId);
    const state = await client.load();
    const original = state.keys.find((key) => key.keyRef === seedKeyRefs.openbaoArtifacts)!;
    if (original.binding.kind !== 'customer_managed') throw new Error('Expected customer key');
    const { locatorText } = await import('../domain/locator');
    await expect(client.prepareBinding({
      backendId: backend.registration.keyBackendId!.value,
      locator: locatorText(original.binding.detail.locator), purpose: artifacts,
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
    })).resolves.toBeDefined();
  });
});
