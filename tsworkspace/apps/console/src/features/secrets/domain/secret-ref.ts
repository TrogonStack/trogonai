import { create } from '@bufbuild/protobuf';
import {
  SecretLeaseRefSchema,
  SecretServiceRefSchema,
  SecretVersionRefSchema,
  type SecretLeaseRef,
  type SecretServiceRef,
  type SecretVersionRef,
} from '@trogonai/proto';

export const SECRET_REF_LENGTH = 43;
const SECRET_REF_PATTERN = /^[A-Za-z0-9_-]{43}$/;
const ENTROPY_BYTES = 32;

declare const secretRefBrand: unique symbol;
declare const leaseRefBrand: unique symbol;

export type SecretRefId = string & { readonly [secretRefBrand]: true };
export type LeaseRefId = string & { readonly [leaseRefBrand]: true };

export function base64Url(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}

export function isOpaqueRef(value: string): boolean {
  return SECRET_REF_PATTERN.test(value);
}

export function mintSecretRefId(): SecretRefId {
  return base64Url(crypto.getRandomValues(new Uint8Array(ENTROPY_BYTES))) as SecretRefId;
}

export function mintLeaseRefId(): LeaseRefId {
  return base64Url(crypto.getRandomValues(new Uint8Array(ENTROPY_BYTES))) as LeaseRefId;
}

export function parseSecretRefId(value: string): SecretRefId | undefined {
  return isOpaqueRef(value) ? (value as SecretRefId) : undefined;
}

export function parseLeaseRefId(value: string): LeaseRefId | undefined {
  return isOpaqueRef(value) ? (value as LeaseRefId) : undefined;
}

export function secretRefMessage(id: SecretRefId): SecretServiceRef {
  return create(SecretServiceRefSchema, { id });
}

export function secretVersionRefMessage(id: SecretRefId, version: number): SecretVersionRef {
  return create(SecretVersionRefSchema, { secret: secretRefMessage(id), version: BigInt(version) });
}

export function leaseRefMessage(id: LeaseRefId): SecretLeaseRef {
  return create(SecretLeaseRefSchema, { id });
}

export function mintShortId(prefix: string): string {
  const bytes = crypto.getRandomValues(new Uint8Array(6));
  return `${prefix}_${base64Url(bytes).replaceAll('_', 'x').replaceAll('-', 'y').toLowerCase()}`;
}

export function mintWebhookSharedSecret(): string {
  return `whsec_${base64Url(crypto.getRandomValues(new Uint8Array(ENTROPY_BYTES)))}`;
}
