import type { CustodyTier } from '@/features/custody/domain/custody';
import type { SecretRefId } from '@/features/secrets/domain/secret-ref';

import type { DeliveryRecord, HourlyDeliveries } from './delivery';
import {
  activeVersionOf,
  isRotatable,
  type CredentialSlot,
  type IntegrationState,
  type SlotState,
  type WebhookRegistration,
} from './integration';
import type { IntegrationId, NatsSubjectPrefix } from './integration-id';
import { formatVersion, type SecretVersion } from './secret-version';
import { failsClosed, sourceKeyUnavailable, type SourceCustody } from './source-custody';
import { slotLabels, type SlotName, type SourceKind } from './source-kind';

export type IntegrationStatus = 'receiving' | 'degraded' | 'paused' | 'misconfigured';

export type RegistrationStatus = WebhookRegistration['status'];

export interface ActionAvailability {
  readonly available: boolean;
  readonly reason?: string;
}

export interface SlotView {
  readonly name: SlotName;
  readonly label: string;
  readonly resolution: 'runtime' | 'static';
  readonly envVar?: string;
  readonly secret?: SecretRefId;
  readonly state?: SlotState;
  readonly version?: SecretVersion;
  readonly rotatable: boolean;
}

export interface RegistrationView {
  readonly status: RegistrationStatus;
  readonly slot?: SlotName;
  readonly registeredVersion?: SecretVersion;
  readonly activeVersion?: SecretVersion;
  readonly mismatch: boolean;
  readonly failure?: string;
  readonly registeredAt?: Date;
}

export interface SourceView {
  readonly id: IntegrationId;
  readonly revision: number;
  readonly kind: SourceKind;
  readonly displayName: string;
  readonly status: IntegrationStatus;
  readonly attention?: string;
  readonly subjectPrefix: NatsSubjectPrefix;
  readonly slots: readonly SlotView[];
  readonly credentialSummary: string;
  readonly registration: RegistrationView;
  readonly custody: SourceCustody;
  readonly tier: CustodyTier;
  readonly lastDeliveryAt: Date | undefined;
  readonly deliveries24h: number;
  readonly rejected24h: number;
  readonly hourly: readonly HourlyDeliveries[];
  readonly recent: readonly DeliveryRecord[];
  readonly settling: boolean;
  readonly createdAt: Date;
  readonly actions: {
    readonly pause: ActionAvailability;
    readonly resume: ActionAvailability;
    readonly rotate: ActionAvailability;
    readonly reregister: ActionAvailability;
  };
}

const yes: ActionAvailability = { available: true };
const no = (reason: string): ActionAvailability => ({ available: false, reason });

function slotView(slot: CredentialSlot): SlotView {
  const base = { name: slot.name, label: slotLabels[slot.name], rotatable: isRotatable(slot) };
  return slot.resolution.mode === 'static'
    ? { ...base, resolution: 'static', envVar: slot.resolution.envVar }
    : {
        ...base,
        resolution: 'runtime',
        secret: slot.resolution.secret,
        state: slot.resolution.state,
        version: slot.resolution.version,
      };
}

function registrationView(registration: WebhookRegistration, slots: readonly CredentialSlot[]): RegistrationView {
  if (registration.status === 'not_applicable') return { status: 'not_applicable', mismatch: false };
  const activeVersion = activeVersionOf(slots.find((slot) => slot.name === registration.slot));
  const mismatch =
    registration.status === 'registered' &&
    registration.registeredVersion !== undefined &&
    activeVersion !== undefined &&
    registration.registeredVersion !== activeVersion;
  return {
    status: registration.status,
    slot: registration.slot,
    registeredVersion: registration.registeredVersion,
    activeVersion,
    mismatch,
    failure: registration.status === 'registration_failed' ? registration.reason : undefined,
    registeredAt: registration.status === 'registered' ? registration.registeredAt : undefined,
  };
}

function runtimeSlots(slots: readonly SlotView[]): readonly SlotView[] {
  return slots.filter((slot) => slot.resolution === 'runtime');
}

function diagnose(
  state: IntegrationState,
  slots: readonly SlotView[],
  registration: RegistrationView,
): { status: IntegrationStatus; attention?: string } {
  if (state.paused) return { status: 'paused' };
  if (failsClosed(state.custody)) {
    return {
      status: 'misconfigured',
      attention:
        'The customer managed key cannot be used. The gateway fails closed and rejects every delivery until the key is usable again.',
    };
  }
  const broken = runtimeSlots(slots).find((slot) => slot.state === 'revoked' || slot.state === 'destroyed');
  if (broken) {
    return {
      status: 'misconfigured',
      attention: `The ${broken.label.toLowerCase()} is ${broken.state}. Nothing can verify deliveries.`,
    };
  }
  const failed = runtimeSlots(slots).find((slot) => slot.state === 'write_failed');
  if (failed) {
    return {
      status: 'degraded',
      attention: `Writing the ${failed.label.toLowerCase()} failed. Rotate it to write a fresh version.`,
    };
  }
  const dirty = runtimeSlots(slots).find((slot) => slot.state === 'cleanup_failed');
  if (dirty) {
    return {
      status: 'degraded',
      attention: `Cleaning up the previous ${dirty.label.toLowerCase()} failed. The old version may still exist.`,
    };
  }
  if (
    registration.mismatch &&
    registration.registeredVersion !== undefined &&
    registration.activeVersion !== undefined
  ) {
    return {
      status: 'degraded',
      attention: `The webhook is registered with a superseded secret (${formatVersion(registration.registeredVersion)}). The active version is ${formatVersion(registration.activeVersion)}. Re-register the webhook.`,
    };
  }
  if (registration.status === 'registration_failed') {
    return { status: 'degraded', attention: registration.failure ?? 'The webhook registration failed.' };
  }
  const pending = runtimeSlots(slots).find(
    (slot) => slot.state === 'pending_write' || slot.state === 'pending_rotation',
  );
  if (pending || registration.status === 'registration_pending') {
    return { status: 'degraded', attention: 'A credential or webhook registration change is still settling.' };
  }
  return { status: 'receiving' };
}

function credentialSummary(slots: readonly SlotView[]): string {
  const runtime = runtimeSlots(slots).length;
  const parts = [
    runtime > 0 ? `${runtime} runtime` : undefined,
    slots.length - runtime > 0 ? `${slots.length - runtime} static` : undefined,
  ].filter((part): part is string => part !== undefined);
  return parts.join(', ');
}

export function toSourceView(state: IntegrationState): SourceView {
  const slots = state.slots.map(slotView);
  const registration = registrationView(state.registration, state.slots);
  const { status, attention } = diagnose(state, slots, registration);
  const settling = state.settlement !== undefined;
  const closed = failsClosed(state.custody);
  const cannotWrap = sourceKeyUnavailable(state.custody, 'wrap');
  const busy = settling ? 'Another change is still settling.' : undefined;
  const rotatable = slots.some((slot) => slot.rotatable);
  return {
    id: state.id,
    revision: state.revision,
    kind: state.kind,
    displayName: state.displayName,
    status,
    attention,
    subjectPrefix: state.subjectPrefix,
    slots,
    credentialSummary: credentialSummary(slots),
    registration,
    custody: state.custody,
    tier: state.custody.tier,
    lastDeliveryAt: state.recent[0]?.receivedAt,
    deliveries24h: state.hourly.reduce((total, hour) => total + hour.accepted + hour.rejected, 0),
    rejected24h: state.hourly.reduce((total, hour) => total + hour.rejected, 0),
    hourly: state.hourly,
    recent: state.recent,
    settling,
    createdAt: state.createdAt,
    actions: {
      pause: state.paused ? no('The integration is already paused.') : yes,
      resume: state.paused ? yes : no('The integration is not paused.'),
      rotate: cannotWrap
        ? no('The customer managed key cannot be used, so credentials cannot be rotated.')
        : busy
          ? no(busy)
          : rotatable
            ? yes
            : no('No runtime credential can be rotated.'),
      reregister:
        registration.status === 'not_applicable'
          ? no('This source does not register a webhook with the provider.')
          : closed
            ? no('The customer managed key cannot be used, so the webhook cannot be re-registered.')
            : busy
              ? no(busy)
              : yes,
    },
  };
}

export interface SourcesOverview {
  readonly total: number;
  readonly receiving: number;
  readonly needsAttention: number;
  readonly rejected24h: number;
}

export function summarizeSources(sources: readonly SourceView[]): SourcesOverview {
  return {
    total: sources.length,
    receiving: sources.filter((source) => source.status === 'receiving').length,
    needsAttention: sources.filter((source) => source.status === 'degraded' || source.status === 'misconfigured')
      .length,
    rejected24h: sources.reduce((total, source) => total + source.rejected24h, 0),
  };
}
