import { create } from '@bufbuild/protobuf';
import { timestampDate, timestampFromDate } from '@bufbuild/protobuf/wkt';
import { VaultEventSchema, VaultLifecycle, type VaultEvent } from '@trogonai/proto';

import type { CustodyTier } from '@/features/custody/domain/custody';
import { environmentIdFromStored, type EnvironmentId } from '@/features/environments/domain/environment-id';

import type { VaultCommand, VaultTransition } from './commands';
import type { DecisionContext } from './context';
import { SecretFault } from './faults';
import { vaultLifecycleLabels } from './labels';

export const MAX_DISPLAY_NAME_LENGTH = 120;

export interface VaultState {
  readonly vaultId: string;
  readonly projectId: string;
  readonly displayName: string;
  readonly environment: EnvironmentId;
  readonly custody: CustodyTier;
  readonly lifecycle: VaultLifecycle;
  readonly createdAt: Date;
  readonly updatedAt: Date;
  readonly revision: number;
}

export type VaultDecision =
  | { readonly ok: true; readonly events: readonly VaultEvent[] }
  | { readonly ok: false; readonly fault: SecretFault };

export function validateDisplayName(value: string): string {
  const name = value.trim();
  if (name.length === 0) throw new SecretFault('COMMAND_INVALID', 'A display name is required.');
  if (name.length > MAX_DISPLAY_NAME_LENGTH) {
    throw new SecretFault('COMMAND_INVALID', `A display name is at most ${MAX_DISPLAY_NAME_LENGTH} characters.`);
  }
  return name;
}

const transitions: Record<VaultTransition, { from: readonly VaultLifecycle[]; to: VaultLifecycle }> = {
  DISABLE: { from: [VaultLifecycle.ACTIVE], to: VaultLifecycle.DISABLED },
  ENABLE: { from: [VaultLifecycle.DISABLED], to: VaultLifecycle.ACTIVE },
  SCHEDULE_DESTRUCTION: {
    from: [VaultLifecycle.ACTIVE, VaultLifecycle.DISABLED],
    to: VaultLifecycle.DESTROY_PENDING,
  },
  CONFIRM_DESTROYED: { from: [VaultLifecycle.DESTROY_PENDING], to: VaultLifecycle.DESTROYED },
};

function envelope(state: VaultState | undefined, vaultId: string, context: DecisionContext, index: number) {
  return {
    vaultId,
    projectId: context.projectId,
    eventId: context.newId('evt'),
    revision: BigInt((state?.revision ?? 0) + index + 1),
    occurredAt: timestampFromDate(context.now),
  };
}

function decideOrThrow(
  state: VaultState | undefined,
  command: VaultCommand,
  context: DecisionContext,
): readonly VaultEvent[] {
  switch (command.case) {
    case 'createVault': {
      if (state) throw new SecretFault('IDEMPOTENCY_CONFLICT', 'A vault with this id already exists.');
      if (command.vaultId.length === 0) throw new SecretFault('COMMAND_INVALID', 'A vault id is required.');
      const displayName = validateDisplayName(command.displayName);
      return [
        create(VaultEventSchema, {
          ...envelope(state, command.vaultId, context, 0),
          fact: { case: 'created', value: { displayName, environment: command.environment } },
        }),
      ];
    }
    case 'renameVault': {
      const vault = requireVault(state);
      if (vault.lifecycle === VaultLifecycle.DESTROYED) {
        throw new SecretFault('INVALID_LIFECYCLE', 'A destroyed vault cannot be renamed.');
      }
      const displayName = validateDisplayName(command.displayName);
      if (displayName === vault.displayName) throw new SecretFault('COMMAND_INVALID', 'The name is unchanged.');
      return [
        create(VaultEventSchema, {
          ...envelope(state, vault.vaultId, context, 0),
          fact: { case: 'metadataUpdated', value: { displayName } },
        }),
      ];
    }
    case 'changeVaultLifecycle': {
      const vault = requireVault(state);
      const rule = transitions[command.transition];
      if (!rule.from.includes(vault.lifecycle)) {
        throw new SecretFault(
          'INVALID_LIFECYCLE',
          `A ${vaultLifecycleLabels[vault.lifecycle].toLowerCase()} vault cannot take this transition.`,
        );
      }
      if (rule.to === VaultLifecycle.DESTROY_PENDING || rule.to === VaultLifecycle.DESTROYED) {
        const live = context.liveCredentialCount(vault.vaultId);
        if (live > 0) {
          throw new SecretFault(
            'INVALID_LIFECYCLE',
            `${live} live ${live === 1 ? 'credential remains' : 'credentials remain'} in this vault. Destroy them first.`,
          );
        }
      }
      return [
        create(VaultEventSchema, {
          ...envelope(state, vault.vaultId, context, 0),
          fact: { case: 'lifecycleChanged', value: { lifecycle: rule.to } },
        }),
      ];
    }
  }
}

function requireVault(state: VaultState | undefined): VaultState {
  if (!state) throw new SecretFault('NOT_FOUND', 'The vault does not exist.');
  return state;
}

export function decideVault(
  state: VaultState | undefined,
  command: VaultCommand,
  context: DecisionContext,
): VaultDecision {
  try {
    return { ok: true, events: decideOrThrow(state, command, context) };
  } catch (error) {
    if (error instanceof SecretFault) return { ok: false, fault: error };
    throw error;
  }
}

export function evolveVault(state: VaultState | undefined, event: VaultEvent): VaultState | undefined {
  const at = event.occurredAt ? timestampDate(event.occurredAt) : new Date(0);
  const revision = Number(event.revision);
  switch (event.fact.case) {
    case 'created':
      return {
        vaultId: event.vaultId,
        projectId: event.projectId,
        displayName: event.fact.value.displayName,
        environment: environmentIdFromStored(event.fact.value.environment),
        custody: 'managed',
        lifecycle: VaultLifecycle.ACTIVE,
        createdAt: at,
        updatedAt: at,
        revision,
      };
    case 'metadataUpdated':
      return state && { ...state, displayName: event.fact.value.displayName, updatedAt: at, revision };
    case 'lifecycleChanged':
      return state && { ...state, lifecycle: event.fact.value.lifecycle, updatedAt: at, revision };
    default:
      return state;
  }
}

export function foldVault(events: readonly VaultEvent[]): VaultState | undefined {
  return events.reduce<VaultState | undefined>(evolveVault, undefined);
}
