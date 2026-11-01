import { create } from '@bufbuild/protobuf';
import { EmptySchema } from '@bufbuild/protobuf/wkt';
import {
  CustomerManagedCustodySchema,
  DedicatedCustodySchema,
  PurposeCustodySchema,
  type KeyProvider,
  type PurposeCustody,
} from '@trogonai/proto';

import {
  keyBackendIdMessage,
  keyRefMessage,
  keyRefOf,
  purposeIdOf,
  purposeMessage,
  type KeyBackendId,
  type KeyRef,
  type PurposeId,
} from './identifiers';

export const custodyTiers = ['managed', 'dedicated', 'customer_managed'] as const;

export type CustodyTier = (typeof custodyTiers)[number];

export const custodyTierLabels: Record<CustodyTier, string> = {
  managed: 'Managed key',
  dedicated: 'Dedicated key',
  customer_managed: 'Customer managed key',
};

export const custodyTierSummaries: Record<CustodyTier, string> = {
  managed: 'The default. A key operated by the platform and shared across tenants.',
  dedicated: 'A key operated by the platform and reserved for one tenant and one purpose.',
  customer_managed: 'A key encryption key in a backend you control.',
};

export const custodyTierDetails: Record<CustodyTier, string> = {
  managed:
    'One platform key protects many tenants. Each tenant is separated by an authenticated binding, so one tenant can never ask for material that belongs to another. Managed by the platform, so no key identity is shown.',
  dedicated:
    'The platform operates the key, but it serves one tenant for one purpose. You see its opaque key reference and its state, nothing about where the platform keeps it.',
  customer_managed:
    'The key encryption key lives in a backend you control. You can disable it at your provider at any time, and the platform then fails closed: nothing is decrypted until the key returns.',
};

export function tierOfCustody(custody: PurposeCustody): CustodyTier {
  switch (custody.custody.case) {
    case 'dedicated':
      return 'dedicated';
    case 'customerManaged':
      return 'customer_managed';
    default:
      return 'managed';
  }
}

export function purposeOfCustody(custody: PurposeCustody): PurposeId {
  return purposeIdOf(custody.purpose);
}

export function keyRefOfCustody(custody: PurposeCustody): KeyRef | undefined {
  switch (custody.custody.case) {
    case 'dedicated':
      return keyRefOf(custody.custody.value.keyRef);
    case 'customerManaged':
      return keyRefOf(custody.custody.value.keyRef);
    default:
      return undefined;
  }
}

export function managedCustody(purpose: PurposeId): PurposeCustody {
  return create(PurposeCustodySchema, {
    purpose: purposeMessage(purpose),
    custody: { case: 'managed', value: create(EmptySchema) },
  });
}

export function dedicatedCustody(purpose: PurposeId, ref: KeyRef): PurposeCustody {
  return create(PurposeCustodySchema, {
    purpose: purposeMessage(purpose),
    custody: { case: 'dedicated', value: create(DedicatedCustodySchema, { keyRef: keyRefMessage(ref) }) },
  });
}

export function customerManagedCustody(
  purpose: PurposeId,
  ref: KeyRef,
  backend: KeyBackendId,
  provider: KeyProvider,
): PurposeCustody {
  return create(PurposeCustodySchema, {
    purpose: purposeMessage(purpose),
    custody: {
      case: 'customerManaged',
      value: create(CustomerManagedCustodySchema, {
        keyRef: keyRefMessage(ref),
        keyBackendId: keyBackendIdMessage(backend),
        provider,
      }),
    },
  });
}
