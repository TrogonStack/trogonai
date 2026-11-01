import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  CredentialPolicyReason,
  ResourceKind,
  SecretAuthorizationEndReason,
  SecretFailureReason,
  SecretLeaseEndReason,
  SecretMaterialKind,
  SecretPurpose,
  SecretUseAction,
  SecretUseDirection,
  SecretUseMode,
  SecretUseResult,
} from '@trogonai/proto';

import { seedEnvironmentIds } from '@/features/environments/fake/seed';

import { credentialCommands, vaultCommands } from '../domain/commands';
import { fakeDigest } from '../domain/events';
import {
  base64Url,
  secretRefMessage,
  secretVersionRefMessage,
  type LeaseRefId,
  type SecretRefId,
} from '../domain/secret-ref';
import { DEMO_PROJECT, type SeedWriter } from './in-memory-client';

const MINUTE = 60_000;
const HOUR = 3_600_000;
const DAY = 86_400_000;

export function deriveRefId(label: string): SecretRefId {
  return base64Url(fakeDigest(`secret:${label}`)) as SecretRefId;
}

function deriveLeaseId(label: string): LeaseRefId {
  return base64Url(fakeDigest(`lease:${label}`)) as LeaseRefId;
}

export const seedIds = {
  vaults: { production: 'vlt_production', staging: 'vlt_staging', webhooks: 'vlt_webhooks' },
  credentials: {
    openai: deriveRefId('openai-production'),
    github: deriveRefId('github-webhook'),
    stripe: deriveRefId('stripe-webhook'),
    slack: deriveRefId('slack-oauth'),
    linear: deriveRefId('linear-mcp'),
    anthropic: deriveRefId('anthropic-legacy'),
    gemini: deriveRefId('gemini-staging'),
    mistral: deriveRefId('mistral-retired'),
  },
} as const;

interface UseSpec {
  readonly id: string;
  readonly principal: string;
  readonly operation: string;
  readonly purpose: SecretPurpose;
  readonly action: SecretUseAction;
  readonly direction: SecretUseDirection;
  readonly resource: { kind: ResourceKind; id: string };
  readonly audience: string;
  readonly bindingId: string;
  readonly bindingRevision: number;
  readonly version: number;
  readonly previousVersion?: number;
  readonly ttlSeconds: number;
  readonly result: SecretUseResult;
  readonly failure?: SecretFailureReason;
  readonly ended?: SecretAuthorizationEndReason;
}

async function recordUse(writer: SeedWriter, secret: SecretRefId, at: Date, spec: UseSpec): Promise<void> {
  const expires = new Date(at.getTime() + spec.ttlSeconds * 1000);
  await writer.credential(
    credentialCommands.recordAudit(secret, {
      case: 'accepted',
      value: {
        authorization: {
          id: spec.id,
          projectId: DEMO_PROJECT,
          secret: secretRefMessage(secret),
          resource: spec.resource,
          bindingId: spec.bindingId,
          bindingRevision: BigInt(spec.bindingRevision),
          operationId: spec.operation,
          attemptId: `${spec.operation}-1`,
          workloadPrincipalId: spec.principal,
          purpose: spec.purpose,
          action: spec.action,
          direction: spec.direction,
          audience: spec.audience,
          endpointPolicyDigest: fakeDigest(`policy:${spec.audience}`),
          issuedAt: timestampFromDate(at),
          expiresAt: timestampFromDate(expires),
          mode: SecretUseMode.RUNTIME,
          material: { case: 'activeVersion', value: secretVersionRefMessage(secret, spec.version) },
          previousVersion:
            spec.previousVersion !== undefined ? secretVersionRefMessage(secret, spec.previousVersion) : undefined,
        },
      },
    }),
    at,
  );
  const completedAt = new Date(at.getTime() + 800);
  await writer.credential(
    credentialCommands.recordAudit(secret, {
      case: 'completed',
      value: {
        authorizationId: spec.id,
        result: spec.result,
        ...(spec.failure !== undefined ? { failureReason: spec.failure } : {}),
        usedVersions: [secretVersionRefMessage(secret, spec.version)],
      },
    }),
    completedAt,
  );
  if (spec.ended !== undefined) {
    await writer.credential(
      credentialCommands.recordAudit(secret, {
        case: 'authorizationEnded',
        value: { authorizationId: spec.id, reason: spec.ended },
      }),
      new Date(completedAt.getTime() + 400),
    );
  }
}

async function seedLeases(writer: SeedWriter, secret: SecretRefId, now: Date): Promise<void> {
  const endedLease = deriveLeaseId('openai-ended');
  const endedAt = new Date(now.getTime() - 50 * MINUTE);
  const common = {
    workloadPrincipalId: 'wrk_agent-runner',
    purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
    resource: { kind: ResourceKind.CONNECTION, id: 'conn_openai' },
    bindingId: 'bind_openai_primary',
    bindingRevision: 3n,
    audience: 'api.openai.com',
    endpointPolicyDigest: fakeDigest('policy:api.openai.com'),
  };
  const endedProvisioning = { id: 'prov_lease_ended', projectId: DEMO_PROJECT };
  const issuedAt = new Date(endedAt.getTime() + 10_000);
  const issuedExpiry = new Date(issuedAt.getTime() + 5 * MINUTE);
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'reserved',
      value: {
        ...common,
        provisioning: endedProvisioning,
        lease: { id: endedLease },
        operationId: 'op_batch_embed',
        attemptId: 'op_batch_embed-1',
      },
    }),
    endedAt,
  );
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'issued',
      value: {
        lease: { id: endedLease },
        provisioning: endedProvisioning,
        acquisition: { provisioning: endedProvisioning, receiptId: 'rcpt_lease_ended', resource: common.resource },
        issuedAt: timestampFromDate(issuedAt),
        expiresAt: timestampFromDate(issuedExpiry),
        providerExpiresAt: timestampFromDate(issuedExpiry),
        prepared: { streamId: secret, revision: 1n, eventId: 'evt_prepared' },
      },
    }),
    issuedAt,
  );
  const removal = { id: 'prov_lease_removal', projectId: DEMO_PROJECT };
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'useEnded',
      value: { lease: { id: endedLease }, reason: SecretLeaseEndReason.OPERATION_FINISHED, removal },
    }),
    new Date(endedAt.getTime() + 3 * MINUTE),
  );
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'cleanupConfirmed',
      value: {
        lease: { id: endedLease },
        removal,
        cleanupEvidence: [{ streamId: secret, revision: 1n, eventId: 'evt_cleanup' }],
      },
    }),
    new Date(endedAt.getTime() + 3 * MINUTE + 20_000),
  );

  const liveLease = deriveLeaseId('openai-live');
  const liveProvisioning = { id: 'prov_lease_live', projectId: DEMO_PROJECT };
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'reserved',
      value: {
        ...common,
        provisioning: liveProvisioning,
        lease: { id: liveLease },
        operationId: 'op_eval_sweep',
        attemptId: 'op_eval_sweep-1',
      },
    }),
    new Date(now.getTime() - 20_000),
  );
  await writer.credential(
    credentialCommands.recordLease(secret, {
      case: 'issued',
      value: {
        lease: { id: liveLease },
        provisioning: liveProvisioning,
        acquisition: { provisioning: liveProvisioning, receiptId: 'rcpt_lease_live', resource: common.resource },
        issuedAt: timestampFromDate(now),
        expiresAt: timestampFromDate(new Date(now.getTime() + 5 * MINUTE)),
        providerExpiresAt: timestampFromDate(new Date(now.getTime() + 8 * MINUTE)),
        prepared: { streamId: secret, revision: 1n, eventId: 'evt_prepared_live' },
      },
    }),
    now,
  );
}

async function seedOpenAi(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.openai;
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.production,
      displayName: 'OpenAI production key',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-openai-token-v1' },
    }),
    at(80 * DAY),
  );
  await writer.credential(
    credentialCommands.rotate(secret, { material: { case: 'imported', material: 'seed-openai-token-v2' } }),
    at(40 * DAY),
  );
  await writer.credential(
    credentialCommands.rotate(secret, { material: { case: 'imported', material: 'seed-openai-token-v3' } }),
    at(9 * DAY),
  );
  const base = {
    purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
    action: SecretUseAction.RESOLVE,
    direction: SecretUseDirection.OUTBOUND,
    resource: { kind: ResourceKind.CONNECTION, id: 'conn_openai' },
    audience: 'api.openai.com',
    bindingId: 'bind_openai_primary',
    bindingRevision: 3,
    version: 3,
    ttlSeconds: 120,
  };
  const uses: readonly (readonly [number, string, string, string, Partial<UseSpec>])[] = [
    [
      6 * HOUR,
      'auth_openai_01',
      'wrk_gateway',
      'op_chat_9921',
      { result: SecretUseResult.MATERIAL_RELEASED, ended: SecretAuthorizationEndReason.OPERATION_FINISHED },
    ],
    [
      4 * HOUR,
      'auth_openai_02',
      'wrk_agent-runner',
      'op_agent_1180',
      { result: SecretUseResult.MATERIAL_RELEASED, ended: SecretAuthorizationEndReason.EXPIRED },
    ],
    [
      2 * HOUR,
      'auth_openai_03',
      'wrk_gateway',
      'op_chat_9987',
      {
        result: SecretUseResult.FAILED,
        failure: SecretFailureReason.PROVIDER_UNAVAILABLE,
        ended: SecretAuthorizationEndReason.OPERATION_FINISHED,
      },
    ],
    [
      50 * MINUTE,
      'auth_openai_04',
      'wrk_eval-harness',
      'op_eval_3301',
      { result: SecretUseResult.MATERIAL_RELEASED, ended: SecretAuthorizationEndReason.OPERATION_FINISHED },
    ],
    [
      12 * MINUTE,
      'auth_openai_05',
      'wrk_gateway',
      'op_chat_10044',
      { result: SecretUseResult.MATERIAL_RELEASED, ended: SecretAuthorizationEndReason.OPERATION_FINISHED },
    ],
    [90_000, 'auth_openai_06', 'wrk_agent-runner', 'op_agent_1203', { result: SecretUseResult.MATERIAL_RELEASED }],
  ];
  for (const [offset, id, principal, operation, rest] of uses) {
    await recordUse(writer, secret, at(offset), {
      ...base,
      id,
      principal,
      operation,
      result: SecretUseResult.MATERIAL_RELEASED,
      ...rest,
    });
  }
  await writer.credential(
    credentialCommands.recordAudit(secret, {
      case: 'denied',
      value: {
        workloadPrincipalId: 'wrk_untrusted-sandbox',
        operationId: 'op_sandbox_77',
        reason: SecretFailureReason.PERMISSION_DENIED,
      },
    }),
    at(30 * MINUTE),
  );
  await seedLeases(writer, secret, now);
}

async function seedGithub(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.github;
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.webhooks,
      displayName: 'GitHub webhook verification',
      purpose: SecretPurpose.WEBHOOK_VERIFICATION,
      kind: SecretMaterialKind.WEBHOOK_SHARED_SECRET,
      material: { case: 'generated' },
    }),
    at(30 * DAY),
  );
  await writer.credential(
    credentialCommands.rotate(secret, { material: { case: 'generated' }, overlapHours: 12 }),
    at(6 * HOUR),
  );
  const base = {
    purpose: SecretPurpose.WEBHOOK_VERIFICATION,
    action: SecretUseAction.VERIFY_MAC,
    direction: SecretUseDirection.INBOUND,
    resource: { kind: ResourceKind.INBOUND_WEBHOOK_ENDPOINT, id: 'ep_github_main' },
    audience: 'hooks.example.test/github',
    bindingId: 'bind_github_endpoint',
    bindingRevision: 2,
    ttlSeconds: 30,
    principal: 'wrk_webhook-ingress',
    version: 2,
    ended: SecretAuthorizationEndReason.OPERATION_FINISHED,
  };
  await recordUse(writer, secret, at(5 * HOUR), {
    ...base,
    id: 'auth_github_01',
    operation: 'op_delivery_a1',
    previousVersion: 1,
    result: SecretUseResult.VERIFICATION_REJECTED,
  });
  await recordUse(writer, secret, at(3 * HOUR), {
    ...base,
    id: 'auth_github_02',
    operation: 'op_delivery_a2',
    previousVersion: 1,
    result: SecretUseResult.VERIFICATION_MATCHED,
  });
  await recordUse(writer, secret, at(20 * MINUTE), {
    ...base,
    id: 'auth_github_03',
    operation: 'op_delivery_a3',
    result: SecretUseResult.VERIFICATION_MATCHED,
  });
}

async function seedStripe(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.stripe;
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.webhooks,
      displayName: 'Stripe webhook secret',
      purpose: SecretPurpose.WEBHOOK_VERIFICATION,
      kind: SecretMaterialKind.WEBHOOK_SHARED_SECRET,
      material: { case: 'imported', material: 'seed-stripe-signing-secret' },
    }),
    new Date(now.getTime() - 20 * DAY),
  );
  await recordUse(writer, secret, new Date(now.getTime() - 75 * MINUTE), {
    purpose: SecretPurpose.WEBHOOK_VERIFICATION,
    action: SecretUseAction.VERIFY_MAC,
    direction: SecretUseDirection.INBOUND,
    resource: { kind: ResourceKind.INBOUND_WEBHOOK_ENDPOINT, id: 'ep_stripe_billing' },
    audience: 'hooks.example.test/stripe',
    bindingId: 'bind_stripe_endpoint',
    bindingRevision: 1,
    ttlSeconds: 30,
    id: 'auth_stripe_01',
    principal: 'wrk_webhook-ingress',
    operation: 'op_delivery_s1',
    version: 1,
    result: SecretUseResult.VERIFICATION_MATCHED,
    ended: SecretAuthorizationEndReason.OPERATION_FINISHED,
  });
}

async function seedSlack(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.slack;
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.production,
      displayName: 'Slack OAuth refresh',
      purpose: SecretPurpose.OAUTH_REFRESH,
      kind: SecretMaterialKind.OAUTH_REFRESH_TOKEN,
      material: { case: 'imported', material: 'seed-slack-refresh-token' },
    }),
    new Date(now.getTime() - 70 * DAY),
  );
  await writer.credential(credentialCommands.markReconnectRequired(secret), new Date(now.getTime() - 3 * DAY));
}

async function seedLinear(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.linear;
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.staging,
      displayName: 'Linear MCP auth',
      purpose: SecretPurpose.MCP_AUTHENTICATION,
      kind: SecretMaterialKind.OAUTH_ACCESS_TOKEN,
      material: { case: 'imported', material: 'seed-linear-mcp-token' },
    }),
    new Date(now.getTime() - 35 * DAY),
  );
  await writer.credential(credentialCommands.disable(secret), new Date(now.getTime() - 5 * DAY));
}

async function seedAnthropic(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.anthropic;
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.production,
      displayName: 'Old Anthropic key',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-anthropic-token-v1' },
    }),
    at(85 * DAY),
  );
  await writer.credential(
    credentialCommands.rotate(secret, { material: { case: 'imported', material: 'seed-anthropic-token-v2' } }),
    at(60 * DAY),
  );
  await writer.credential(credentialCommands.revoke(secret, CredentialPolicyReason.COMPROMISE), at(4 * DAY));
  await writer.credential(
    credentialCommands.scheduleDestruction(
      secret,
      new Date(now.getTime() + 3 * DAY),
      CredentialPolicyReason.COMPROMISE,
    ),
    at(4 * DAY - 10 * MINUTE),
  );
}

async function seedGemini(writer: SeedWriter, now: Date): Promise<void> {
  await writer.credential(
    credentialCommands.create({
      secret: seedIds.credentials.gemini,
      vaultId: seedIds.vaults.staging,
      displayName: 'Gemini staging key',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-gemini-token-v1' },
      simulation: 'PENDING',
    }),
    new Date(now.getTime() - 4 * MINUTE),
  );
}

async function seedMistral(writer: SeedWriter, now: Date): Promise<void> {
  const secret = seedIds.credentials.mistral;
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.credential(
    credentialCommands.create({
      secret,
      vaultId: seedIds.vaults.staging,
      displayName: 'Retired Mistral key',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-mistral-token-v1' },
    }),
    at(50 * DAY),
  );
  await writer.credential(credentialCommands.revoke(secret, CredentialPolicyReason.OPERATOR_REQUESTED), at(20 * DAY));
  await writer.credential(
    credentialCommands.scheduleDestruction(secret, at(12 * DAY), CredentialPolicyReason.OPERATOR_REQUESTED),
    at(19 * DAY),
  );
  await writer.credential(credentialCommands.confirmDestruction(secret), at(10 * DAY));
}

export async function populateSeed(writer: SeedWriter, now: Date = new Date()): Promise<void> {
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.vault(
    vaultCommands.create({
      vaultId: seedIds.vaults.production,
      displayName: 'Production',
      environment: seedEnvironmentIds.production,
    }),
    at(90 * DAY),
  );
  await writer.vault(
    vaultCommands.create({
      vaultId: seedIds.vaults.staging,
      displayName: 'Staging',
      environment: seedEnvironmentIds.staging,
    }),
    at(60 * DAY),
  );
  await writer.vault(
    vaultCommands.create({
      vaultId: seedIds.vaults.webhooks,
      displayName: 'Webhooks',
      environment: seedEnvironmentIds.production,
    }),
    at(45 * DAY),
  );
  writer.custody(seedIds.vaults.production, 'dedicated');
  writer.custody(seedIds.vaults.webhooks, 'customer_managed');
  await seedOpenAi(writer, now);
  await seedGithub(writer, now);
  await seedStripe(writer, now);
  await seedSlack(writer, now);
  await seedLinear(writer, now);
  await seedAnthropic(writer, now);
  await seedGemini(writer, now);
  await seedMistral(writer, now);
}

export const beaconSeedIds = {
  vault: 'vlt_beacon_live',
  credentials: {
    sendgrid: deriveRefId('beacon-sendgrid'),
    sentry: deriveRefId('beacon-sentry'),
  },
} as const;

export async function populateBeaconSeed(writer: SeedWriter, now: Date = new Date()): Promise<void> {
  const at = (offset: number) => new Date(now.getTime() - offset);
  await writer.vault(
    vaultCommands.create({
      vaultId: beaconSeedIds.vault,
      displayName: 'Beacon live',
      environment: seedEnvironmentIds.production,
    }),
    at(40 * DAY),
  );
  await writer.credential(
    credentialCommands.create({
      secret: beaconSeedIds.credentials.sendgrid,
      vaultId: beaconSeedIds.vault,
      displayName: 'SendGrid delivery key',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-beacon-sendgrid-token' },
    }),
    at(30 * DAY),
  );
  await writer.credential(
    credentialCommands.create({
      secret: beaconSeedIds.credentials.sentry,
      vaultId: beaconSeedIds.vault,
      displayName: 'Sentry ingest token',
      purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
      kind: SecretMaterialKind.API_TOKEN,
      material: { case: 'imported', material: 'seed-beacon-sentry-token' },
    }),
    at(12 * DAY),
  );
}
