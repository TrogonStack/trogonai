import { timestampDate, type Timestamp } from '@bufbuild/protobuf/wkt';
import {
  KeyBackendAdmission,
  KeyBackendFailureReason,
  KeyBackendLifecycle,
  type KeyBackendAccess,
  type KeyBackendHealth,
  type KeyBackendTrustExpiry,
  KeyProvider,
} from '@trogonai/proto';

export const lifecycleLabels: Record<KeyBackendLifecycle, string> = {
  [KeyBackendLifecycle.UNSPECIFIED]: 'Unknown',
  [KeyBackendLifecycle.PENDING_VALIDATION]: 'Pending validation',
  [KeyBackendLifecycle.ACTIVE]: 'Active',
  [KeyBackendLifecycle.RETIRED]: 'Retired',
};

export const admissionLabels: Record<KeyBackendAdmission, string> = {
  [KeyBackendAdmission.UNSPECIFIED]: 'Unknown',
  [KeyBackendAdmission.ENABLED]: 'Enabled',
  [KeyBackendAdmission.PAUSED]: 'Paused',
};

export const failureReasonLabels: Record<KeyBackendFailureReason, string> = {
  [KeyBackendFailureReason.UNSPECIFIED]: 'Unknown failure',
  [KeyBackendFailureReason.AUTHENTICATION_FAILED]: 'Authentication failed',
  [KeyBackendFailureReason.ACCESS_DENIED]: 'Access denied',
  [KeyBackendFailureReason.TLS_FAILURE]: 'TLS failure',
  [KeyBackendFailureReason.UNREACHABLE]: 'Unreachable',
  [KeyBackendFailureReason.TIMEOUT]: 'Timed out',
  [KeyBackendFailureReason.INVALID_RESPONSE]: 'Invalid response',
  [KeyBackendFailureReason.CAPABILITY_MISSING]: 'Capability missing',
};

export const failureReasonHints: Record<KeyBackendFailureReason, string> = {
  [KeyBackendFailureReason.UNSPECIFIED]: 'The platform could not classify the failure.',
  [KeyBackendFailureReason.AUTHENTICATION_FAILED]:
    'The provider rejected the platform identity. Check the role, trust policy or auth role you registered.',
  [KeyBackendFailureReason.ACCESS_DENIED]:
    'The platform authenticated but the provider denied the request. Check the permissions granted to it.',
  [KeyBackendFailureReason.TLS_FAILURE]:
    'The TLS handshake did not verify against the trust root you registered. Check the certificate chain and its expiry.',
  [KeyBackendFailureReason.UNREACHABLE]: 'The platform could not open a connection to the endpoint.',
  [KeyBackendFailureReason.TIMEOUT]: 'The provider did not answer within the deadline.',
  [KeyBackendFailureReason.INVALID_RESPONSE]: 'The provider answered with something the platform could not interpret.',
  [KeyBackendFailureReason.CAPABILITY_MISSING]:
    'The provider is missing a capability the platform requires, for example a Transit setting.',
};

export type ConnectionStatus =
  | { readonly kind: 'unknown' }
  | { readonly kind: 'connected'; readonly observedAt: Date }
  | { readonly kind: 'failed'; readonly reason: KeyBackendFailureReason; readonly observedAt: Date };

export function connectionStatusOf(health: KeyBackendHealth | undefined): ConnectionStatus {
  const observedAt = health?.observedAt ? timestampDate(health.observedAt) : undefined;
  switch (health?.status.case) {
    case 'connected':
      return observedAt ? { kind: 'connected', observedAt } : { kind: 'unknown' };
    case 'failed':
      return observedAt ? { kind: 'failed', reason: health.status.value.reason, observedAt } : { kind: 'unknown' };
    default:
      return { kind: 'unknown' };
  }
}

export type ExpirySignal =
  | 'server_certificate'
  | 'client_certificate'
  | 'trust_root'
  | 'credential_expiry'
  | 'credential_issued';

export const expirySignalLabels: Record<ExpirySignal, string> = {
  server_certificate: 'Endpoint server certificate',
  client_certificate: 'Client certificate',
  trust_root: 'Trust root',
  credential_expiry: 'Credential',
  credential_issued: 'Credential age',
};

export type ExpirySeverity = 'expired' | 'soon' | 'later';

export interface ExpiryWarning {
  readonly signal: ExpirySignal;
  readonly at: Date;
  readonly severity: ExpirySeverity;
  readonly daysRemaining: number;
}

export const EXPIRY_WARNING_DAYS = 30;
export const CREDENTIAL_AGE_WARNING_DAYS = 90;
const DAY_MS = 86_400_000;

export function expiryWarnings(expiry: KeyBackendTrustExpiry | undefined, now: Date): readonly ExpiryWarning[] {
  if (!expiry) return [];
  const warnings: ExpiryWarning[] = [];
  const consider = (signal: ExpirySignal, value: Timestamp | undefined) => {
    if (!value) return;
    const at = timestampDate(value);
    const daysRemaining = Math.floor((at.getTime() - now.getTime()) / DAY_MS);
    if (daysRemaining < 0) warnings.push({ signal, at, severity: 'expired', daysRemaining });
    else if (daysRemaining <= EXPIRY_WARNING_DAYS) warnings.push({ signal, at, severity: 'soon', daysRemaining });
  };
  consider('server_certificate', expiry.serverCertificateNotAfter);
  consider('client_certificate', expiry.clientCertificateNotAfter);
  consider('trust_root', expiry.trustRootNotAfter);
  consider('credential_expiry', expiry.credentialExpiresAt);
  if (expiry.credentialIssuedAt) {
    const issued = timestampDate(expiry.credentialIssuedAt);
    const age = Math.floor((now.getTime() - issued.getTime()) / DAY_MS);
    if (age >= CREDENTIAL_AGE_WARNING_DAYS) {
      warnings.push({ signal: 'credential_issued', at: issued, severity: 'soon', daysRemaining: -age });
    }
  }
  return [...warnings].sort((a, b) => a.at.getTime() - b.at.getTime());
}

export interface RevisionNote {
  readonly revision: number;
  readonly at: Date;
  readonly summary: string;
}

export function providerOfAccess(access: KeyBackendAccess | undefined): KeyProvider {
  switch (access?.provider.case) {
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

export function accessSummary(access: KeyBackendAccess | undefined): string {
  switch (access?.provider.case) {
    case 'awsKms': {
      const aws = access.provider.value;
      return `STS AssumeRole into account ${aws.account?.value ?? 'unknown'} (${aws.region?.value ?? 'unknown'})`;
    }
    case 'googleCloudKms':
      return `Workload Identity Federation into project ${access.provider.value.project?.value ?? 'unknown'}`;
    case 'openBaoTransit': {
      const bao = access.provider.value;
      const method = bao.authentication?.method.case === 'certificate' ? 'Certificate' : 'JWT';
      return `${method} authentication to ${hostOf(bao.endpoint?.baseUrl ?? '')}`;
    }
    default:
      return 'No access recorded';
  }
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}
