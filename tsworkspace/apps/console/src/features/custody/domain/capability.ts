import { KeyCapability } from '@trogonai/proto';

export const keyCapabilityLabels: Record<KeyCapability, string> = {
  [KeyCapability.UNSPECIFIED]: 'Unknown capability',
  [KeyCapability.ENVELOPE_WRAPPING]: 'Envelope wrapping',
};

export const keyCapabilityDescriptions: Record<KeyCapability, string> = {
  [KeyCapability.UNSPECIFIED]: '',
  [KeyCapability.ENVELOPE_WRAPPING]:
    'Wraps and unwraps data-encryption keys under authenticated data. It is the only capability the platform requires today.',
};
