import type { GatedOperation } from '@/features/custody/domain/admission';
import type { SecretRefId } from '@/features/secrets/domain/secret-ref';

import type { SourceCommand } from './commands';
import type { DeliveryRecord, HourlyDeliveries } from './delivery';
import { SourceFault } from './faults';
import type { IntegrationId, NatsSubjectPrefix } from './integration-id';
import { nextVersion, type SecretVersion } from './secret-version';
import { sourceKeyUnavailable, type SourceCustody } from './source-custody';
import type { SlotName, SourceKind } from './source-kind';

export type SlotState =
  | 'active'
  | 'pending_write'
  | 'pending_rotation'
  | 'revoked'
  | 'write_failed'
  | 'cleanup_failed'
  | 'destroyed';

export type SlotResolution =
  | { readonly mode: 'static'; readonly envVar: string }
  | {
      readonly mode: 'runtime';
      readonly secret: SecretRefId;
      readonly state: SlotState;
      readonly version: SecretVersion;
    };

export interface CredentialSlot {
  readonly name: SlotName;
  readonly resolution: SlotResolution;
}

export type WebhookRegistration =
  | { readonly status: 'not_applicable' }
  | {
      readonly status: 'registered';
      readonly slot: SlotName;
      readonly registeredVersion?: SecretVersion;
      readonly registeredAt: Date;
    }
  | {
      readonly status: 'registration_pending';
      readonly slot: SlotName;
      readonly registeredVersion?: SecretVersion;
    }
  | {
      readonly status: 'registration_failed';
      readonly slot: SlotName;
      readonly registeredVersion?: SecretVersion;
      readonly reason: string;
      readonly failedAt: Date;
    };

export interface Settlement {
  readonly operation: 'rotate' | 'reregister';
  readonly slot?: SlotName;
  readonly at: Date;
}

export interface IntegrationState {
  readonly id: IntegrationId;
  readonly revision: number;
  readonly kind: SourceKind;
  readonly displayName: string;
  readonly paused: boolean;
  readonly subjectPrefix: NatsSubjectPrefix;
  readonly slots: readonly CredentialSlot[];
  readonly registration: WebhookRegistration;
  readonly custody: SourceCustody;
  readonly hourly: readonly HourlyDeliveries[];
  readonly recent: readonly DeliveryRecord[];
  readonly createdAt: Date;
  readonly settlement?: Settlement;
}

const rotatableStates: readonly SlotState[] = ['active', 'pending_rotation', 'write_failed', 'cleanup_failed'];
const registrableStates: readonly SlotState[] = ['active', 'pending_rotation'];

export function isRotatable(slot: CredentialSlot): boolean {
  return slot.resolution.mode === 'runtime' && rotatableStates.includes(slot.resolution.state);
}

export function activeVersionOf(slot: CredentialSlot | undefined): SecretVersion | undefined {
  return slot?.resolution.mode === 'runtime' ? slot.resolution.version : undefined;
}

function replaceSlot(
  slots: readonly CredentialSlot[],
  name: SlotName,
  change: (resolution: Extract<SlotResolution, { mode: 'runtime' }>) => SlotResolution,
): readonly CredentialSlot[] {
  return slots.map((slot) =>
    slot.name === name && slot.resolution.mode === 'runtime' ? { ...slot, resolution: change(slot.resolution) } : slot,
  );
}

function guard(state: IntegrationState, operation: GatedOperation): void {
  if (state.settlement) throw new SourceFault('OPERATION_IN_PROGRESS');
  if (sourceKeyUnavailable(state.custody, operation)) throw new SourceFault('KEY_UNAVAILABLE');
}

function advance(state: IntegrationState, change: Partial<IntegrationState>): IntegrationState {
  return { ...state, ...change, revision: state.revision + 1 };
}

export function decideSource(state: IntegrationState, command: SourceCommand, settleAt: Date): IntegrationState {
  switch (command.case) {
    case 'pauseSource':
      if (state.paused) throw new SourceFault('ALREADY_PAUSED');
      return advance(state, { paused: true });
    case 'resumeSource':
      if (!state.paused) throw new SourceFault('NOT_PAUSED');
      return advance(state, { paused: false });
    case 'rotateSlot': {
      guard(state, 'wrap');
      const slot = state.slots.find((candidate) => candidate.name === command.slot);
      if (!slot) throw new SourceFault('SLOT_NOT_FOUND');
      if (!isRotatable(slot)) throw new SourceFault('SLOT_NOT_ROTATABLE');
      const registration = state.registration;
      return advance(state, {
        slots: replaceSlot(state.slots, command.slot, (resolution) => ({ ...resolution, state: 'pending_rotation' })),
        registration:
          registration.status !== 'not_applicable' && registration.slot === command.slot
            ? {
                status: 'registration_pending',
                slot: registration.slot,
                registeredVersion: registration.registeredVersion,
              }
            : registration,
        settlement: { operation: 'rotate', slot: command.slot, at: settleAt },
      });
    }
    case 'reregisterWebhook': {
      guard(state, 'unwrap');
      const registration = state.registration;
      if (registration.status === 'not_applicable') throw new SourceFault('REGISTRATION_NOT_APPLICABLE');
      return advance(state, {
        registration: {
          status: 'registration_pending',
          slot: registration.slot,
          registeredVersion: registration.registeredVersion,
        },
        settlement: { operation: 'reregister', at: settleAt },
      });
    }
  }
}

export function settleSource(state: IntegrationState): IntegrationState {
  const settlement = state.settlement;
  if (!settlement) return state;
  const settled: IntegrationState = { ...state, settlement: undefined, revision: state.revision + 1 };

  if (settlement.operation === 'rotate' && settlement.slot) {
    const name = settlement.slot;
    const slots = replaceSlot(settled.slots, name, (resolution) => ({
      ...resolution,
      state: 'active',
      version: nextVersion(resolution.version),
    }));
    const rotated = activeVersionOf(slots.find((slot) => slot.name === name));
    const registration = settled.registration;
    return {
      ...settled,
      slots,
      registration:
        registration.status !== 'not_applicable' && registration.slot === name
          ? {
              status: 'registered',
              slot: name,
              registeredVersion: rotated,
              registeredAt: settlement.at,
            }
          : registration,
    };
  }

  const registration = settled.registration;
  if (registration.status === 'not_applicable') return settled;
  const target = settled.slots.find((slot) => slot.name === registration.slot);
  if (target?.resolution.mode === 'runtime' && !registrableStates.includes(target.resolution.state)) {
    return {
      ...settled,
      registration: {
        status: 'registration_failed',
        slot: registration.slot,
        registeredVersion: registration.registeredVersion,
        reason: 'The credential is not active, so there is no secret to register.',
        failedAt: settlement.at,
      },
    };
  }
  const version = activeVersionOf(target);
  return {
    ...settled,
    slots: replaceSlot(settled.slots, registration.slot, (resolution) => ({
      ...resolution,
      state: resolution.state === 'pending_rotation' ? 'active' : resolution.state,
    })),
    registration: {
      status: 'registered',
      slot: registration.slot,
      registeredVersion: version,
      registeredAt: settlement.at,
    },
  };
}
