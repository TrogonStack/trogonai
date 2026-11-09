import {
  KeyBackendFailureReason,
  KeyCapability,
  KeyOperationKind,
  KeyProvider,
  KeyState,
  KeyUseDenialReason,
  OperationFailureCategory,
  ProviderRequestAbsence,
  type KeyOperationAuditRecord,
} from '@trogonai/proto';

import { customerManagedCustody, dedicatedCustody } from '../domain/custody';
import { migrationId, type KeyBackendId, type KeyRef, type PurposeId } from '../domain/identifiers';
import { externalId } from '../domain/identifiers';
import { USAGE_DAYS, emptyUsage, type DailyUsage, type TenantKey } from '../domain/key';
import { buildLocator } from '../domain/locator';
import type { Migration } from '../domain/migration';
import { buildAccess } from '../domain/registration';
import type { BackendRecord, PayloadSet } from '../domain/state';
import type { InternalCustodyState } from './internal-state';
import {
  attemptOf,
  auditRecordOf,
  bindingDetail,
  healthOf,
  registrationOf,
  stateObservation,
  type AttemptRequest,
  type RecordOutcome,
} from './proto-builders';
import { fakePurposeCatalogue } from './purpose-catalogue';
import { seedBackendIds, seedKeyRefs, seedLocators } from './seed-ids';

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;

const sessionMessages = fakePurposeCatalogue[0]!.id;
const sessionArtifacts = fakePurposeCatalogue[1]!.id;
const workflowInteractions = fakePurposeCatalogue[2]!.id;

const SEED_PEM = '-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUSEEDFAKECERTIFICATE\n-----END CERTIFICATE-----';

function hash(value: string): number {
  let result = 2166136261;
  for (const character of value) result = Math.imul(result ^ character.charCodeAt(0), 16777619);
  return result >>> 0;
}

function noise(seed: number, index: number): number {
  return ((Math.imul(seed ^ (index + 1), 2654435761) >>> 0) % 1000) / 1000;
}

interface UsageShape {
  readonly wraps: number;
  readonly unwraps: number;
  readonly silentDays?: number;
}

function usageOf(seedKey: string, shape: UsageShape): readonly DailyUsage[] {
  const seed = hash(seedKey);
  return Array.from({ length: USAGE_DAYS }, (_, index): DailyUsage => {
    const daysAgo = USAGE_DAYS - 1 - index;
    if (shape.silentDays !== undefined && daysAgo < shape.silentDays) return { wraps: 0, unwraps: 0 };
    return {
      wraps: Math.round(shape.wraps * (0.55 + noise(seed, index) * 0.9)),
      unwraps: Math.round(shape.unwraps * (0.55 + noise(seed, index + 17) * 0.9)),
    };
  });
}

function mustLocator(provider: KeyProvider, text: string) {
  const locator = buildLocator(provider, text);
  if (!locator) throw new Error(`Seed locator ${text} is invalid.`);
  return locator;
}

export function populateCustodySeed(now: Date = new Date()): InternalCustodyState {
  const ago = (ms: number) => new Date(now.getTime() - ms);

  const platformAccess = (host: string) =>
    buildAccess(
      {
        provider: KeyProvider.OPENBAO_TRANSIT,
        displayName: 'platform',
        endpoint: `https://${host}`,
        namespace: '',
        transitMount: 'transit',
        authMethod: 'jwt',
        authRole: 'platform-custody',
        trustRootPem: SEED_PEM,
        version: '2.6.2',
      },
      undefined,
    );

  const backends: BackendRecord[] = [
    {
      displayName: 'Platform key service primary',
      registration: registrationOf({
        id: seedBackendIds.platformPrimary,
        owner: 'platform',
        access: platformAccess('bao-a.platform.trogon.example'),
        health: healthOf({ status: 'connected' }, ago(2 * MINUTE)),
      }),
      registeredAt: ago(240 * DAY),
      revisions: [{ revision: 1, at: ago(240 * DAY), summary: 'Registered by the platform and validated.' }],
    },
    {
      displayName: 'Platform key service secondary',
      registration: registrationOf({
        id: seedBackendIds.platformSecondary,
        owner: 'platform',
        access: platformAccess('bao-b.platform.trogon.example'),
        health: healthOf({ status: 'connected' }, ago(2 * MINUTE)),
      }),
      registeredAt: ago(120 * DAY),
      revisions: [{ revision: 1, at: ago(120 * DAY), summary: 'Registered by the platform and validated.' }],
    },
    {
      displayName: 'Acme AWS production',
      registration: registrationOf({
        id: seedBackendIds.aws,
        owner: 'tenant',
        revision: 2,
        access: buildAccess(
          {
            provider: KeyProvider.AWS_KMS,
            displayName: 'Acme AWS production',
            region: 'eu-west-1',
            roleArn: 'arn:aws:iam::512049177301:role/trogon-key-access',
          },
          externalId('trogon-ext-7f3a9c2e41b0d586'),
        ),
        health: healthOf({ status: 'connected' }, ago(3 * MINUTE)),
      }),
      registeredAt: ago(130 * DAY),
      revisions: [
        { revision: 1, at: ago(130 * DAY), summary: 'Registered after both external ID denials were proven.' },
        {
          revision: 2,
          at: ago(41 * DAY),
          summary: 'Role ARN updated. The new revision took effect after it validated.',
        },
      ],
    },
    {
      displayName: 'Acme Google Cloud KMS',
      registration: registrationOf({
        id: seedBackendIds.gcp,
        owner: 'tenant',
        access: buildAccess(
          {
            provider: KeyProvider.GOOGLE_CLOUD_KMS,
            displayName: 'Acme Google Cloud KMS',
            projectId: 'acme-prod-kms',
            location: 'europe-west1',
            workloadIdentityProvider:
              '//iam.googleapis.com/projects/482916350274/locations/global/workloadIdentityPools/trogon/providers/key-custody',
            impersonatedServiceAccount: 'trogon-kms@acme-prod-kms.iam.gserviceaccount.com',
          },
          undefined,
        ),
        health: healthOf({ status: 'connected' }, ago(4 * MINUTE)),
      }),
      registeredAt: ago(210 * DAY),
      revisions: [
        {
          revision: 1,
          at: ago(210 * DAY),
          summary: 'Registered through Workload Identity Federation with impersonation.',
        },
      ],
    },
    {
      displayName: 'Acme datacenter OpenBao',
      registration: registrationOf({
        id: seedBackendIds.openbao,
        owner: 'tenant',
        access: buildAccess(
          {
            provider: KeyProvider.OPENBAO_TRANSIT,
            displayName: 'Acme datacenter OpenBao',
            endpoint: 'https://bao.acme-corp.example:8200',
            namespace: 'key-custody',
            transitMount: 'transit',
            authMethod: 'jwt',
            authRole: 'trogon-key-custody',
            trustRootPem: SEED_PEM,
            version: '2.6.1',
          },
          undefined,
        ),
        health: healthOf(
          {
            status: 'failed',
            reason: KeyBackendFailureReason.TIMEOUT,
            expiry: {
              serverCertificateNotAfter: new Date(now.getTime() + 12 * DAY),
              trustRootNotAfter: new Date(now.getTime() + 400 * DAY),
              credentialIssuedAt: ago(100 * DAY),
            },
          },
          ago(6 * MINUTE),
        ),
      }),
      registeredAt: ago(72 * DAY),
      revisions: [
        { revision: 1, at: ago(72 * DAY), summary: 'Registered after disable_upsert was confirmed on the mount.' },
      ],
    },
  ];

  const customerKey = (
    ref: KeyRef,
    backendId: KeyBackendId,
    provider: KeyProvider,
    locatorText: string,
    purpose: PurposeId,
    state: KeyState,
    options: { readonly boundDaysAgo: number; readonly observedAgoMs?: number; readonly usage: readonly DailyUsage[] },
  ): TenantKey => {
    const observation = stateObservation(state, ago(options.observedAgoMs ?? 2 * MINUTE));
    return {
      keyRef: ref,
      purpose,
      state: observation,
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
      boundAt: ago(options.boundDaysAgo * DAY),
      usage: options.usage,
      binding: {
        kind: 'customer_managed',
        detail: bindingDetail({
          keyRef: ref,
          backendId,
          purpose,
          locator: mustLocator(provider, locatorText),
          state: observation,
        }),
      },
    };
  };

  const keys: TenantKey[] = [
    {
      keyRef: seedKeyRefs.dedicatedArtifacts,
      purpose: sessionArtifacts,
      state: stateObservation(KeyState.ACTIVE, ago(2 * MINUTE)),
      capabilities: [KeyCapability.ENVELOPE_WRAPPING],
      boundAt: ago(150 * DAY),
      usage: usageOf('dedicated-artifacts', { wraps: 25, unwraps: 180 }),
      binding: { kind: 'dedicated' },
    },
    customerKey(
      seedKeyRefs.awsMessages,
      seedBackendIds.aws,
      KeyProvider.AWS_KMS,
      seedLocators.awsMessages,
      sessionMessages,
      KeyState.ACTIVE,
      { boundDaysAgo: 28, usage: usageOf('aws-messages', { wraps: 60, unwraps: 140 }) },
    ),
    customerKey(
      seedKeyRefs.awsLegacyMessages,
      seedBackendIds.aws,
      KeyProvider.AWS_KMS,
      seedLocators.awsLegacyMessages,
      sessionMessages,
      KeyState.DECRYPT_ONLY,
      { boundDaysAgo: 120, usage: usageOf('aws-legacy', { wraps: 0, unwraps: 6, silentDays: 3 }) },
    ),
    customerKey(
      seedKeyRefs.gcpInteractions,
      seedBackendIds.gcp,
      KeyProvider.GOOGLE_CLOUD_KMS,
      seedLocators.gcpInteractions,
      workflowInteractions,
      KeyState.DISABLED,
      { boundDaysAgo: 205, observedAgoMs: 5 * HOUR, usage: usageOf('gcp-interactions', { wraps: 12, unwraps: 70 }) },
    ),
    customerKey(
      seedKeyRefs.openbaoArtifacts,
      seedBackendIds.openbao,
      KeyProvider.OPENBAO_TRANSIT,
      seedLocators.openbaoArtifacts,
      sessionArtifacts,
      KeyState.ACTIVE,
      { boundDaysAgo: 70, usage: emptyUsage() },
    ),
  ];

  const payload = (
    id: string,
    purpose: PurposeId,
    label: string,
    keyRef: KeyRef | undefined,
    count: number,
  ): PayloadSet => ({
    id,
    purpose,
    label,
    keyRef,
    count,
  });

  const payloads: PayloadSet[] = [
    payload('pl_support_conversations', sessionMessages, 'Support conversations', undefined, 46),
    payload('pl_beacon_conversations', sessionMessages, 'Beacon conversations', seedKeyRefs.awsMessages, 5),
    payload('pl_uploaded_files', sessionArtifacts, 'Uploaded files', seedKeyRefs.dedicatedArtifacts, 18),
    payload('pl_generated_reports', sessionArtifacts, 'Generated reports', seedKeyRefs.dedicatedArtifacts, 3),
    payload('pl_approval_requests', workflowInteractions, 'Approval requests', seedKeyRefs.gcpInteractions, 2),
    payload('pl_run_replies', workflowInteractions, 'Run replies', seedKeyRefs.gcpInteractions, 4),
  ];

  const migrations: Migration[] = [
    {
      id: migrationId('mig_5d2a91c4'),
      operationId: 'op_8c41e07a',
      purpose: sessionMessages,
      from: { tier: 'managed' },
      to: { tier: 'customer_managed', keyRef: seedKeyRefs.awsMessages },
      status: 'running',
      total: 46,
      rewrapped: 29,
      ratePerMinute: 1,
      startedAt: ago(38 * MINUTE),
      updatedAt: now,
    },
    {
      id: migrationId('mig_a3f07be8'),
      operationId: 'op_1f9d6b32',
      purpose: sessionMessages,
      from: { tier: 'customer_managed', keyRef: seedKeyRefs.awsLegacyMessages },
      to: { tier: 'customer_managed', keyRef: seedKeyRefs.awsMessages },
      status: 'completed',
      total: 5,
      rewrapped: 5,
      ratePerMinute: 1,
      startedAt: ago(26 * DAY),
      updatedAt: ago(26 * DAY - 5 * MINUTE),
      completedAt: ago(26 * DAY - 5 * MINUTE),
    },
    {
      id: migrationId('mig_c19e4d70'),
      operationId: 'op_5b7a3e19',
      purpose: sessionArtifacts,
      from: { tier: 'customer_managed', keyRef: seedKeyRefs.openbaoArtifacts },
      to: { tier: 'dedicated', keyRef: seedKeyRefs.dedicatedArtifacts },
      status: 'completed',
      total: 21,
      rewrapped: 21,
      ratePerMinute: 7,
      startedAt: ago(9 * DAY),
      updatedAt: ago(9 * DAY - 3 * MINUTE),
      completedAt: ago(9 * DAY - 3 * MINUTE),
    },
  ];

  const record = (
    id: string,
    agoMs: number,
    operation: KeyOperationKind,
    keyRef: KeyRef,
    backendId: KeyBackendId,
    purpose: PurposeId,
    provider: KeyProvider,
    outcome: RecordOutcome,
    attempts: readonly {
      agoMs: number;
      durationMs: number;
      outcome: 'succeeded' | OperationFailureCategory;
      request: AttemptRequest;
    }[],
  ): KeyOperationAuditRecord =>
    auditRecordOf({
      operationId: `op_${id}`,
      correlationId: `cor_${id}`,
      operation,
      keyRef,
      backendId,
      purpose,
      provider,
      occurredAt: ago(agoMs),
      outcome,
      attempts: attempts.map((attempt, index) =>
        attemptOf({
          number: index + 1,
          startedAt: ago(attempt.agoMs),
          durationMs: attempt.durationMs,
          outcome: attempt.outcome,
          request: attempt.request,
        }),
      ),
    });

  const ok: RecordOutcome = { kind: 'succeeded' };
  const awsRef = seedKeyRefs.awsMessages;
  const operations: KeyOperationAuditRecord[] = [
    record(
      'a01f3c52',
      3 * MINUTE,
      KeyOperationKind.WRAP_DEK,
      awsRef,
      seedBackendIds.aws,
      sessionMessages,
      KeyProvider.AWS_KMS,
      ok,
      [{ agoMs: 3 * MINUTE, durationMs: 41, outcome: 'succeeded', request: '7c3e91a2-5b84-4f0d-a6e1-2d9c8b40f517' }],
    ),
    record(
      'a02d7e19',
      11 * MINUTE,
      KeyOperationKind.UNWRAP_DEK,
      awsRef,
      seedBackendIds.aws,
      sessionMessages,
      KeyProvider.AWS_KMS,
      ok,
      [{ agoMs: 11 * MINUTE, durationMs: 38, outcome: 'succeeded', request: 'e5b20d84-91c7-43aa-8f16-04ad7c3b9e62' }],
    ),
    record(
      'a03b4f86',
      26 * MINUTE,
      KeyOperationKind.UNWRAP_DEK,
      awsRef,
      seedBackendIds.aws,
      sessionMessages,
      KeyProvider.AWS_KMS,
      ok,
      [
        {
          agoMs: 26 * MINUTE + 900,
          durationMs: 120,
          outcome: OperationFailureCategory.PROVIDER_THROTTLED,
          request: '3d8a6f10-27c4-4be9-b5e1-9a0c72d4f385',
        },
        { agoMs: 26 * MINUTE, durationMs: 44, outcome: 'succeeded', request: 'b94c1e07-6a52-4d38-87f3-1e5d09a2c6b4' },
      ],
    ),
    record(
      'a04c8a31',
      2 * HOUR,
      KeyOperationKind.UNWRAP_DEK,
      awsRef,
      seedBackendIds.aws,
      sessionMessages,
      KeyProvider.AWS_KMS,
      ok,
      [
        {
          agoMs: 2 * HOUR,
          durationMs: 52,
          outcome: 'succeeded',
          request: ProviderRequestAbsence.NOT_PROVIDED_BY_PROVIDER,
        },
      ],
    ),
    record(
      'a05e6d90',
      2 * DAY,
      KeyOperationKind.UNWRAP_DEK,
      awsRef,
      seedBackendIds.aws,
      sessionMessages,
      KeyProvider.AWS_KMS,
      { kind: 'failed', category: OperationFailureCategory.TRANSPORT_INTEGRITY_MISMATCH },
      [
        {
          agoMs: 2 * DAY,
          durationMs: 47,
          outcome: OperationFailureCategory.TRANSPORT_INTEGRITY_MISMATCH,
          request: '0f6e3b9d-4c21-48a7-92d5-7be18a4c05f3',
        },
      ],
    ),
    ...[4 * MINUTE, 33 * MINUTE, 71 * MINUTE].map((agoMs, index) =>
      record(
        `b0${index}a7c41`,
        agoMs,
        index === 1 ? KeyOperationKind.WRAP_DEK : KeyOperationKind.UNWRAP_DEK,
        seedKeyRefs.gcpInteractions,
        seedBackendIds.gcp,
        workflowInteractions,
        KeyProvider.GOOGLE_CLOUD_KMS,
        { kind: 'failed', category: OperationFailureCategory.KEY_USE_DENIED, denial: KeyUseDenialReason.KEY_STATE },
        [],
      ),
    ),
    record(
      'c01d9e72',
      14 * MINUTE,
      KeyOperationKind.UNWRAP_DEK,
      seedKeyRefs.openbaoArtifacts,
      seedBackendIds.openbao,
      sessionArtifacts,
      KeyProvider.OPENBAO_TRANSIT,
      { kind: 'failed', category: OperationFailureCategory.PROVIDER_UNAVAILABLE },
      [
        {
          agoMs: 14 * MINUTE + 11_000,
          durationMs: 5000,
          outcome: OperationFailureCategory.PROVIDER_UNAVAILABLE,
          request: ProviderRequestAbsence.NO_RESPONSE,
        },
        {
          agoMs: 14 * MINUTE + 5_000,
          durationMs: 5000,
          outcome: OperationFailureCategory.PROVIDER_UNAVAILABLE,
          request: ProviderRequestAbsence.NO_RESPONSE,
        },
      ],
    ),
    record(
      'c02f1a58',
      47 * MINUTE,
      KeyOperationKind.WRAP_DEK,
      seedKeyRefs.openbaoArtifacts,
      seedBackendIds.openbao,
      sessionArtifacts,
      KeyProvider.OPENBAO_TRANSIT,
      ok,
      [{ agoMs: 47 * MINUTE, durationMs: 210, outcome: 'succeeded', request: '6a1b8c3e-d5f2-4079-b4a6-8e2c11f93d07' }],
    ),
    record(
      'd01a5b63',
      5 * MINUTE,
      KeyOperationKind.UNWRAP_DEK,
      seedKeyRefs.dedicatedArtifacts,
      seedBackendIds.platformSecondary,
      sessionArtifacts,
      KeyProvider.OPENBAO_TRANSIT,
      ok,
      [{ agoMs: 5 * MINUTE, durationMs: 9, outcome: 'succeeded', request: 'platform-request-never-shown' }],
    ),
  ];

  return {
    backends,
    purposes: fakePurposeCatalogue,
    custody: [
      customerManagedCustody(sessionMessages, seedKeyRefs.awsMessages, seedBackendIds.aws, KeyProvider.AWS_KMS),
      dedicatedCustody(sessionArtifacts, seedKeyRefs.dedicatedArtifacts),
      customerManagedCustody(
        workflowInteractions,
        seedKeyRefs.gcpInteractions,
        seedBackendIds.gcp,
        KeyProvider.GOOGLE_CLOUD_KMS,
      ),
    ],
    keys,
    payloads,
    migrations,
    operations,
    placements: new Map<KeyRef, KeyBackendId>([[seedKeyRefs.dedicatedArtifacts, seedBackendIds.platformSecondary]]),
  };
}
