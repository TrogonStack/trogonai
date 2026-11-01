import { create } from '@bufbuild/protobuf';
import {
  AwsKmsKeyArnSchema,
  AwsKmsKeyLocatorSchema,
  GoogleCloudKmsCryptoKeyNameSchema,
  GoogleCloudKmsKeyLocatorSchema,
  KeyProvider,
  OpenBaoTransitKeyLocatorSchema,
  OpenBaoTransitKeyPathSchema,
  ProviderKeyLocatorSchema,
  type ProviderKeyLocator,
} from '@trogonai/proto';

const AWS_KEY_ARN = /^arn:aws[a-z-]*:kms:([a-z0-9-]+):(\d{12}):key\/[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/;
const GCP_CRYPTO_KEY =
  /^projects\/([a-z][a-z0-9-]{4,28}[a-z0-9])\/locations\/([a-z0-9-]+)\/keyRings\/([\w-]+)\/cryptoKeys\/([\w-]+)$/;
const OPENBAO_KEY = /^([a-z0-9][a-z0-9_-]*(?:\/[a-z0-9][a-z0-9_-]*)*)\/keys\/([a-z0-9][a-z0-9_-]*)$/;

export const locatorPlaceholders: Record<KeyProvider, string> = {
  [KeyProvider.UNSPECIFIED]: '',
  [KeyProvider.AWS_KMS]: 'arn:aws:kms:eu-west-1:512049177301:key/1c0e7d52-9b4a-4f6e-8a31-6d2f90b7c418',
  [KeyProvider.GOOGLE_CLOUD_KMS]:
    'projects/acme-prod-kms/locations/europe-west1/keyRings/trogon/cryptoKeys/session-messages',
  [KeyProvider.OPENBAO_TRANSIT]: 'transit/keys/trogon-session-messages',
};

export const locatorHints: Record<KeyProvider, string> = {
  [KeyProvider.UNSPECIFIED]: '',
  [KeyProvider.AWS_KMS]: 'The full key ARN. An alias can be repointed to another key, so aliases are never bound.',
  [KeyProvider.GOOGLE_CLOUD_KMS]: 'The fully qualified CryptoKey resource name, without a key version.',
  [KeyProvider.OPENBAO_TRANSIT]: 'The Transit mount and key path, written as mount/keys/name.',
};

export function locatorProblem(provider: KeyProvider, value: string): string | undefined {
  const trimmed = value.trim();
  if (trimmed.length === 0) return 'Enter the exact provider locator of the key.';
  if (provider === KeyProvider.AWS_KMS) {
    if (/(^|[:/])alias\//.test(trimmed)) {
      return 'This is an alias. An alias can be repointed to a different key, so the platform binds only a full key ARN.';
    }
    return AWS_KEY_ARN.test(trimmed)
      ? undefined
      : 'Use the full key ARN, for example arn:aws:kms:eu-west-1:512049177301:key/1c0e7d52-9b4a-4f6e-8a31-6d2f90b7c418.';
  }
  if (provider === KeyProvider.GOOGLE_CLOUD_KMS) {
    if (trimmed.includes('/cryptoKeyVersions/')) {
      return 'This names a key version. Bind the CryptoKey, because versions rotate beneath it.';
    }
    return GCP_CRYPTO_KEY.test(trimmed)
      ? undefined
      : 'Use the fully qualified CryptoKey name: projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY.';
  }
  return OPENBAO_KEY.test(trimmed) ? undefined : 'Use the Transit mount and key path, written as mount/keys/name.';
}

export function buildLocator(provider: KeyProvider, value: string): ProviderKeyLocator | undefined {
  const trimmed = value.trim();
  if (locatorProblem(provider, trimmed) !== undefined) return undefined;
  switch (provider) {
    case KeyProvider.AWS_KMS:
      return create(ProviderKeyLocatorSchema, {
        provider: {
          case: 'awsKms',
          value: create(AwsKmsKeyLocatorSchema, { keyArn: create(AwsKmsKeyArnSchema, { value: trimmed }) }),
        },
      });
    case KeyProvider.GOOGLE_CLOUD_KMS:
      return create(ProviderKeyLocatorSchema, {
        provider: {
          case: 'googleCloudKms',
          value: create(GoogleCloudKmsKeyLocatorSchema, {
            cryptoKey: create(GoogleCloudKmsCryptoKeyNameSchema, { value: trimmed }),
          }),
        },
      });
    case KeyProvider.OPENBAO_TRANSIT:
      return create(ProviderKeyLocatorSchema, {
        provider: {
          case: 'openBaoTransit',
          value: create(OpenBaoTransitKeyLocatorSchema, {
            keyPath: create(OpenBaoTransitKeyPathSchema, { value: trimmed }),
          }),
        },
      });
    default:
      return undefined;
  }
}

export function locatorText(locator: ProviderKeyLocator | undefined): string {
  switch (locator?.provider.case) {
    case 'awsKms':
      return locator.provider.value.keyArn?.value ?? '';
    case 'googleCloudKms':
      return locator.provider.value.cryptoKey?.value ?? '';
    case 'openBaoTransit':
      return locator.provider.value.keyPath?.value ?? '';
    default:
      return '';
  }
}

export function providerOfLocator(locator: ProviderKeyLocator | undefined): KeyProvider {
  switch (locator?.provider.case) {
    case 'awsKms':
      return KeyProvider.AWS_KMS;
    case 'googleCloudKms':
      return KeyProvider.GOOGLE_CLOUD_KMS;
    case 'openBaoTransit':
      return KeyProvider.OPENBAO_TRANSIT;
    default:
      return KeyProvider.UNSPECIFIED;
  }
}

export interface LocatorBoundary {
  readonly account?: string;
  readonly region?: string;
  readonly location?: string;
  readonly project?: string;
  readonly mount?: string;
}

export function boundaryOf(locator: ProviderKeyLocator | undefined): LocatorBoundary {
  const text = locatorText(locator);
  switch (locator?.provider.case) {
    case 'awsKms':
      return { account: AWS_KEY_ARN.exec(text)?.[2], region: AWS_KEY_ARN.exec(text)?.[1] };
    case 'googleCloudKms':
      return { project: GCP_CRYPTO_KEY.exec(text)?.[1], location: GCP_CRYPTO_KEY.exec(text)?.[2] };
    case 'openBaoTransit':
      return { mount: OPENBAO_KEY.exec(text)?.[1] };
    default:
      return {};
  }
}
