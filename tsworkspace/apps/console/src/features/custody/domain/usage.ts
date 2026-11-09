import { durationMs, timestampDate } from '@bufbuild/protobuf/wkt';
import {
  KeyOperationKind,
  OperationFailureCategory,
  ProviderRequestAbsence,
  type KeyOperationAuditRecord,
  type OperationFailure,
  type ProviderAttempt,
} from '@trogonai/proto';

import { keyBackendIdOf, keyRefOf, purposeIdOf, type KeyBackendId, type KeyRef, type PurposeId } from './identifiers';

export const operationKindLabels: Record<KeyOperationKind, string> = {
  [KeyOperationKind.UNSPECIFIED]: 'Unknown operation',
  [KeyOperationKind.WRAP_DEK]: 'Wrap',
  [KeyOperationKind.UNWRAP_DEK]: 'Unwrap',
  [KeyOperationKind.BIND_EXTERNAL_KEY]: 'Bind key',
};

export const providerRequestAbsenceLabels: Record<ProviderRequestAbsence, string> = {
  [ProviderRequestAbsence.UNSPECIFIED]: 'Not recorded',
  [ProviderRequestAbsence.NOT_PROVIDED_BY_PROVIDER]: 'Not provided by provider',
  [ProviderRequestAbsence.NO_RESPONSE]: 'No response',
};

export const providerRequestIdNote = 'The identifier the provider returned for this attempt.';

export type ProviderRequestView =
  | { readonly kind: 'id'; readonly id: string }
  | { readonly kind: 'absent'; readonly reason: ProviderRequestAbsence };

export type AttemptOutcomeView =
  | { readonly kind: 'succeeded' }
  | { readonly kind: 'failed'; readonly category: OperationFailureCategory };

export interface AttemptView {
  readonly number: number;
  readonly startedAt: Date;
  readonly durationMs: number;
  readonly outcome: AttemptOutcomeView;
  readonly providerRequest: ProviderRequestView;
}

export type UsageOutcomeView =
  | { readonly kind: 'succeeded' }
  | { readonly kind: 'failed'; readonly failure: OperationFailure };

export interface UsageRecordView {
  readonly operationId: string;
  readonly correlationId: string;
  readonly occurredAt: Date;
  readonly operation: KeyOperationKind;
  readonly keyRef: KeyRef;
  readonly backendId: KeyBackendId;
  readonly purpose: PurposeId;
  readonly outcome: UsageOutcomeView;
  readonly attempts: readonly AttemptView[];
}

function attemptView(attempt: ProviderAttempt): AttemptView {
  return {
    number: attempt.attemptNumber,
    startedAt: attempt.startedAt ? timestampDate(attempt.startedAt) : new Date(0),
    durationMs: attempt.duration ? durationMs(attempt.duration) : 0,
    outcome:
      attempt.outcome.case === 'failed' ? { kind: 'failed', category: attempt.outcome.value } : { kind: 'succeeded' },
    providerRequest:
      attempt.providerRequest.case === 'providerRequestId'
        ? { kind: 'id', id: attempt.providerRequest.value.value }
        : {
            kind: 'absent',
            reason:
              attempt.providerRequest.case === 'absence'
                ? attempt.providerRequest.value
                : ProviderRequestAbsence.UNSPECIFIED,
          },
  };
}

export function toUsageRecordView(record: KeyOperationAuditRecord): UsageRecordView {
  return {
    operationId: record.operationId?.value ?? '',
    correlationId: record.correlationId?.value ?? '',
    occurredAt: record.occurredAt ? timestampDate(record.occurredAt) : new Date(0),
    operation: record.operation,
    keyRef: keyRefOf(record.keyRef),
    backendId: keyBackendIdOf(record.keyBackendId),
    purpose: purposeIdOf(record.purpose),
    outcome:
      record.outcome.case === 'failed' ? { kind: 'failed', failure: record.outcome.value } : { kind: 'succeeded' },
    attempts: record.attempts.map(attemptView),
  };
}
