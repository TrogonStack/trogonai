import { create } from '@bufbuild/protobuf';
import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import {
  KeyBackendAdmission,
  KeyBackendLifecycle,
  KeyBackendTrustExpirySchema,
  KeyState,
  KeyProvider,
  KeyUseDenialReason,
} from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { canPause, canResume, keyUseDenial, registrationDenial } from './admission';
import { expiryWarnings } from './backend';
import { custodyTierDetails, custodyTierSummaries } from './custody';
import { keyRef, migrationId, parseKeyBackendId, parseKeyRef, parseMigrationId, purposeId } from './identifiers';
import { canBeDefault, canUnwrap, canWrap, keyStateLabels } from './key-state';
import { advanceMigration, migrationDirection, type Migration } from './migration';
import { boundaryKeyOfAccess, buildAccess, registrationProblems, type OpenBaoRegistrationInput } from './registration';

const now = new Date('2026-03-01T12:00:00Z');
const days = (count: number) => new Date(now.getTime() + count * 86_400_000);

describe('identifiers', () => {
  it('accepts well formed ids and rejects the rest', () => {
    expect(parseKeyBackendId('kbe_aws_acme')).toBe('kbe_aws_acme');
    expect(parseKeyBackendId('aws')).toBeUndefined();
    expect(parseKeyRef('kr_0a724d56')).toBe('kr_0a724d56');
    expect(parseKeyRef('kr_xyz')).toBeUndefined();
    expect(parseMigrationId('mig_5d2a91c4')).toBe('mig_5d2a91c4');
    expect(parseMigrationId('mig')).toBeUndefined();
  });
});

describe('admission', () => {
  it('denies every gated operation while paused, before the key state is consulted', () => {
    expect(registrationDenial(KeyBackendAdmission.PAUSED)).toBe(KeyUseDenialReason.BACKEND_PAUSED);
    expect(registrationDenial(KeyBackendAdmission.ENABLED)).toBeUndefined();
    for (const operation of ['wrap', 'unwrap'] as const) {
      expect(keyUseDenial(KeyBackendAdmission.PAUSED, KeyState.DISABLED, operation)).toBe(
        KeyUseDenialReason.BACKEND_PAUSED,
      );
    }
  });

  it('falls through to the key state matrix when enabled', () => {
    expect(keyUseDenial(KeyBackendAdmission.ENABLED, KeyState.ACTIVE, 'wrap')).toBeUndefined();
    expect(keyUseDenial(KeyBackendAdmission.ENABLED, KeyState.DECRYPT_ONLY, 'wrap')).toBe(KeyUseDenialReason.KEY_STATE);
    expect(keyUseDenial(KeyBackendAdmission.ENABLED, KeyState.DECRYPT_ONLY, 'unwrap')).toBeUndefined();
  });

  it('lets only an active registration be paused or resumed', () => {
    expect(canPause(KeyBackendLifecycle.ACTIVE, KeyBackendAdmission.ENABLED)).toBe(true);
    expect(canPause(KeyBackendLifecycle.PENDING_VALIDATION, KeyBackendAdmission.ENABLED)).toBe(false);
    expect(canResume(KeyBackendLifecycle.ACTIVE, KeyBackendAdmission.PAUSED)).toBe(true);
    expect(canResume(KeyBackendLifecycle.RETIRED, KeyBackendAdmission.PAUSED)).toBe(false);
  });
});

describe('key states', () => {
  it('knows the five states', () => {
    expect(Object.keys(keyStateLabels).length).toBeGreaterThanOrEqual(5);
    expect(canWrap(KeyState.ACTIVE)).toBe(true);
    expect(canWrap(KeyState.DECRYPT_ONLY)).toBe(false);
    expect(canUnwrap(KeyState.DECRYPT_ONLY)).toBe(true);
    for (const state of [KeyState.DISABLED, KeyState.PENDING_DESTRUCTION, KeyState.DESTROYED]) {
      expect(canWrap(state)).toBe(false);
      expect(canUnwrap(state)).toBe(false);
      expect(canBeDefault(state)).toBe(false);
    }
  });
});

describe('custody copy', () => {
  it('makes no hardware, certification or external key manager claim', () => {
    const text = [...Object.values(custodyTierSummaries), ...Object.values(custodyTierDetails)].join(' ');
    expect(text).not.toMatch(/\bHSM\b|FIPS|\bXKS\b|\bEKM\b/);
  });
});

describe('expiry warnings', () => {
  it('keeps absent signals absent', () => {
    expect(expiryWarnings(undefined, now)).toEqual([]);
    expect(expiryWarnings(create(KeyBackendTrustExpirySchema, {}), now)).toEqual([]);
  });

  it('warns about what is near or past, in order, and ignores what is far off', () => {
    const warnings = expiryWarnings(
      create(KeyBackendTrustExpirySchema, {
        serverCertificateNotAfter: timestampFromDate(days(12)),
        clientCertificateNotAfter: timestampFromDate(days(-2)),
        trustRootNotAfter: timestampFromDate(days(400)),
        credentialIssuedAt: timestampFromDate(days(-100)),
      }),
      now,
    );

    expect(warnings.map((warning) => warning.signal)).toEqual([
      'credential_issued',
      'client_certificate',
      'server_certificate',
    ]);
    expect(warnings.find((warning) => warning.signal === 'client_certificate')?.severity).toBe('expired');
  });
});

describe('migrations', () => {
  const base: Migration = {
    id: migrationId('mig_00000001'),
    operationId: 'op_1',
    purpose: purposeId('session_messages'),
    from: { tier: 'managed' },
    to: { tier: 'dedicated', keyRef: keyRef('kr_00000001') },
    status: 'running',
    total: 10,
    rewrapped: 2,
    ratePerMinute: 2,
    startedAt: now,
    updatedAt: now,
  };

  it('names the direction from the two ends', () => {
    const customer = { tier: 'customer_managed', keyRef: keyRef('kr_00000002') } as const;
    expect(migrationDirection(base)).toBe('within_platform');
    expect(migrationDirection({ ...base, to: customer })).toBe('into_customer_managed');
    expect(migrationDirection({ ...base, from: customer })).toBe('out_of_customer_managed');
    expect(migrationDirection({ ...base, from: customer, to: { ...customer, keyRef: keyRef('kr_00000003') } })).toBe(
      'between_customer_managed',
    );
  });

  it('advances at its rate and completes exactly once', () => {
    const half = advanceMigration(base, new Date(now.getTime() + 2 * 60_000));
    expect(half.rewrapped).toBe(6);
    expect(half.status).toBe('running');
    const done = advanceMigration(base, new Date(now.getTime() + 30 * 60_000));
    expect(done.status).toBe('completed');
    expect(done.rewrapped).toBe(10);
    expect(advanceMigration({ ...base, status: 'paused' }, new Date(now.getTime() + 30 * 60_000)).rewrapped).toBe(2);
  });
});

describe('OpenBao namespace', () => {
  const input: OpenBaoRegistrationInput = {
    provider: KeyProvider.OPENBAO_TRANSIT,
    displayName: 'Edge',
    endpoint: 'https://bao.example.com:8200',
    namespace: '',
    transitMount: 'transit',
    authMethod: 'jwt',
    authRole: 'custody',
    trustRootPem: '-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----',
    version: '2.6.1',
  };

  it('leaves the namespace absent for the root namespace', () => {
    const access = buildAccess(input, undefined);
    expect(access.provider.case === 'openBaoTransit' && access.provider.value.namespace).toBeFalsy();
  });

  it('treats a different namespace as a different security boundary', () => {
    const root = boundaryKeyOfAccess(buildAccess(input, undefined));
    const scoped = boundaryKeyOfAccess(buildAccess({ ...input, namespace: 'team-a' }, undefined));
    expect(scoped).not.toBe(root);
  });

  it('rejects a malformed namespace', () => {
    expect(registrationProblems({ ...input, namespace: '/team-a' }).namespace).toBeDefined();
    expect(registrationProblems({ ...input, namespace: 'team-a/edge' }).namespace).toBeUndefined();
  });
});

describe('Google Cloud locations', () => {
  const input = {
    provider: KeyProvider.GOOGLE_CLOUD_KMS,
    displayName: 'Google Cloud',
    projectId: 'acme-prod-kms',
    location: 'europe-west1',
    workloadIdentityProvider:
      '//iam.googleapis.com/projects/123456789012/locations/global/workloadIdentityPools/trogon/providers/console',
    impersonatedServiceAccount: '',
  } as const;

  it('accepts documented regional, multi-region and dual-region locations', () => {
    for (const location of [
      'global',
      'europe-west1',
      'us',
      'eu',
      'asia',
      'ca',
      'europe',
      'nam4',
      'eur4',
      'asia1',
      'nam-eur-asia1',
    ]) {
      expect(registrationProblems({ ...input, location }).location).toBeUndefined();
    }
    for (const location of ['unknown', 'abc1', 'nam999', 'eur999', 'us_east1', 'us-east']) {
      expect(registrationProblems({ ...input, location }).location).toBeDefined();
    }
  });

  it('treats locations in the same project as distinct security boundaries', () => {
    const regional = boundaryKeyOfAccess(buildAccess(input, undefined));
    const multiRegion = boundaryKeyOfAccess(buildAccess({ ...input, location: 'us' }, undefined));

    expect(multiRegion).not.toBe(regional);
  });
});
describe('locator region boundaries', () => {
  it('rejects other AWS regions and GCP locations', async () => {
    const { buildLocator } = await import('./locator');
    const { locatorBoundaryMismatch } = await import('./registration');
    const aws = buildAccess(
      {
        provider: KeyProvider.AWS_KMS,
        displayName: 'AWS',
        region: 'eu-west-1',
        roleArn: 'arn:aws:iam::512049177301:role/access',
      },
      undefined,
    );
    expect(
      locatorBoundaryMismatch(
        aws,
        buildLocator(
          KeyProvider.AWS_KMS,
          'arn:aws:kms:us-east-1:512049177301:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee',
        )!,
      ),
    ).toBeDefined();
    const gcp = buildAccess(
      {
        provider: KeyProvider.GOOGLE_CLOUD_KMS,
        displayName: 'GCP',
        projectId: 'acme-prod-kms',
        location: 'europe-west1',
        workloadIdentityProvider: '',
        impersonatedServiceAccount: '',
      },
      undefined,
    );
    expect(
      locatorBoundaryMismatch(
        gcp,
        buildLocator(
          KeyProvider.GOOGLE_CLOUD_KMS,
          'projects/acme-prod-kms/locations/us-east1/keyRings/ring/cryptoKeys/key',
        )!,
      ),
    ).toBeDefined();
  });
});
