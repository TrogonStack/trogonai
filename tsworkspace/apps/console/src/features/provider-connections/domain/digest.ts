import { create, toBinary } from '@bufbuild/protobuf';
import {
  CredentialBindingConfigurationSchema,
  DigestSchema,
  ProviderConnectionConfigurationSchema,
  type CredentialBindingConfiguration,
  type Digest,
  type ProviderConnectionConfiguration,
} from '@trogonai/proto';

export interface CommittedConfiguration<TDigest = Digest> {
  readonly bytes: Uint8Array;
  readonly digest: TDigest;
}

export async function sha256Digest(bytes: Uint8Array): Promise<Digest> {
  const hash = await crypto.subtle.digest('SHA-256', bytes as Uint8Array<ArrayBuffer>);
  return create(DigestSchema, { algorithm: 'sha256', value: new Uint8Array(hash) });
}

export async function commitConnectionConfiguration(
  configuration: ProviderConnectionConfiguration,
): Promise<CommittedConfiguration> {
  const bytes = toBinary(ProviderConnectionConfigurationSchema, configuration);
  return { bytes, digest: await sha256Digest(bytes) };
}

export async function commitBindingConfiguration(
  configuration: CredentialBindingConfiguration,
): Promise<CommittedConfiguration> {
  const bytes = toBinary(CredentialBindingConfigurationSchema, configuration);
  return { bytes, digest: await sha256Digest(bytes) };
}

export function digestsEqual(left: Digest, right: Digest): boolean {
  return (
    left.algorithm === right.algorithm &&
    left.value.length === right.value.length &&
    left.value.every((byte, index) => byte === right.value[index])
  );
}

export function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  return left.length === right.length && left.every((byte, index) => byte === right[index]);
}

export function digestHex(digest: Digest): string {
  return Array.from(digest.value, (byte) => byte.toString(16).padStart(2, '0')).join('');
}
