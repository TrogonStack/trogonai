import { create } from '@bufbuild/protobuf';
import {
  AwsAccountIdSchema,
  AwsAssumeRoleAuthenticationSchema,
  AwsExternalIdSchema,
  AwsKmsAccessSchema,
  AwsRegionSchema,
  AwsRoleArnSchema,
  GoogleCloudFederationAuthenticationSchema,
  GoogleCloudKmsAccessSchema,
  GoogleCloudLocationSchema,
  GoogleCloudProjectIdSchema,
  GoogleCloudServiceAccountSchema,
  GoogleCloudWorkloadIdentityProviderSchema,
  KeyBackendAccessSchema,
  KeyCapability,
  KeyProvider,
  OpenBaoAuthRoleSchema,
  OpenBaoAuthenticationSchema,
  OpenBaoCertificateAuthenticationSchema,
  OpenBaoEndpointSchema,
  OpenBaoJwtAuthenticationSchema,
  OpenBaoTransitAccessSchema,
  OpenBaoNamespaceSchema,
  OpenBaoTransitMountSchema,
  TrustRootSchema,
  type KeyBackendAccess,
  type ProviderKeyLocator,
} from '@trogonai/proto';

import type { ExternalId, PurposeId } from './identifiers';
import { boundaryOf } from './locator';

export const awsRegions = ['us-east-1', 'us-west-2', 'eu-west-1', 'eu-central-1', 'ap-southeast-2'] as const;

export type AwsRegion = (typeof awsRegions)[number];

export type OpenBaoAuthMethod = 'jwt' | 'certificate';

export interface AwsRegistrationInput {
  readonly provider: KeyProvider.AWS_KMS;
  readonly displayName: string;
  readonly region: AwsRegion;
  readonly roleArn: string;
}

export interface GcpRegistrationInput {
  readonly provider: KeyProvider.GOOGLE_CLOUD_KMS;
  readonly displayName: string;
  readonly projectId: string;
  readonly location: string;
  readonly workloadIdentityProvider: string;
  readonly impersonatedServiceAccount: string;
}

export interface OpenBaoRegistrationInput {
  readonly provider: KeyProvider.OPENBAO_TRANSIT;
  readonly displayName: string;
  readonly endpoint: string;
  readonly namespace: string;
  readonly transitMount: string;
  readonly authMethod: OpenBaoAuthMethod;
  readonly authRole: string;
  readonly trustRootPem: string;
  readonly version: string;
}

export type RegistrationInput = AwsRegistrationInput | GcpRegistrationInput | OpenBaoRegistrationInput;

export interface BindingInput {
  readonly backendId: string;
  readonly locator: string;
  readonly purpose: PurposeId;
  readonly capabilities: readonly KeyCapability[];
}

export type FieldProblems = Readonly<Record<string, string>>;

const ROLE_ARN = /^arn:aws:iam::(\d{12}):role\/[\w+=,.@/-]+$/;
const WORKLOAD_PROVIDER =
  /^\/\/iam\.googleapis\.com\/projects\/\d+\/locations\/global\/workloadIdentityPools\/[\w-]+\/providers\/[\w-]+$/;
const SERVICE_ACCOUNT = /^[a-z][a-z0-9-]{4,28}[a-z0-9]@[a-z][a-z0-9-]{4,28}[a-z0-9]\.iam\.gserviceaccount\.com$/;
const PROJECT_ID = /^[a-z][a-z0-9-]{4,28}[a-z0-9]$/;
const REGION = /^[a-z]+-[a-z]+\d+$/;
const MULTI_REGION_LOCATIONS = new Set([
  'global',
  'ca',
  'nam3',
  'nam4',
  'nam6',
  'nam7',
  'nam8',
  'nam9',
  'nam10',
  'nam11',
  'nam12',
  'us',
  'asia',
  'asia1',
  'au',
  'in',
  'jp',
  'de',
  'eu',
  'eur3',
  'eur4',
  'eur5',
  'eur6',
  'eur7',
  'eur8',
  'europe',
  'it',
  'nam-eur-asia1',
]);
const NAMESPACE = /^[A-Za-z0-9][A-Za-z0-9_-]*(?:\/[A-Za-z0-9][A-Za-z0-9_-]*)*$/;
const MOUNT = /^[a-z0-9][a-z0-9_-]*(?:\/[a-z0-9][a-z0-9_-]*)*$/;
const ROLE = /^[a-z0-9][a-z0-9_-]*$/;
const PEM = /^-----BEGIN CERTIFICATE-----[\s\S]+-----END CERTIFICATE-----$/;
const VERSION = /^(\d+)\.(\d+)(?:\.(\d+))?$/;

export const MIN_OPENBAO_VERSION = '2.6';

export function accountOfRole(roleArn: string): string | undefined {
  return ROLE_ARN.exec(roleArn)?.[1];
}

export function versionAtLeast(version: string, minimum: string): boolean {
  const have = VERSION.exec(version);
  const need = VERSION.exec(minimum);
  if (!have || !need) return false;
  const [haveMajor, haveMinor] = [Number(have[1]), Number(have[2])];
  const [needMajor, needMinor] = [Number(need[1]), Number(need[2])];
  return haveMajor > needMajor || (haveMajor === needMajor && haveMinor >= needMinor);
}

function hostProblem(endpoint: string): string | undefined {
  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    return 'Enter a complete https URL.';
  }
  if (url.protocol !== 'https:') return 'The endpoint must use authenticated HTTPS.';
  const host = url.hostname;
  if (
    host === 'localhost' ||
    host.startsWith('127.') ||
    host.startsWith('10.') ||
    host.startsWith('192.168.') ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(host) ||
    host.startsWith('169.254.') ||
    host.endsWith('.internal')
  ) {
    return 'The egress policy rejects loopback, link-local, metadata and undeclared internal destinations.';
  }
  return undefined;
}

export function registrationProblems(input: RegistrationInput): FieldProblems {
  const problems: Record<string, string> = {};
  if (input.displayName.trim().length < 3)
    problems.displayName = 'Give the backend a name of at least three characters.';
  if (input.provider === KeyProvider.AWS_KMS) {
    if (!ROLE_ARN.test(input.roleArn.trim())) {
      problems.roleArn = 'Use the full IAM role ARN, for example arn:aws:iam::512049177301:role/trogon-key-access.';
    }
  } else if (input.provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    if (!PROJECT_ID.test(input.projectId.trim())) problems.projectId = 'Enter the Google Cloud project id.';
    if (!REGION.test(input.location.trim()) && !MULTI_REGION_LOCATIONS.has(input.location.trim()))
      problems.location = 'Enter the location of the keys, such as europe-west1.';
    if (!WORKLOAD_PROVIDER.test(input.workloadIdentityProvider.trim())) {
      problems.workloadIdentityProvider =
        'Use the provider resource: //iam.googleapis.com/projects/NUMBER/locations/global/workloadIdentityPools/POOL/providers/PROVIDER.';
    }
    if (
      input.impersonatedServiceAccount.trim() !== '' &&
      !SERVICE_ACCOUNT.test(input.impersonatedServiceAccount.trim())
    ) {
      problems.impersonatedServiceAccount = 'Enter a service account email, or leave it empty for direct access.';
    }
  } else {
    const host = hostProblem(input.endpoint.trim());
    if (host) problems.endpoint = host;
    if (input.namespace.trim() !== '' && !NAMESPACE.test(input.namespace.trim()))
      problems.namespace = 'Enter the namespace path, such as team-a, or leave it empty for the root namespace.';
    if (!MOUNT.test(input.transitMount.trim()))
      problems.transitMount = 'Enter the Transit mount path, such as transit.';
    if (!ROLE.test(input.authRole.trim())) problems.authRole = 'Enter the auth role the platform logs in with.';
    if (!PEM.test(input.trustRootPem.trim())) {
      problems.trustRootPem = 'Paste the PEM encoded trust root certificate the endpoint chains to.';
    }
    if (!VERSION.test(input.version.trim())) problems.version = 'Enter the OpenBao version, such as 2.6.1.';
  }
  return problems;
}

export function buildAccess(input: RegistrationInput, generatedExternalId: ExternalId | undefined): KeyBackendAccess {
  if (input.provider === KeyProvider.AWS_KMS) {
    const roleArn = input.roleArn.trim();
    return create(KeyBackendAccessSchema, {
      provider: {
        case: 'awsKms',
        value: create(AwsKmsAccessSchema, {
          account: create(AwsAccountIdSchema, { value: accountOfRole(roleArn) ?? '' }),
          region: create(AwsRegionSchema, { value: input.region }),
          authentication: create(AwsAssumeRoleAuthenticationSchema, {
            role: create(AwsRoleArnSchema, { value: roleArn }),
            externalId: create(AwsExternalIdSchema, { value: generatedExternalId ?? '' }),
          }),
        }),
      },
    });
  }
  if (input.provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    const impersonated = input.impersonatedServiceAccount.trim();
    return create(KeyBackendAccessSchema, {
      provider: {
        case: 'googleCloudKms',
        value: create(GoogleCloudKmsAccessSchema, {
          project: create(GoogleCloudProjectIdSchema, { value: input.projectId.trim() }),
          location: create(GoogleCloudLocationSchema, { value: input.location.trim() }),
          authentication: create(GoogleCloudFederationAuthenticationSchema, {
            workloadIdentityProvider: create(GoogleCloudWorkloadIdentityProviderSchema, {
              value: input.workloadIdentityProvider.trim(),
            }),
            ...(impersonated
              ? { impersonatedServiceAccount: create(GoogleCloudServiceAccountSchema, { value: impersonated }) }
              : {}),
          }),
        }),
      },
    });
  }
  const role = create(OpenBaoAuthRoleSchema, { value: input.authRole.trim() });
  const namespace = input.namespace.trim();
  return create(KeyBackendAccessSchema, {
    provider: {
      case: 'openBaoTransit',
      value: create(OpenBaoTransitAccessSchema, {
        endpoint: create(OpenBaoEndpointSchema, { baseUrl: input.endpoint.trim() }),
        transitMount: create(OpenBaoTransitMountSchema, { value: input.transitMount.trim() }),
        authentication: create(OpenBaoAuthenticationSchema, {
          method:
            input.authMethod === 'jwt'
              ? { case: 'jwt', value: create(OpenBaoJwtAuthenticationSchema, { role }) }
              : { case: 'certificate', value: create(OpenBaoCertificateAuthenticationSchema, { role }) },
        }),
        trustRoots: [create(TrustRootSchema, { certificate: new TextEncoder().encode(input.trustRootPem.trim()) })],
        ...(namespace !== '' ? { namespace: create(OpenBaoNamespaceSchema, { value: namespace }) } : {}),
      }),
    },
  });
}

export function boundaryKeyOfAccess(access: KeyBackendAccess | undefined): string {
  switch (access?.provider.case) {
    case 'awsKms':
      return `aws:${access.provider.value.account?.value}:${access.provider.value.region?.value}`;
    case 'googleCloudKms':
      return `gcp:${access.provider.value.project?.value}:${access.provider.value.location?.value}`;
    case 'openBaoTransit': {
      const bao = access.provider.value;
      return `bao:${new URL(bao.endpoint?.baseUrl ?? 'https://unknown.invalid').host}:${bao.namespace?.value ?? ''}:${bao.transitMount?.value}`;
    }
    default:
      return 'unknown';
  }
}

export function bindingProblems(input: BindingInput, hasBackend: boolean): FieldProblems {
  const problems: Record<string, string> = {};
  if (!hasBackend) problems.backendId = 'Choose a registered backend.';
  if (input.capabilities.length === 0) problems.capabilities = 'Request at least the envelope wrapping capability.';
  return problems;
}

export function locatorBoundaryMismatch(
  access: KeyBackendAccess | undefined,
  locator: ProviderKeyLocator,
): string | undefined {
  const boundary = boundaryOf(locator);
  switch (access?.provider.case) {
    case 'awsKms': {
      const account = access.provider.value.account?.value;
      return boundary.account !== account
        ? `The key lives in account ${boundary.account ?? 'unknown'}, but this backend assumes a role in account ${account}.`
        : boundary.region !== access.provider.value.region?.value
          ? 'The key region does not match the registered backend region.'
          : undefined;
    }
    case 'googleCloudKms': {
      const project = access.provider.value.project?.value;
      return boundary.project !== project
        ? `The key lives in project ${boundary.project ?? 'unknown'}, but this backend federates into project ${project}.`
        : boundary.location !== access.provider.value.location?.value
          ? 'The key location does not match the registered backend location.'
          : undefined;
    }
    case 'openBaoTransit': {
      const mount = access.provider.value.transitMount?.value;
      return boundary.mount !== mount
        ? `The key is on mount ${boundary.mount ?? 'unknown'}, but this backend registered the ${mount} mount.`
        : undefined;
    }
    default:
      return undefined;
  }
}
