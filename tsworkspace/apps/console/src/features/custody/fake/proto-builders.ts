import { create } from '@bufbuild/protobuf';
import { EmptySchema, durationFromMs, timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  CorrelationIdSchema,
  CustomerManagedKeyBindingDetailSchema,
  KeyBackendAdmission,
  KeyBackendConfigurationSchema,
  KeyBackendConnectionFailureSchema,
  KeyBackendHealthSchema,
  KeyBackendLifecycle,
  KeyBackendOwnerSchema,
  KeyBackendRegistrationSchema,
  KeyBackendTrustExpirySchema,
  KeyOperationAuditRecordSchema,
  type KeyOperationKind,
  type KeyProvider,
  KeyStateObservationSchema,
  NodeIdSchema,
  type OperationFailureCategory,
  OperationFailureSchema,
  OperationIdSchema,
  ProviderAttemptSchema,
  type ProviderRequestAbsence,
  ProviderRequestIdSchema,
  RegistrationRevisionSchema,
  type CustomerManagedKeyBindingDetail,
  type KeyBackendAccess,
  type KeyBackendFailureReason,
  type KeyBackendHealth,
  type KeyBackendRegistration,
  type KeyOperationAuditRecord,
  type KeyState,
  type KeyStateObservation,
  type KeyUseDenialReason,
  type ProviderAttempt,
  type ProviderKeyLocator,
} from '@trogonai/proto';

import {
  keyBackendIdMessage,
  keyRefMessage,
  purposeMessage,
  type KeyBackendId,
  type KeyRef,
  type PurposeId,
} from '../domain/identifiers';

export const SEED_TENANT = 'node_acme';

export type HealthSpec =
  | { readonly status: 'unknown' }
  | { readonly status: 'connected'; readonly expiry?: TrustExpirySpec }
  | { readonly status: 'failed'; readonly reason: KeyBackendFailureReason; readonly expiry?: TrustExpirySpec };

export interface TrustExpirySpec {
  readonly serverCertificateNotAfter?: Date;
  readonly clientCertificateNotAfter?: Date;
  readonly trustRootNotAfter?: Date;
  readonly credentialIssuedAt?: Date;
  readonly credentialExpiresAt?: Date;
}

function expiryOf(spec: TrustExpirySpec | undefined) {
  if (!spec) return undefined;
  const stamp = (date: Date | undefined) => (date ? timestampFromDate(date) : undefined);
  return create(KeyBackendTrustExpirySchema, {
    serverCertificateNotAfter: stamp(spec.serverCertificateNotAfter),
    clientCertificateNotAfter: stamp(spec.clientCertificateNotAfter),
    trustRootNotAfter: stamp(spec.trustRootNotAfter),
    credentialIssuedAt: stamp(spec.credentialIssuedAt),
    credentialExpiresAt: stamp(spec.credentialExpiresAt),
  });
}

export function healthOf(spec: HealthSpec, observedAt: Date): KeyBackendHealth {
  const status =
    spec.status === 'unknown'
      ? ({ case: 'unknown', value: create(EmptySchema) } as const)
      : spec.status === 'connected'
        ? ({ case: 'connected', value: create(EmptySchema) } as const)
        : ({
            case: 'failed',
            value: create(KeyBackendConnectionFailureSchema, { reason: spec.reason }),
          } as const);
  return create(KeyBackendHealthSchema, {
    status,
    observedAt: timestampFromDate(observedAt),
    trustExpiry: spec.status === 'unknown' ? undefined : expiryOf(spec.expiry),
  });
}

export interface RegistrationSpec {
  readonly id: KeyBackendId;
  readonly owner: 'tenant' | 'platform';
  readonly access: KeyBackendAccess;
  readonly revision?: number;
  readonly lifecycle?: KeyBackendLifecycle;
  readonly admission?: KeyBackendAdmission;
  readonly health?: KeyBackendHealth;
}

export function registrationOf(spec: RegistrationSpec): KeyBackendRegistration {
  return create(KeyBackendRegistrationSchema, {
    keyBackendId: keyBackendIdMessage(spec.id),
    revision: create(RegistrationRevisionSchema, { value: BigInt(spec.revision ?? 1) }),
    configuration: create(KeyBackendConfigurationSchema, {
      owner: create(KeyBackendOwnerSchema, {
        owner:
          spec.owner === 'tenant'
            ? { case: 'tenant', value: create(NodeIdSchema, { value: SEED_TENANT }) }
            : { case: 'platform', value: create(EmptySchema) },
      }),
      access: spec.access,
    }),
    lifecycle: spec.lifecycle ?? KeyBackendLifecycle.ACTIVE,
    admission: spec.admission ?? KeyBackendAdmission.ENABLED,
    health: spec.health,
  });
}

export function withRegistration(
  registration: KeyBackendRegistration,
  patch: Partial<Pick<KeyBackendRegistration, 'lifecycle' | 'admission' | 'health'>> & { revision?: number },
): KeyBackendRegistration {
  return create(KeyBackendRegistrationSchema, {
    ...registration,
    ...patch,
    revision:
      patch.revision === undefined
        ? registration.revision
        : create(RegistrationRevisionSchema, { value: BigInt(patch.revision) }),
  });
}

export function stateObservation(state: KeyState, observedAt: Date): KeyStateObservation {
  return create(KeyStateObservationSchema, { state, observedAt: timestampFromDate(observedAt) });
}

export function bindingDetail(args: {
  readonly keyRef: KeyRef;
  readonly backendId: KeyBackendId;
  readonly purpose: PurposeId;
  readonly locator: ProviderKeyLocator;
  readonly state: KeyStateObservation;
}): CustomerManagedKeyBindingDetail {
  return create(CustomerManagedKeyBindingDetailSchema, {
    keyRef: keyRefMessage(args.keyRef),
    keyBackendId: keyBackendIdMessage(args.backendId),
    purpose: purposeMessage(args.purpose),
    locator: args.locator,
    state: args.state,
  });
}

export type AttemptRequest = string | ProviderRequestAbsence;

export function attemptOf(args: {
  readonly number: number;
  readonly startedAt: Date;
  readonly durationMs: number;
  readonly outcome: 'succeeded' | OperationFailureCategory;
  readonly request: AttemptRequest;
}): ProviderAttempt {
  return create(ProviderAttemptSchema, {
    attemptNumber: args.number,
    startedAt: timestampFromDate(args.startedAt),
    duration: durationFromMs(args.durationMs),
    outcome:
      args.outcome === 'succeeded'
        ? { case: 'succeeded', value: create(EmptySchema) }
        : { case: 'failed', value: args.outcome },
    providerRequest:
      typeof args.request === 'string'
        ? { case: 'providerRequestId', value: create(ProviderRequestIdSchema, { value: args.request }) }
        : { case: 'absence', value: args.request },
  });
}

export type RecordOutcome =
  | { readonly kind: 'succeeded' }
  | { readonly kind: 'failed'; readonly category: OperationFailureCategory; readonly denial?: KeyUseDenialReason };

export function auditRecordOf(args: {
  readonly operationId: string;
  readonly correlationId: string;
  readonly operation: KeyOperationKind;
  readonly keyRef: KeyRef;
  readonly backendId: KeyBackendId;
  readonly purpose: PurposeId;
  readonly provider: KeyProvider;
  readonly occurredAt: Date;
  readonly outcome: RecordOutcome;
  readonly attempts: readonly ProviderAttempt[];
}): KeyOperationAuditRecord {
  return create(KeyOperationAuditRecordSchema, {
    operationId: create(OperationIdSchema, { value: args.operationId }),
    correlationId: create(CorrelationIdSchema, { value: args.correlationId }),
    operation: args.operation,
    tenant: create(NodeIdSchema, { value: SEED_TENANT }),
    keyRef: keyRefMessage(args.keyRef),
    keyBackendId: keyBackendIdMessage(args.backendId),
    purpose: purposeMessage(args.purpose),
    provider: args.provider,
    occurredAt: timestampFromDate(args.occurredAt),
    outcome:
      args.outcome.kind === 'succeeded'
        ? { case: 'succeeded', value: create(EmptySchema) }
        : {
            case: 'failed',
            value: create(OperationFailureSchema, {
              category: args.outcome.category,
              ...(args.outcome.denial === undefined ? {} : { denialReason: args.outcome.denial }),
            }),
          },
    attempts: [...args.attempts],
  });
}
