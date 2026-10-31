import { keyBackendId, keyRef } from '../domain/identifiers';

export const seedBackendIds = {
  platformPrimary: keyBackendId('kbe_platform_primary'),
  platformSecondary: keyBackendId('kbe_platform_secondary'),
  aws: keyBackendId('kbe_aws_acme_prod'),
  gcp: keyBackendId('kbe_gcp_acme_prod'),
  openbao: keyBackendId('kbe_obao_acme_dc'),
} as const;

export const seedKeyRefs = {
  dedicatedArtifacts: keyRef('kr_c63e9012'),
  awsMessages: keyRef('kr_d74f1a23'),
  awsLegacyMessages: keyRef('kr_e8502b34'),
  gcpInteractions: keyRef('kr_f9613c45'),
  openbaoArtifacts: keyRef('kr_0a724d56'),
} as const;

export const seedLocators = {
  awsMessages: 'arn:aws:kms:eu-west-1:512049177301:key/9b1d6c0e-52f4-4a7e-b3a1-7c2f0d8e41aa',
  awsLegacyMessages: 'arn:aws:kms:eu-west-1:512049177301:key/4e7a20c8-1d63-49b5-9f08-a6b3c5d71e92',
  gcpInteractions: 'projects/acme-prod-kms/locations/europe-west1/keyRings/trogon/cryptoKeys/workflow-run-interactions',
  openbaoArtifacts: 'transit/keys/trogon-session-artifacts',
} as const;
