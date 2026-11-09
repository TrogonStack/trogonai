import { create } from '@bufbuild/protobuf';
import {
  KeyBackendIdSchema,
  KeyPurposeSchema,
  KeyRefSchema,
  type KeyBackendId as KeyBackendIdMessage,
  type KeyPurpose as KeyPurposeMessage,
  type KeyRef as KeyRefMessage,
} from '@trogonai/proto';

declare const keyBackendIdBrand: unique symbol;
declare const keyRefBrand: unique symbol;
declare const purposeIdBrand: unique symbol;
declare const migrationIdBrand: unique symbol;
declare const attemptIdBrand: unique symbol;
declare const externalIdBrand: unique symbol;

export type KeyBackendId = string & { readonly [keyBackendIdBrand]: true };
export type KeyRef = string & { readonly [keyRefBrand]: true };
export type PurposeId = string & { readonly [purposeIdBrand]: true };
export type MigrationId = string & { readonly [migrationIdBrand]: true };
export type AttemptId = string & { readonly [attemptIdBrand]: true };
export type ExternalId = string & { readonly [externalIdBrand]: true };

const BACKEND_PATTERN = /^kbe_[a-z0-9_]+$/;
const KEY_REF_PATTERN = /^kr_[0-9a-f]{8}$/;
const PURPOSE_PATTERN = /^[a-z][a-z0-9_]*$/;
const MIGRATION_PATTERN = /^mig_[0-9a-f]{8}$/;

export function parseKeyBackendId(value: string): KeyBackendId | undefined {
  return BACKEND_PATTERN.test(value) ? (value as KeyBackendId) : undefined;
}

export function parseKeyRef(value: string): KeyRef | undefined {
  return KEY_REF_PATTERN.test(value) ? (value as KeyRef) : undefined;
}

export function parsePurposeId(value: string): PurposeId | undefined {
  return PURPOSE_PATTERN.test(value) ? (value as PurposeId) : undefined;
}

export function parseMigrationId(value: string): MigrationId | undefined {
  return MIGRATION_PATTERN.test(value) ? (value as MigrationId) : undefined;
}

function required<T>(parsed: T | undefined, value: string, noun: string): T {
  if (parsed === undefined) throw new Error(`${value} is not a ${noun}.`);
  return parsed;
}

export function keyBackendId(value: string): KeyBackendId {
  return required(parseKeyBackendId(value), value, 'key backend id');
}

export function keyRef(value: string): KeyRef {
  return required(parseKeyRef(value), value, 'key reference');
}

export function purposeId(value: string): PurposeId {
  return required(parsePurposeId(value), value, 'key purpose');
}

export function migrationId(value: string): MigrationId {
  return required(parseMigrationId(value), value, 'migration id');
}

export function attemptId(value: string): AttemptId {
  return value as AttemptId;
}

export function externalId(value: string): ExternalId {
  return value as ExternalId;
}

export function keyBackendIdMessage(id: KeyBackendId): KeyBackendIdMessage {
  return create(KeyBackendIdSchema, { value: id });
}

export function keyRefMessage(ref: KeyRef): KeyRefMessage {
  return create(KeyRefSchema, { value: ref });
}

export function purposeMessage(id: PurposeId): KeyPurposeMessage {
  return create(KeyPurposeSchema, { value: id });
}

export function keyBackendIdOf(message: KeyBackendIdMessage | undefined): KeyBackendId {
  return keyBackendId(message?.value ?? '');
}

export function keyRefOf(message: KeyRefMessage | undefined): KeyRef {
  return keyRef(message?.value ?? '');
}

export function purposeIdOf(message: KeyPurposeMessage | undefined): PurposeId {
  return purposeId(message?.value ?? '');
}

export function hexOf(value: number): string {
  return (value >>> 0).toString(16).padStart(8, '0');
}
