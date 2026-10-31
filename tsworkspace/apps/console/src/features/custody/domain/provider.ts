import { KeyProvider } from '@trogonai/proto';

export const keyProviders = [KeyProvider.AWS_KMS, KeyProvider.GOOGLE_CLOUD_KMS, KeyProvider.OPENBAO_TRANSIT] as const;

export type RegistrableProvider = (typeof keyProviders)[number];

export const keyProviderLabels: Record<KeyProvider, string> = {
  [KeyProvider.UNSPECIFIED]: 'Unknown provider',
  [KeyProvider.AWS_KMS]: 'AWS KMS',
  [KeyProvider.GOOGLE_CLOUD_KMS]: 'Google Cloud KMS',
  [KeyProvider.OPENBAO_TRANSIT]: 'OpenBao',
};
