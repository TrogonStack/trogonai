import { create } from '@bufbuild/protobuf';
import { EmptySchema } from '@bufbuild/protobuf/wkt';
import {
  BindingReplacementMode,
  CredentialBindingLifecycle,
  ProviderConnectionLifecycle,
  ProviderConnectionEventSchema,
  ProviderConnectionSchema,
  StateSchema,
  type ActivateProviderConnection,
  type BindingState,
  type CredentialBinding,
  type CredentialBindingReplaced,
  type ProviderConnection,
  type ProviderConnectionEvent,
  type RegisterProviderConnection,
  type ReplaceCredentialBinding,
  type RevokeCredentialBinding,
  type RevokeProviderConnection,
  type SetProviderConnectionProfile,
  type SetProviderRoutingPolicy,
  type State,
} from '@trogonai/proto';

import { invalid, ProviderConnectionFault } from './faults';
import {
  bindingScopeMismatch,
  isLive,
  validateBinding,
  validateReplacementMode,
  validateRegistration,
  validateRoutingPolicy,
} from './validation';

export type ProviderCommand =
  | { readonly case: 'register'; readonly value: RegisterProviderConnection }
  | { readonly case: 'activate'; readonly value: ActivateProviderConnection }
  | { readonly case: 'replaceBinding'; readonly value: ReplaceCredentialBinding }
  | { readonly case: 'revokeBinding'; readonly value: RevokeCredentialBinding }
  | { readonly case: 'revokeConnection'; readonly value: RevokeProviderConnection }
  | { readonly case: 'setProfile'; readonly value: SetProviderConnectionProfile }
  | { readonly case: 'setRoutingPolicy'; readonly value: SetProviderRoutingPolicy };

export interface DecisionContext {
  pinnedSessions(bindingId: string): number;
  permitsDrainOverlap(connection: ProviderConnection): boolean;
}

export const unrestrictedContext: DecisionContext = {
  pinnedSessions: () => 0,
  permitsDrainOverlap: () => true,
};

export type Decision =
  | { readonly ok: true; readonly events: readonly ProviderConnectionEvent[] }
  | { readonly ok: false; readonly fault: ProviderConnectionFault };

export class ReplayFault extends Error {
  constructor(message: string) {
    super(`Replay rejected: ${message}`);
    this.name = 'ReplayFault';
  }
}

export function commandConnectionId(command: ProviderCommand): string {
  return command.case === 'register'
    ? (command.value.registration?.id?.value ?? '')
    : (command.value.connectionId?.value ?? '');
}

export function initialState(): State {
  return create(StateSchema, { state: { case: 'missing', value: create(EmptySchema) } });
}

function presentState(connection: ProviderConnection): State {
  return create(StateSchema, { state: { case: 'present', value: connection } });
}

export function presentConnection(state: State): ProviderConnection | undefined {
  return state.state.case === 'present' ? state.state.value : undefined;
}

function event(init: ProviderConnectionEvent['event']): ProviderConnectionEvent {
  return create(ProviderConnectionEventSchema, { event: init });
}

function findBinding(connection: ProviderConnection, bindingId: string): BindingState | undefined {
  return connection.bindings.find((entry) => entry.binding?.id?.value === bindingId);
}

function requireLive(state: State): ProviderConnection {
  const connection = presentConnection(state);
  if (!connection) throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_FOUND');
  if (connection.lifecycle === ProviderConnectionLifecycle.REVOKED) {
    throw new ProviderConnectionFault('PROVIDER_CONNECTION_REVOKED');
  }
  return connection;
}

function requireScopeCompatible(connection: ProviderConnection, binding: CredentialBinding): void {
  const mismatch = binding.configuration
    ? bindingScopeMismatch(connection.configuration!, binding.configuration)
    : undefined;
  if (mismatch) throw new ProviderConnectionFault('PROVIDER_AUTHENTICATION_UNSUPPORTED', mismatch);
}

function decideRegister(state: State, command: RegisterProviderConnection): ProviderConnectionEvent[] {
  if (state.state.case !== 'missing') throw new ProviderConnectionFault('PROVIDER_CONNECTION_ALREADY_EXISTS');
  const registration = command.registration;
  if (!registration)
    throw invalid([{ path: 'registration', constraint: 'required', message: 'registration is required' }]);
  const violations = validateRegistration(registration);
  if (violations.length > 0) throw invalid(violations);
  const configuration = registration.configuration!;
  const mismatch = bindingScopeMismatch(configuration, registration.initialBinding!.configuration!);
  if (mismatch) throw new ProviderConnectionFault('PROVIDER_AUTHENTICATION_UNSUPPORTED', mismatch);
  return [
    event({
      case: 'registered',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.ProviderConnectionRegistered',
        registration,
      },
    }),
  ];
}

function decideActivate(state: State, command: ActivateProviderConnection): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  if (connection.lifecycle !== ProviderConnectionLifecycle.PENDING) {
    throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_PENDING');
  }
  if (!command.validation?.validationId) {
    throw invalid([
      { path: 'validation.validation_id', constraint: 'min_len', message: 'validation receipt is required' },
    ]);
  }
  const pending = connection.bindings.find((entry) => entry.lifecycle === CredentialBindingLifecycle.PENDING);
  return [
    event({
      case: 'activated',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.ProviderConnectionActivated',
        connectionId: connection.id,
        bindingId: pending!.binding!.id,
        validation: command.validation,
      },
    }),
  ];
}

function decideReplace(
  state: State,
  command: ReplaceCredentialBinding,
  context: DecisionContext,
): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  if (connection.lifecycle !== ProviderConnectionLifecycle.ACTIVE) {
    throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_ACTIVE');
  }
  const replacement = command.replacement;
  const previousId = command.previousBindingId?.value ?? '';
  const violations = [
    ...(replacement
      ? validateBinding(replacement, 'replacement')
      : [{ path: 'replacement', constraint: 'required', message: 'replacement is required' }]),
    ...(previousId
      ? []
      : [{ path: 'previous_binding_id', constraint: 'min_len', message: 'previous binding id is required' }]),
    ...validateReplacementMode(command.mode),
    ...(command.validation?.validationId
      ? []
      : [{ path: 'validation.validation_id', constraint: 'min_len', message: 'validation receipt is required' }]),
  ];
  if (replacement && previousId && previousId === replacement.id?.value) {
    violations.push({
      path: 'replacement.id',
      constraint: 'replace_binding.new_identity',
      message: 'replacement must have a different identity from the previous binding',
    });
  }
  if (violations.length > 0) throw invalid(violations);
  const replacementId = replacement!.id!.value;

  if (findBinding(connection, replacementId)) throw new ProviderConnectionFault('CREDENTIAL_BINDING_IDENTITY_REUSED');
  const previous = findBinding(connection, previousId);
  if (!previous) throw new ProviderConnectionFault('CREDENTIAL_BINDING_NOT_FOUND');
  if (previous.lifecycle === CredentialBindingLifecycle.REVOKED) {
    throw new ProviderConnectionFault('CREDENTIAL_BINDING_REVOKED');
  }
  if (previous.lifecycle !== CredentialBindingLifecycle.ACTIVE) {
    throw new ProviderConnectionFault('CREDENTIAL_BINDING_NOT_ACTIVE');
  }
  requireScopeCompatible(connection, replacement!);

  if (command.mode === BindingReplacementMode.FAIL_IF_IN_USE && context.pinnedSessions(previousId) > 0) {
    throw new ProviderConnectionFault(
      'CREDENTIAL_BINDING_IN_USE',
      `${context.pinnedSessions(previousId)} live Sessions pin binding ${previousId}.`,
    );
  }
  if (command.mode === BindingReplacementMode.DRAIN && !context.permitsDrainOverlap(connection)) {
    throw new ProviderConnectionFault('CREDENTIAL_DRAIN_NOT_PERMITTED');
  }
  return [
    event({
      case: 'bindingReplaced',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.CredentialBindingReplaced',
        connectionId: connection.id,
        previousBindingId: command.previousBindingId,
        replacement,
        mode: command.mode,
        validation: command.validation,
      },
    }),
  ];
}

function decideRevokeBinding(state: State, command: RevokeCredentialBinding): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  if (connection.lifecycle !== ProviderConnectionLifecycle.ACTIVE) {
    throw new ProviderConnectionFault('PROVIDER_CONNECTION_NOT_ACTIVE');
  }
  const target = findBinding(connection, command.bindingId?.value ?? '');
  if (!target) throw new ProviderConnectionFault('CREDENTIAL_BINDING_NOT_FOUND');
  if (target.lifecycle === CredentialBindingLifecycle.REVOKED) {
    throw new ProviderConnectionFault('CREDENTIAL_BINDING_REVOKED');
  }
  return [
    event({
      case: 'bindingRevoked',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.CredentialBindingRevoked',
        connectionId: connection.id,
        bindingId: command.bindingId,
      },
    }),
  ];
}

function decideRevokeConnection(state: State): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  return [
    event({
      case: 'revoked',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.ProviderConnectionRevoked',
        connectionId: connection.id,
      },
    }),
  ];
}

function decideSetProfile(state: State, command: SetProviderConnectionProfile): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  if (!command.profile) throw invalid([{ path: 'profile', constraint: 'required', message: 'profile is required' }]);
  return [
    event({
      case: 'profileSet',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.ProviderConnectionProfileSet',
        connectionId: connection.id,
        profile: command.profile,
      },
    }),
  ];
}

function decideSetRoutingPolicy(state: State, command: SetProviderRoutingPolicy): ProviderConnectionEvent[] {
  const connection = requireLive(state);
  if (!command.routingPolicy) {
    throw invalid([{ path: 'routing_policy', constraint: 'required', message: 'routing policy is required' }]);
  }
  const violations = validateRoutingPolicy(command.routingPolicy, 'routing_policy');
  if (violations.length > 0) throw invalid(violations);
  return [
    event({
      case: 'routingPolicySet',
      value: {
        $typeName: 'trogonai.model_access.provider_connections.v1alpha1.ProviderRoutingPolicySet',
        connectionId: connection.id,
        routingPolicy: command.routingPolicy,
      },
    }),
  ];
}

export function decide(
  state: State,
  command: ProviderCommand,
  context: DecisionContext = unrestrictedContext,
): Decision {
  try {
    switch (command.case) {
      case 'register':
        return { ok: true, events: decideRegister(state, command.value) };
      case 'activate':
        return { ok: true, events: decideActivate(state, command.value) };
      case 'replaceBinding':
        return { ok: true, events: decideReplace(state, command.value, context) };
      case 'revokeBinding':
        return { ok: true, events: decideRevokeBinding(state, command.value) };
      case 'revokeConnection':
        return { ok: true, events: decideRevokeConnection(state) };
      case 'setProfile':
        return { ok: true, events: decideSetProfile(state, command.value) };
      case 'setRoutingPolicy':
        return { ok: true, events: decideSetRoutingPolicy(state, command.value) };
    }
  } catch (error) {
    if (error instanceof ProviderConnectionFault) return { ok: false, fault: error };
    throw error;
  }
}

function withBindingLifecycle(
  bindings: readonly BindingState[],
  bindingId: string,
  lifecycle: CredentialBindingLifecycle,
): BindingState[] {
  return bindings.map((entry) => (entry.binding?.id?.value === bindingId ? { ...entry, lifecycle } : entry));
}

function bindingState(binding: CredentialBinding, lifecycle: CredentialBindingLifecycle): BindingState {
  return {
    $typeName: 'trogonai.model_access.provider_connections.state.v1alpha1.BindingState',
    binding,
    lifecycle,
  };
}

function evolveReplaced(connection: ProviderConnection, replaced: CredentialBindingReplaced): ProviderConnection {
  const previousId = replaced.previousBindingId?.value ?? '';
  const replacement = replaced.replacement!;
  if (connection.lifecycle !== ProviderConnectionLifecycle.ACTIVE)
    throw new ReplayFault('replacement requires an active connection');
  if (findBinding(connection, replacement.id!.value)) throw new ReplayFault('replacement binding id was already used');
  if (findBinding(connection, previousId)?.lifecycle !== CredentialBindingLifecycle.ACTIVE) {
    throw new ReplayFault('previous binding is not the active binding');
  }
  if (replacement.configuration && bindingScopeMismatch(connection.configuration!, replacement.configuration)) {
    throw new ReplayFault('replacement authentication does not match the connection scope');
  }
  const previousLifecycle =
    replaced.mode === BindingReplacementMode.DRAIN
      ? CredentialBindingLifecycle.DRAINING
      : CredentialBindingLifecycle.REVOKED;
  return {
    ...connection,
    bindings: [
      ...withBindingLifecycle(connection.bindings, previousId, previousLifecycle),
      bindingState(replacement, CredentialBindingLifecycle.ACTIVE),
    ],
  };
}

export function evolve(state: State, input: ProviderConnectionEvent): State {
  const next = input.event;
  const connection = presentConnection(state);

  if (next.case === undefined) throw new ReplayFault('event is not set');

  if (next.case === 'registered') {
    if (connection) throw new ReplayFault('connection is already registered');
    const registration = next.value.registration!;
    const binding = registration.initialBinding!;
    if (
      registration.configuration &&
      binding.configuration &&
      bindingScopeMismatch(registration.configuration, binding.configuration)
    ) {
      throw new ReplayFault('initial binding does not match the connection scope');
    }
    return presentState(
      create(ProviderConnectionSchema, {
        id: registration.id,
        configuration: registration.configuration,
        configurationDigest: registration.configurationDigest,
        profile: registration.profile,
        routingPolicy: registration.routingPolicy,
        lifecycle: ProviderConnectionLifecycle.PENDING,
        bindings: [bindingState(binding, CredentialBindingLifecycle.PENDING)],
        configurationBytes: registration.configurationBytes,
      }),
    );
  }

  if (!connection) throw new ReplayFault('event precedes registration');

  switch (next.case) {
    case 'activated': {
      if (connection.lifecycle !== ProviderConnectionLifecycle.PENDING)
        throw new ReplayFault('activation requires a pending connection');
      const bindingId = next.value.bindingId?.value ?? '';
      if (findBinding(connection, bindingId)?.lifecycle !== CredentialBindingLifecycle.PENDING) {
        throw new ReplayFault('activation names a binding that is not pending');
      }
      return presentState({
        ...connection,
        lifecycle: ProviderConnectionLifecycle.ACTIVE,
        bindings: withBindingLifecycle(connection.bindings, bindingId, CredentialBindingLifecycle.ACTIVE),
      });
    }
    case 'bindingReplaced':
      return presentState(evolveReplaced(connection, next.value));
    case 'bindingRevoked': {
      if (connection.lifecycle !== ProviderConnectionLifecycle.ACTIVE)
        throw new ReplayFault('binding revocation requires an active connection');
      const bindingId = next.value.bindingId?.value ?? '';
      const target = findBinding(connection, bindingId);
      if (
        target?.lifecycle !== CredentialBindingLifecycle.ACTIVE &&
        target?.lifecycle !== CredentialBindingLifecycle.DRAINING
      ) {
        throw new ReplayFault('only active or draining bindings can be revoked');
      }
      return presentState({
        ...connection,
        bindings: withBindingLifecycle(connection.bindings, bindingId, CredentialBindingLifecycle.REVOKED),
      });
    }
    case 'revoked':
      if (!isLive(connection.lifecycle)) throw new ReplayFault('connection is already revoked');
      return presentState({
        ...connection,
        lifecycle: ProviderConnectionLifecycle.REVOKED,
        bindings: connection.bindings.map((entry) => ({ ...entry, lifecycle: CredentialBindingLifecycle.REVOKED })),
      });
    case 'profileSet':
      if (!isLive(connection.lifecycle))
        throw new ReplayFault('profile changes require a pending or active connection');
      return presentState({ ...connection, profile: next.value.profile });
    case 'routingPolicySet':
      if (!isLive(connection.lifecycle)) throw new ReplayFault('policy changes require a pending or active connection');
      return presentState({ ...connection, routingPolicy: next.value.routingPolicy });
  }
}

export function fold(events: Iterable<ProviderConnectionEvent>, from: State = initialState()): State {
  let state = from;
  for (const next of events) state = evolve(state, next);
  return state;
}
