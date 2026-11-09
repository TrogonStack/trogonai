import { create } from '@bufbuild/protobuf';
import {
  AuthorizationScheme,
  BindingReplacementMode,
  CredentialBindingLifecycle,
  OpenAiOutputLimitField,
  ProviderAccessScope,
  ProviderConnectionEventSchema,
  ProviderConnectionLifecycle,
  ProviderProtocol,
  ProviderRoutingEligibility,
  ProviderValidationRefSchema,
  type ProviderConnection,
  type ProviderConnectionEvent,
  type State,
} from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { commands } from './commands';
import {
  decide,
  evolve,
  fold,
  initialState,
  presentConnection,
  ReplayFault,
  unrestrictedContext,
  type DecisionContext,
  type ProviderCommand,
} from './decider';
import { buildRegistration, buildReplacement, type BindingDraft, type RegistrationDraft } from './draft';
import { ProviderConnectionFault, type ProviderFaultReason } from './faults';

const DIGEST = 'ab'.repeat(32);
const validation = create(ProviderValidationRefSchema, { validationId: 'validation-1' });

function storedKey(bindingId: string): BindingDraft {
  return {
    bindingId,
    authentication: {
      kind: 'stored-api-key',
      secretRef: `secret://providers/test/${bindingId}`,
      authorization: AuthorizationScheme.BEARER_TOKEN,
    },
  };
}

function managedDraft(overrides: Partial<RegistrationDraft> = {}): RegistrationDraft {
  return {
    connectionId: 'conn-1',
    parentNodeId: 'node_1',
    providerId: 'openai',
    endpoint: { kind: 'network', baseUrl: 'https://api.openai.com/v1' },
    accessScope: ProviderAccessScope.MANAGED,
    models: [{ identifier: 'gpt-4.1', catalogPin: { modelId: 'gpt-4.1', version: '1', definitionDigestHex: DIGEST } }],
    protocols: [ProviderProtocol.OPENAI_CHAT_COMPLETIONS],
    binding: storedKey('binding-1'),
    profile: { displayName: 'Test', description: '' },
    routingPolicy: { preference: 0, eligibility: ProviderRoutingEligibility.ENABLED },
    ...overrides,
  };
}

function apply(state: State, command: ProviderCommand, context?: DecisionContext): State {
  const decision = decide(state, command, context);
  if (!decision.ok) throw decision.fault;
  return decision.events.reduce(evolve, state);
}

function expectFault(state: State, command: ProviderCommand, reason: ProviderFaultReason, context?: DecisionContext) {
  const decision = decide(state, command, context);
  expect(decision.ok).toBe(false);
  if (!decision.ok) expect(decision.fault.reason).toBe(reason);
}

async function registered(draft: RegistrationDraft = managedDraft()): Promise<State> {
  return apply(initialState(), commands.register(await buildRegistration(draft)));
}

async function active(draft?: RegistrationDraft): Promise<State> {
  return apply(await registered(draft), commands.activate('conn-1', validation));
}

function connectionOf(state: State): ProviderConnection {
  return presentConnection(state)!;
}

function lifecycles(state: State): Record<string, CredentialBindingLifecycle> {
  return Object.fromEntries(connectionOf(state).bindings.map((entry) => [entry.binding!.id!.value, entry.lifecycle]));
}

async function replacement(bindingId: string, mode: BindingReplacementMode, previous = 'binding-1') {
  return commands.replaceBinding(
    await buildReplacement({
      connectionId: 'conn-1',
      previousBindingId: previous,
      replacement: storedKey(bindingId),
      mode,
      validation,
    }),
  );
}

describe('register', () => {
  it('creates a pending connection with a pending binding', async () => {
    const connection = connectionOf(await registered());

    expect(connection.lifecycle).toBe(ProviderConnectionLifecycle.PENDING);
    expect(connection.bindings).toHaveLength(1);
    expect(connection.bindings[0]!.lifecycle).toBe(CredentialBindingLifecycle.PENDING);
    expect(connection.configurationDigest!.value).toHaveLength(32);
    expect(connection.configurationBytes.length).toBeGreaterThan(0);
  });

  it('rejects a second registration of the same stream', async () => {
    expectFault(
      await registered(),
      commands.register(await buildRegistration(managedDraft())),
      'PROVIDER_CONNECTION_ALREADY_EXISTS',
    );
  });

  it('rejects gateway authentication on a managed connection', async () => {
    const draft = managedDraft({
      binding: {
        bindingId: 'binding-1',
        authentication: {
          kind: 'gateway',
          authorization: AuthorizationScheme.BEARER_TOKEN,
          mode: { kind: 'passthrough' },
        },
      },
    });

    expectFault(
      initialState(),
      commands.register(await buildRegistration(draft)),
      'PROVIDER_AUTHENTICATION_UNSUPPORTED',
    );
  });

  it('accepts no credential only on a deployment-local endpoint', async () => {
    const noCredential: BindingDraft = { bindingId: 'binding-1', authentication: { kind: 'deployment-no-credential' } };

    expectFault(
      initialState(),
      commands.register(await buildRegistration(managedDraft({ binding: noCredential }))),
      'PROVIDER_AUTHENTICATION_UNSUPPORTED',
    );
    const local = await registered(
      managedDraft({ binding: noCredential, endpoint: { kind: 'deployment-local', endpointId: 'ollama' } }),
    );
    expect(connectionOf(local).lifecycle).toBe(ProviderConnectionLifecycle.PENDING);
  });

  it('requires gateway authentication on a gateway-only connection', async () => {
    const draft = managedDraft({
      accessScope: ProviderAccessScope.GATEWAY_ONLY,
      models: [{ identifier: 'any-model' }],
    });

    expectFault(
      initialState(),
      commands.register(await buildRegistration(draft)),
      'PROVIDER_AUTHENTICATION_UNSUPPORTED',
    );
  });

  it('requires authorization for gateway key modes', async () => {
    const draft = managedDraft({
      accessScope: ProviderAccessScope.GATEWAY_ONLY,
      models: [{ identifier: 'any-model' }],
      binding: {
        bindingId: 'binding-1',
        authentication: { kind: 'gateway', mode: { kind: 'override-key', secretRef: 'secret://x' } },
      },
    });

    const decision = decide(initialState(), commands.register(await buildRegistration(draft)));
    expect(decision.ok).toBe(false);
    if (!decision.ok) {
      expect(decision.fault.reason).toBe('PROVIDER_COMMAND_INVALID');
      expect(decision.fault.violations.map((violation) => violation.constraint)).toContain(
        'gateway_auth.key_scheme_required',
      );
    }
  });

  it('accepts gateway passthrough and no-authentication without authorization', async () => {
    for (const mode of [{ kind: 'passthrough' }, { kind: 'no-authentication' }] as const) {
      const draft = managedDraft({
        accessScope: ProviderAccessScope.GATEWAY_ONLY,
        models: [{ identifier: 'any-model' }],
        binding: { bindingId: 'binding-1', authentication: { kind: 'gateway', mode } },
      });
      expect(decide(initialState(), commands.register(await buildRegistration(draft))).ok).toBe(true);
    }
  });

  it.each([
    [
      'managed offerings without catalog pins',
      { models: [{ identifier: 'gpt-4.1' }] },
      'connection.managed_catalog_pins',
    ],
    [
      'duplicate model identifiers',
      {
        accessScope: ProviderAccessScope.GATEWAY_ONLY,
        models: [{ identifier: 'a' }, { identifier: 'a' }],
        binding: { bindingId: 'binding-1', authentication: { kind: 'gateway', mode: { kind: 'passthrough' } } },
      },
      'connection.models_unique',
    ],
    ['no protocols', { protocols: [] }, 'repeated.min_items'],
    [
      'duplicate protocols',
      { protocols: [ProviderProtocol.OPENAI_RESPONSES, ProviderProtocol.OPENAI_RESPONSES] },
      'repeated.unique',
    ],
    [
      'an output limit without an OpenAI protocol',
      {
        protocols: [ProviderProtocol.ANTHROPIC_MESSAGES],
        openAiOutputLimit: OpenAiOutputLimitField.MAX_TOKENS,
      },
      'connection.output_limit_protocol',
    ],
    ['a non-https base URL', { endpoint: { kind: 'network', baseUrl: 'http://api.openai.com' } }, 'string.pattern'],
    [
      'a base URL with credentials',
      { endpoint: { kind: 'network', baseUrl: 'https://user@api.openai.com' } },
      'string.pattern',
    ],
    [
      'a base URL with a query',
      { endpoint: { kind: 'network', baseUrl: 'https://api.openai.com/v1?x=1' } },
      'string.pattern',
    ],
  ] as const)('rejects %s', async (_name, overrides, constraint) => {
    const decision = decide(
      initialState(),
      commands.register(await buildRegistration(managedDraft(overrides as Partial<RegistrationDraft>))),
    );

    expect(decision.ok).toBe(false);
    if (!decision.ok) {
      expect(decision.fault.reason).toBe('PROVIDER_COMMAND_INVALID');
      expect(decision.fault.violations.map((violation) => violation.constraint)).toContain(constraint);
    }
  });
});

describe('activate', () => {
  it('makes the connection and its initial binding active', async () => {
    const state = await active();

    expect(connectionOf(state).lifecycle).toBe(ProviderConnectionLifecycle.ACTIVE);
    expect(lifecycles(state)).toEqual({ 'binding-1': CredentialBindingLifecycle.ACTIVE });
  });

  it('rejects an unregistered, active, or revoked connection', async () => {
    expectFault(initialState(), commands.activate('conn-1', validation), 'PROVIDER_CONNECTION_NOT_FOUND');
    expectFault(await active(), commands.activate('conn-1', validation), 'PROVIDER_CONNECTION_NOT_PENDING');
    const revoked = apply(await registered(), commands.revokeConnection('conn-1'));
    expectFault(revoked, commands.activate('conn-1', validation), 'PROVIDER_CONNECTION_REVOKED');
  });
});

describe('replace binding', () => {
  it('drains the previous binding and activates the replacement', async () => {
    const state = apply(await active(), await replacement('binding-2', BindingReplacementMode.DRAIN));

    expect(lifecycles(state)).toEqual({
      'binding-1': CredentialBindingLifecycle.DRAINING,
      'binding-2': CredentialBindingLifecycle.ACTIVE,
    });
  });

  it('revokes the previous binding when failing if in use with no pinned sessions', async () => {
    const state = apply(await active(), await replacement('binding-2', BindingReplacementMode.FAIL_IF_IN_USE));

    expect(lifecycles(state)).toEqual({
      'binding-1': CredentialBindingLifecycle.REVOKED,
      'binding-2': CredentialBindingLifecycle.ACTIVE,
    });
  });

  it('rejects fail-if-in-use while sessions pin the previous binding', async () => {
    const context: DecisionContext = { ...unrestrictedContext, pinnedSessions: () => 2 };

    expectFault(
      await active(),
      await replacement('binding-2', BindingReplacementMode.FAIL_IF_IN_USE),
      'CREDENTIAL_BINDING_IN_USE',
      context,
    );
  });

  it('rejects draining when policy does not permit overlap', async () => {
    const context: DecisionContext = { ...unrestrictedContext, permitsDrainOverlap: () => false };

    expectFault(
      await active(),
      await replacement('binding-2', BindingReplacementMode.DRAIN),
      'CREDENTIAL_DRAIN_NOT_PERMITTED',
      context,
    );
  });

  it('requires an active connection', async () => {
    expectFault(
      await registered(),
      await replacement('binding-2', BindingReplacementMode.DRAIN),
      'PROVIDER_CONNECTION_NOT_ACTIVE',
    );
  });

  it('never reuses a binding identity, including revoked history', async () => {
    const state = apply(await active(), await replacement('binding-2', BindingReplacementMode.FAIL_IF_IN_USE));

    expectFault(
      state,
      await replacement('binding-1', BindingReplacementMode.DRAIN, 'binding-2'),
      'CREDENTIAL_BINDING_IDENTITY_REUSED',
    );
  });

  it('rejects replacing a binding with itself as a contract violation', async () => {
    expectFault(
      await active(),
      await replacement('binding-1', BindingReplacementMode.DRAIN),
      'PROVIDER_COMMAND_INVALID',
    );
  });

  it('requires the exact active previous binding', async () => {
    const drained = apply(await active(), await replacement('binding-2', BindingReplacementMode.DRAIN));

    expectFault(
      drained,
      await replacement('binding-3', BindingReplacementMode.DRAIN, 'binding-1'),
      'CREDENTIAL_BINDING_NOT_ACTIVE',
    );
    expectFault(
      drained,
      await replacement('binding-3', BindingReplacementMode.DRAIN, 'missing'),
      'CREDENTIAL_BINDING_NOT_FOUND',
    );
    const revoked = apply(drained, commands.revokeBinding('conn-1', 'binding-1'));
    expectFault(
      revoked,
      await replacement('binding-3', BindingReplacementMode.DRAIN, 'binding-1'),
      'CREDENTIAL_BINDING_REVOKED',
    );
  });

  it('rejects a replacement whose authentication does not fit the connection scope', async () => {
    const command = commands.replaceBinding(
      await buildReplacement({
        connectionId: 'conn-1',
        previousBindingId: 'binding-1',
        replacement: { bindingId: 'binding-2', authentication: { kind: 'deployment-no-credential' } },
        mode: BindingReplacementMode.DRAIN,
        validation,
      }),
    );

    expectFault(await active(), command, 'PROVIDER_AUTHENTICATION_UNSUPPORTED');
  });
});

describe('revoke binding', () => {
  it('revokes an active binding and leaves none eligible', async () => {
    const state = apply(await active(), commands.revokeBinding('conn-1', 'binding-1'));

    expect(lifecycles(state)).toEqual({ 'binding-1': CredentialBindingLifecycle.REVOKED });
    expect(connectionOf(state).lifecycle).toBe(ProviderConnectionLifecycle.ACTIVE);
  });

  it('revokes a draining binding without touching the active one', async () => {
    const drained = apply(await active(), await replacement('binding-2', BindingReplacementMode.DRAIN));
    const state = apply(drained, commands.revokeBinding('conn-1', 'binding-1'));

    expect(lifecycles(state)).toEqual({
      'binding-1': CredentialBindingLifecycle.REVOKED,
      'binding-2': CredentialBindingLifecycle.ACTIVE,
    });
  });

  it('rejects pending connections, unknown bindings, and revoked bindings', async () => {
    expectFault(await registered(), commands.revokeBinding('conn-1', 'binding-1'), 'PROVIDER_CONNECTION_NOT_ACTIVE');
    expectFault(await active(), commands.revokeBinding('conn-1', 'nope'), 'CREDENTIAL_BINDING_NOT_FOUND');
    const revoked = apply(await active(), commands.revokeBinding('conn-1', 'binding-1'));
    expectFault(revoked, commands.revokeBinding('conn-1', 'binding-1'), 'CREDENTIAL_BINDING_REVOKED');
  });
});

describe('revoke connection', () => {
  it('revokes a pending connection and its binding', async () => {
    const state = apply(await registered(), commands.revokeConnection('conn-1'));

    expect(connectionOf(state).lifecycle).toBe(ProviderConnectionLifecycle.REVOKED);
    expect(lifecycles(state)).toEqual({ 'binding-1': CredentialBindingLifecycle.REVOKED });
  });

  it('revokes every binding of an active connection and is terminal', async () => {
    const drained = apply(await active(), await replacement('binding-2', BindingReplacementMode.DRAIN));
    const state = apply(drained, commands.revokeConnection('conn-1'));

    expect(Object.values(lifecycles(state))).toEqual([
      CredentialBindingLifecycle.REVOKED,
      CredentialBindingLifecycle.REVOKED,
    ]);
    expectFault(state, commands.revokeConnection('conn-1'), 'PROVIDER_CONNECTION_REVOKED');
    expectFault(state, commands.activate('conn-1', validation), 'PROVIDER_CONNECTION_REVOKED');
  });
});

describe('profile and routing policy', () => {
  it('replaces the profile and policy on pending and active connections', async () => {
    for (const base of [await registered(), await active()]) {
      const profiled = apply(base, commands.setProfile('conn-1', { displayName: 'Renamed', description: '' }));
      const state = apply(
        profiled,
        commands.setRoutingPolicy('conn-1', { preference: -3, eligibility: ProviderRoutingEligibility.DISABLED }),
      );

      expect(connectionOf(state).profile).toMatchObject({ displayName: 'Renamed', description: '' });
      expect(connectionOf(state).routingPolicy).toMatchObject({
        preference: -3,
        eligibility: ProviderRoutingEligibility.DISABLED,
      });
    }
  });

  it('rejects changes on a revoked connection and invalid policy values', async () => {
    const revoked = apply(await active(), commands.revokeConnection('conn-1'));

    expectFault(
      revoked,
      commands.setProfile('conn-1', { displayName: 'x', description: '' }),
      'PROVIDER_CONNECTION_REVOKED',
    );
    expectFault(
      revoked,
      commands.setRoutingPolicy('conn-1', { preference: 1, eligibility: ProviderRoutingEligibility.ENABLED }),
      'PROVIDER_CONNECTION_REVOKED',
    );
    expectFault(
      await active(),
      commands.setRoutingPolicy('conn-1', { preference: 1.5, eligibility: ProviderRoutingEligibility.ENABLED }),
      'PROVIDER_COMMAND_INVALID',
    );
    expectFault(
      await active(),
      commands.setRoutingPolicy('conn-1', { preference: 1, eligibility: ProviderRoutingEligibility.UNSPECIFIED }),
      'PROVIDER_COMMAND_INVALID',
    );
    expectFault(
      initialState(),
      commands.setProfile('conn-1', { displayName: 'x', description: '' }),
      'PROVIDER_CONNECTION_NOT_FOUND',
    );
  });
});

describe('fold', () => {
  it('rebuilds state from the event stream', async () => {
    const events: ProviderConnectionEvent[] = [];
    let state = initialState();
    for (const command of [
      commands.register(await buildRegistration(managedDraft())),
      commands.activate('conn-1', validation),
      await replacement('binding-2', BindingReplacementMode.DRAIN),
    ]) {
      const decision = decide(state, command);
      if (!decision.ok) throw decision.fault;
      events.push(...decision.events);
      state = decision.events.reduce(evolve, state);
    }

    expect(fold(events)).toEqual(state);
  });

  it('rejects replay that violates a transition', async () => {
    const decision = decide(initialState(), commands.register(await buildRegistration(managedDraft())));
    if (!decision.ok) throw decision.fault;
    const [registeredEvent] = decision.events;

    expect(() => fold([registeredEvent!, registeredEvent!])).toThrow(ReplayFault);
    expect(() => evolve(initialState(), create(ProviderConnectionEventSchema))).toThrow(ReplayFault);
  });

  it('exposes faults as ProviderConnectionFault instances', async () => {
    const decision = decide(initialState(), commands.revokeConnection('conn-1'));

    expect(decision.ok).toBe(false);
    if (!decision.ok) {
      expect(decision.fault).toBeInstanceOf(ProviderConnectionFault);
      expect(decision.fault.message).toContain('The connection has not been registered.');
    }
  });
});
