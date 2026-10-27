import { SecretMaterialKind, SecretPurpose } from '@trogonai/proto';

export const MAX_OVERLAP_HOURS = 24;
export const MAX_AUTHORIZATION_SECONDS = 300;

export const selectablePurposes: readonly SecretPurpose[] = [
  SecretPurpose.PROVIDER_AUTHENTICATION,
  SecretPurpose.TOOL_AUTHENTICATION,
  SecretPurpose.MCP_AUTHENTICATION,
  SecretPurpose.OAUTH_REFRESH,
  SecretPurpose.WEBHOOK_VERIFICATION,
  SecretPurpose.WEBHOOK_SIGNING,
  SecretPurpose.WEBHOOK_DESTINATION,
];

const bearerKinds: readonly SecretMaterialKind[] = [
  SecretMaterialKind.API_TOKEN,
  SecretMaterialKind.BASIC_AUTHENTICATION,
  SecretMaterialKind.OAUTH_ACCESS_TOKEN,
];

export function compatibleKinds(purpose: SecretPurpose): readonly SecretMaterialKind[] {
  switch (purpose) {
    case SecretPurpose.PROVIDER_AUTHENTICATION:
    case SecretPurpose.TOOL_AUTHENTICATION:
    case SecretPurpose.MCP_AUTHENTICATION:
      return bearerKinds;
    case SecretPurpose.WEBHOOK_VERIFICATION:
    case SecretPurpose.WEBHOOK_SIGNING:
      return [SecretMaterialKind.WEBHOOK_SHARED_SECRET];
    case SecretPurpose.OAUTH_REFRESH:
      return [SecretMaterialKind.OAUTH_REFRESH_TOKEN];
    case SecretPurpose.WEBHOOK_DESTINATION:
      return [SecretMaterialKind.WEBHOOK_DESTINATION];
    default:
      return [];
  }
}

export function isCompatible(purpose: SecretPurpose, kind: SecretMaterialKind): boolean {
  return compatibleKinds(purpose).includes(kind);
}

export function isWebhookPurpose(purpose: SecretPurpose): boolean {
  return purpose === SecretPurpose.WEBHOOK_VERIFICATION || purpose === SecretPurpose.WEBHOOK_SIGNING;
}

export function admitsGeneration(purpose: SecretPurpose): boolean {
  return isWebhookPurpose(purpose);
}

export function requiresExternalVerification(purpose: SecretPurpose): boolean {
  return (
    purpose === SecretPurpose.PROVIDER_AUTHENTICATION ||
    purpose === SecretPurpose.TOOL_AUTHENTICATION ||
    purpose === SecretPurpose.MCP_AUTHENTICATION ||
    purpose === SecretPurpose.OAUTH_REFRESH
  );
}

export function admitsLease(purpose: SecretPurpose): boolean {
  return (
    purpose === SecretPurpose.PROVIDER_AUTHENTICATION ||
    purpose === SecretPurpose.TOOL_AUTHENTICATION ||
    purpose === SecretPurpose.MCP_AUTHENTICATION
  );
}

export function defaultKind(purpose: SecretPurpose): SecretMaterialKind {
  return compatibleKinds(purpose)[0] ?? SecretMaterialKind.UNSPECIFIED;
}
