import { create } from '@bufbuild/protobuf';
import {
  AuthorizationScheme,
  BindingReplacementMode,
  CredentialBindingLifecycle,
  ProviderAccessScope,
  ProviderConnectionLifecycle,
  ProviderProtocol,
  ProviderRoutingEligibility,
  ProviderValidationRefSchema,
} from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { commands } from '../domain/commands';
import {
  assembleReplacement,
  buildBinding,
  buildRegistration,
  buildReplacement,
  type RegistrationDraft,
} from '../domain/draft';
import { ConcurrencyFault, ProviderConnectionFault } from '../domain/faults';
import { createFakeProviderConnectionsClient } from './index';

const validation = create(ProviderValidationRefSchema, { validationId: 'validation-1' });

function draft(connectionId: string, overrides: Partial<RegistrationDraft> = {}): RegistrationDraft {
  return {
    connectionId,
    parentNodeId: 'node_1',
    providerId: 'cloudflare-ai-gateway',
    endpoint: { kind: 'network', baseUrl: 'https://gateway.example.com/v1' },
    accessScope: ProviderAccessScope.GATEWAY_ONLY,
    models: [{ identifier: 'openai/gpt-4o-mini' }],
    protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
    binding: {
      bindingId: `${connectionId}-key-1`,
      authentication: {
        kind: 'gateway',
        mode: { kind: 'passthrough' },
        authorization: AuthorizationScheme.BEARER_TOKEN,
      },
    },
    profile: { displayName: connectionId, description: '' },
    routingPolicy: { preference: 0, eligibility: ProviderRoutingEligibility.ENABLED },
    ...overrides,
  };
}

function newClient() {
  return createFakeProviderConnectionsClient({ seeded: false });
}

async function registered(client: ReturnType<typeof newClient>, id = 'conn-1') {
  return client.dispatch(commands.register(await buildRegistration(draft(id))));
}

describe('in-memory provider connections client', () => {
  it('seeds realistic connections covering every lifecycle', async () => {
    const client = createFakeProviderConnectionsClient();
    const records = await client.list();
    const lifecycles = new Set(records.map((record) => record.connection.lifecycle));
    expect(lifecycles).toEqual(
      new Set([
        ProviderConnectionLifecycle.PENDING,
        ProviderConnectionLifecycle.ACTIVE,
        ProviderConnectionLifecycle.REVOKED,
      ]),
    );
    expect((await client.history('openai-prod')).length).toBeGreaterThan(4);
  });

  it('registers and records the event with revision, actor and time', async () => {
    const client = newClient();
    const record = await registered(client);
    expect(record.revision).toBe(1);
    expect(record.connection.lifecycle).toBe(ProviderConnectionLifecycle.PENDING);
    const history = await client.history('conn-1');
    expect(history).toHaveLength(1);
    expect(history[0]?.event.event.case).toBe('registered');
    expect(history[0]?.actor).toBeTruthy();
  });

  it('rejects registering the same connection twice', async () => {
    const client = newClient();
    await registered(client);
    await expect(client.dispatch(commands.register(await buildRegistration(draft('conn-1'))))).rejects.toMatchObject({
      reason: 'PROVIDER_CONNECTION_ALREADY_EXISTS',
    });
  });

  it('requires and checks the expected revision on existing streams', async () => {
    const client = newClient();
    await registered(client);
    await expect(client.dispatch(commands.revokeConnection('conn-1'))).rejects.toBeInstanceOf(ConcurrencyFault);
    await expect(client.dispatch(commands.revokeConnection('conn-1'), { expectedRevision: 7 })).rejects.toMatchObject({
      reason: 'REVISION_MISMATCH',
    });
    const record = await client.dispatch(commands.revokeConnection('conn-1'), { expectedRevision: 1 });
    expect(record.connection.lifecycle).toBe(ProviderConnectionLifecycle.REVOKED);
  });

  it('treats a repeated command id as already applied', async () => {
    const client = newClient();
    await registered(client);
    const options = { expectedRevision: 1, commandId: 'cmd-1' };
    await client.dispatch(
      commands.setRoutingPolicy('conn-1', { preference: 5, eligibility: ProviderRoutingEligibility.ENABLED }),
      options,
    );
    const again = await client.dispatch(
      commands.setRoutingPolicy('conn-1', { preference: 5, eligibility: ProviderRoutingEligibility.ENABLED }),
      options,
    );
    expect(again.revision).toBe(2);
  });

  it('rejects an unknown connection when validating', async () => {
    const client = newClient();
    const replacement = await buildReplacement({
      connectionId: 'nope',
      previousBindingId: 'a',
      replacement: draft('nope').binding,
      mode: BindingReplacementMode.DRAIN,
      validation,
    });
    await expect(
      client.requestValidation({ connectionId: 'nope', binding: replacement.replacement! }),
    ).rejects.toMatchObject({
      reason: 'PROVIDER_CONNECTION_NOT_FOUND',
    });
  });

  it('fails a FAIL_IF_IN_USE replacement while Sessions pin the binding, then allows it once they end', async () => {
    const client = createFakeProviderConnectionsClient();
    const before = (await client.get('openai-prod'))!;
    const active = before.connection.bindings.find((entry) => entry.lifecycle === CredentialBindingLifecycle.ACTIVE)!;
    const previousBindingId = active.binding!.id!.value;
    const replacement = (draftId: string) => ({
      bindingId: draftId,
      authentication: {
        kind: 'stored-api-key' as const,
        secretRef: `secret://providers/openai-prod/${draftId}`,
        authorization: AuthorizationScheme.BEARER_TOKEN,
      },
    });

    const attempt = async (id: string) => {
      const binding = await buildBinding(replacement(id));
      const receipt = await client.requestValidation({ connectionId: 'openai-prod', binding });
      const built = assembleReplacement({
        connectionId: 'openai-prod',
        previousBindingId,
        replacement: binding,
        mode: BindingReplacementMode.FAIL_IF_IN_USE,
        validation: receipt,
      });
      return client.dispatch(commands.replaceBinding(built), {
        expectedRevision: (await client.get('openai-prod'))!.revision,
      });
    };

    await expect(attempt('openai-prod-key-4')).rejects.toBeInstanceOf(ProviderConnectionFault);
    await expect(attempt('openai-prod-key-4')).rejects.toMatchObject({ reason: 'CREDENTIAL_BINDING_IN_USE' });

    await client.endPinnedSessions('openai-prod', previousBindingId);
    const after = await attempt('openai-prod-key-4');
    const states = new Map(after.connection.bindings.map((entry) => [entry.binding!.id!.value, entry.lifecycle]));
    expect(states.get('openai-prod-key-4')).toBe(CredentialBindingLifecycle.ACTIVE);
    expect(states.get(previousBindingId)).toBe(CredentialBindingLifecycle.REVOKED);
  });

  it('never reuses a binding id', async () => {
    const client = createFakeProviderConnectionsClient();
    const built = await buildReplacement({
      connectionId: 'openai-prod',
      previousBindingId: 'openai-prod-key-3',
      replacement: {
        bindingId: 'openai-prod-key-1',
        authentication: {
          kind: 'stored-api-key',
          secretRef: 'secret://providers/openai-prod/key-1',
          authorization: AuthorizationScheme.BEARER_TOKEN,
        },
      },
      mode: BindingReplacementMode.DRAIN,
      validation,
    });
    await expect(
      client.dispatch(commands.replaceBinding(built), {
        expectedRevision: (await client.get('openai-prod'))!.revision,
      }),
    ).rejects.toMatchObject({ reason: 'CREDENTIAL_BINDING_IDENTITY_REUSED' });
  });

  it('lets the simulated provider reject a credential during validation', async () => {
    const client = createFakeProviderConnectionsClient();
    const built = await buildReplacement({
      connectionId: 'openai-prod',
      previousBindingId: 'openai-prod-key-3',
      replacement: {
        bindingId: 'openai-prod-key-9',
        authentication: {
          kind: 'stored-api-key',
          secretRef: 'secret://providers/openai-prod/reject',
          authorization: AuthorizationScheme.BEARER_TOKEN,
        },
      },
      mode: BindingReplacementMode.DRAIN,
      validation,
    });
    await expect(
      client.requestValidation({ connectionId: 'openai-prod', binding: built.replacement! }),
    ).rejects.toMatchObject({ reason: 'PROVIDER_VALIDATION_REJECTED' });
  });

  it('rejects a registration whose digest does not match its bytes', async () => {
    const client = newClient();
    const registration = await buildRegistration(draft('conn-1'));
    registration.registration!.configurationDigest!.value = new Uint8Array(32);
    await expect(client.dispatch(commands.register(registration))).rejects.toMatchObject({
      reason: 'PROVIDER_COMMAND_INVALID',
    });
  });
});
