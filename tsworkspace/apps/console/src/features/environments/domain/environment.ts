import type { EnvironmentCommand } from './commands';
import { isBuiltInEnvironmentId, type EnvironmentId } from './environment-id';
import type { EnvironmentEvent, EnvironmentFact } from './events';
import { EnvironmentFault } from './faults';

export const MAX_DISPLAY_NAME_LENGTH = 60;
export const MAX_DESCRIPTION_LENGTH = 240;

export type EnvironmentLifecycle = 'ACTIVE' | 'DELETED';

export interface EnvironmentState {
  readonly environmentId: EnvironmentId;
  readonly projectId: string;
  readonly displayName: string;
  readonly description: string;
  readonly builtIn: boolean;
  readonly lifecycle: EnvironmentLifecycle;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly revision: number;
}

export interface EnvironmentDecisionContext {
  readonly now: Date;
  readonly projectId: string;
  newId(prefix: string): string;
  vaultsInUse(environmentId: EnvironmentId): number;
}

export type EnvironmentDecision =
  | { readonly ok: true; readonly events: readonly EnvironmentEvent[] }
  | { readonly ok: false; readonly fault: EnvironmentFault };

export function validateDisplayName(value: string): string {
  const name = value.trim();
  if (name.length === 0) throw new EnvironmentFault('COMMAND_INVALID', 'A display name is required.');
  if (name.length > MAX_DISPLAY_NAME_LENGTH) {
    throw new EnvironmentFault('COMMAND_INVALID', `A display name is at most ${MAX_DISPLAY_NAME_LENGTH} characters.`);
  }
  return name;
}

export function validateDescription(value: string): string {
  const description = value.trim();
  if (description.length > MAX_DESCRIPTION_LENGTH) {
    throw new EnvironmentFault('COMMAND_INVALID', `A description is at most ${MAX_DESCRIPTION_LENGTH} characters.`);
  }
  return description;
}

function present(state: EnvironmentState | undefined): EnvironmentState | undefined {
  return state?.lifecycle === 'ACTIVE' ? state : undefined;
}

function requireEnvironment(state: EnvironmentState | undefined): EnvironmentState {
  const environment = present(state);
  if (!environment) throw new EnvironmentFault('ENVIRONMENT_NOT_FOUND');
  return environment;
}

function event(
  state: EnvironmentState | undefined,
  environmentId: EnvironmentId,
  context: EnvironmentDecisionContext,
  fact: EnvironmentFact,
): EnvironmentEvent {
  return {
    environmentId,
    projectId: context.projectId,
    eventId: context.newId('evt'),
    revision: (state?.revision ?? 0) + 1,
    occurredAt: context.now,
    fact,
  };
}

function decideOrThrow(
  state: EnvironmentState | undefined,
  command: EnvironmentCommand,
  context: EnvironmentDecisionContext,
): readonly EnvironmentEvent[] {
  switch (command.case) {
    case 'createEnvironment': {
      if (present(state)) throw new EnvironmentFault('ENVIRONMENT_ALREADY_EXISTS');
      const displayName = validateDisplayName(command.displayName);
      const description = validateDescription(command.description);
      return [
        event(state, command.environmentId, context, {
          case: 'created',
          displayName,
          description,
          builtIn: isBuiltInEnvironmentId(command.environmentId),
        }),
      ];
    }
    case 'updateEnvironment': {
      const environment = requireEnvironment(state);
      const displayName = validateDisplayName(command.displayName);
      const description = validateDescription(command.description);
      if (displayName === environment.displayName && description === environment.description) {
        throw new EnvironmentFault('COMMAND_INVALID', 'Nothing changed.');
      }
      return [event(state, environment.environmentId, context, { case: 'metadataUpdated', displayName, description })];
    }
    case 'deleteEnvironment': {
      const environment = requireEnvironment(state);
      if (environment.builtIn) throw new EnvironmentFault('ENVIRONMENT_BUILT_IN');
      const vaults = context.vaultsInUse(environment.environmentId);
      if (vaults > 0) {
        throw new EnvironmentFault(
          'ENVIRONMENT_IN_USE',
          `${vaults} ${vaults === 1 ? 'vault uses' : 'vaults use'} it. Destroy ${vaults === 1 ? 'it' : 'them'} first.`,
        );
      }
      return [event(state, environment.environmentId, context, { case: 'deleted' })];
    }
  }
}

export function decideEnvironment(
  state: EnvironmentState | undefined,
  command: EnvironmentCommand,
  context: EnvironmentDecisionContext,
): EnvironmentDecision {
  try {
    return { ok: true, events: decideOrThrow(state, command, context) };
  } catch (error) {
    if (error instanceof EnvironmentFault) return { ok: false, fault: error };
    throw error;
  }
}

export function evolveEnvironment(
  state: EnvironmentState | undefined,
  occurred: EnvironmentEvent,
): EnvironmentState | undefined {
  const { fact, revision, occurredAt } = occurred;
  switch (fact.case) {
    case 'created':
      return {
        environmentId: occurred.environmentId,
        projectId: occurred.projectId,
        displayName: fact.displayName,
        description: fact.description,
        builtIn: fact.builtIn,
        lifecycle: 'ACTIVE',
        createdAt: occurredAt,
        updatedAt: occurredAt,
        revision,
      };
    case 'metadataUpdated':
      return (
        state && {
          ...state,
          displayName: fact.displayName,
          description: fact.description,
          updatedAt: occurredAt,
          revision,
        }
      );
    case 'deleted':
      return state && { ...state, lifecycle: 'DELETED', updatedAt: occurredAt, revision };
  }
}

export function foldEnvironment(events: readonly EnvironmentEvent[]): EnvironmentState | undefined {
  return events.reduce<EnvironmentState | undefined>(evolveEnvironment, undefined);
}
