import { KeyProvider } from '@trogonai/proto';

import type { ExternalId } from '../domain/identifiers';
import type {
  AwsRegistrationInput,
  GcpRegistrationInput,
  OpenBaoRegistrationInput,
  RegistrationInput,
} from '../domain/registration';
import type { SetupInstruction } from '../domain/validation';

export const PLATFORM_AWS_PRINCIPAL = 'arn:aws:iam::903318246571:role/trogon-key-custody';
export const PLATFORM_OIDC_ISSUER = 'https://auth.trogon.example';
export const PLATFORM_WORKLOAD_SUBJECT = 'tenant/acme/key-custody';
export const PLATFORM_AUDIENCE = 'trogon-key-custody';

function json(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

function awsInstructions(input: AwsRegistrationInput, externalId: ExternalId): readonly SetupInstruction[] {
  return [
    {
      id: 'aws_trust_policy',
      title: 'Trust policy for the role',
      description:
        'Attach this trust policy to the role you named. It admits only the platform principal, and only when it presents the external ID generated for this tenant.',
      language: 'json',
      content: json({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Principal: { AWS: PLATFORM_AWS_PRINCIPAL },
            Action: 'sts:AssumeRole',
            Condition: { StringEquals: { 'sts:ExternalId': externalId } },
          },
        ],
      }),
    },
    {
      id: 'aws_permissions_policy',
      title: 'Permissions policy for the role',
      description:
        'Grant use of the keys you will bind and nothing else. The platform never creates, deletes or reconfigures keys.',
      language: 'json',
      content: json({
        Version: '2012-10-17',
        Statement: [
          {
            Effect: 'Allow',
            Action: ['kms:Encrypt', 'kms:Decrypt', 'kms:DescribeKey'],
            Resource: `arn:aws:kms:${input.region}:*:key/KEY-ID`,
          },
        ],
      }),
    },
  ];
}

function gcpInstructions(input: GcpRegistrationInput): readonly SetupInstruction[] {
  const pool = /workloadIdentityPools\/([\w-]+)/.exec(input.workloadIdentityProvider)?.[1] ?? 'trogon';
  const provider = /providers\/([\w-]+)/.exec(input.workloadIdentityProvider)?.[1] ?? 'key-custody';
  const impersonating = input.impersonatedServiceAccount.trim();
  const lines = [
    `gcloud iam workload-identity-pools create ${pool} --project=${input.projectId} --location=global`,
    `gcloud iam workload-identity-pools providers create-oidc ${provider} \\`,
    `  --project=${input.projectId} --location=global --workload-identity-pool=${pool} \\`,
    `  --issuer-uri=${PLATFORM_OIDC_ISSUER} \\`,
    `  --allowed-audiences=${PLATFORM_AUDIENCE} \\`,
    `  --attribute-mapping=google.subject=assertion.sub \\`,
    `  --attribute-condition="assertion.sub == '${PLATFORM_WORKLOAD_SUBJECT}'"`,
  ];
  if (impersonating) {
    lines.push(
      '',
      `gcloud iam service-accounts add-iam-policy-binding ${impersonating} \\`,
      `  --role=roles/iam.workloadIdentityUser \\`,
      `  --member="principal://iam.googleapis.com/projects/NUMBER/locations/global/workloadIdentityPools/${pool}/subject/${PLATFORM_WORKLOAD_SUBJECT}"`,
    );
  }
  return [
    {
      id: 'gcp_workload_identity',
      title: 'Workload Identity Federation',
      description:
        'Create the pool and provider so the platform can exchange its own short-lived token for Google credentials. No service account key is ever issued.',
      language: 'bash',
      content: lines.join('\n'),
    },
    {
      id: 'gcp_key_access',
      title: 'Grant use of each key',
      description:
        'Grant the encrypter and decrypter role on each CryptoKey you will bind, not on the project or the key ring.',
      language: 'bash',
      content: [
        'gcloud kms keys add-iam-policy-binding KEY_NAME \\',
        `  --project=${input.projectId} --location=LOCATION --keyring=KEY_RING \\`,
        '  --role=roles/cloudkms.cryptoKeyEncrypterDecrypter \\',
        `  --member="${impersonating ? `serviceAccount:${impersonating}` : `principal://iam.googleapis.com/projects/NUMBER/locations/global/workloadIdentityPools/${pool}/subject/${PLATFORM_WORKLOAD_SUBJECT}`}"`,
      ].join('\n'),
    },
  ];
}

function openBaoInstructions(input: OpenBaoRegistrationInput): readonly SetupInstruction[] {
  const mount = input.transitMount.trim();
  const namespace = input.namespace.trim();
  const role = input.authRole.trim();
  const authentication =
    input.authMethod === 'jwt'
      ? [
          `bao write auth/jwt/role/${role} \\`,
          `  bound_audiences=${PLATFORM_AUDIENCE} \\`,
          `  bound_subject=${PLATFORM_WORKLOAD_SUBJECT} \\`,
          '  token_policies=trogon-key-custody \\',
          '  token_max_ttl=10m',
        ]
      : [
          `bao write auth/cert/certs/${role} \\`,
          '  certificate=@trogon-key-custody-client-ca.pem \\',
          '  token_policies=trogon-key-custody \\',
          '  token_max_ttl=10m',
        ];
  return [
    {
      id: 'openbao_policy',
      title: 'Use-only policy',
      description:
        'The platform may encrypt and decrypt with existing keys. It cannot create, rotate, export or delete them, and it cannot read key material.',
      language: 'hcl',
      content: [
        `path "${mount}/encrypt/*" {`,
        '  capabilities = ["update"]',
        '}',
        '',
        `path "${mount}/decrypt/*" {`,
        '  capabilities = ["update"]',
        '}',
        '',
        `path "${mount}/keys/*" {`,
        '  capabilities = ["read"]',
        '}',
      ].join('\n'),
    },
    {
      id: 'openbao_disable_upsert',
      title: 'Disable upsert and create the role',
      description:
        'With upsert disabled, an encrypt call against a deleted key fails instead of silently creating fresh key material. Run these commands as an administrator.',
      language: 'bash',
      content: [
        ...(namespace !== '' ? [`export BAO_NAMESPACE=${namespace}`] : []),
        `bao write ${mount}/config/keys disable_upsert=true`,
        'bao policy write trogon-key-custody trogon-key-custody.hcl',
        ...authentication,
      ].join('\n'),
    },
  ];
}

export function instructionsFor(
  input: RegistrationInput,
  externalId: ExternalId | undefined,
): readonly SetupInstruction[] {
  if (input.provider === KeyProvider.AWS_KMS) return awsInstructions(input, externalId!);
  if (input.provider === KeyProvider.GOOGLE_CLOUD_KMS) return gcpInstructions(input);
  return openBaoInstructions(input);
}
