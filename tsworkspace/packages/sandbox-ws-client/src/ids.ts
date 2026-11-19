export type LeaseId = string & { readonly __brand: 'LeaseId' };
export type TenantId = string & { readonly __brand: 'TenantId' };
export type Digest = string & { readonly __brand: 'Digest' };
export type CorrelationId = string & { readonly __brand: 'CorrelationId' };
export type PullSecretId = string & { readonly __brand: 'PullSecretId' };
export type ServiceAccountId = string & { readonly __brand: 'ServiceAccountId' };
export type ExecutionId = string & { readonly __brand: 'ExecutionId' };
export type StdinSeq = number & { readonly __brand: 'StdinSeq' };
export type LspRequestId = number & { readonly __brand: 'LspRequestId' };

const DIGEST_FORMAT = /^sha256:[0-9a-f]{64}$/;

export function leaseId(s: string): LeaseId {
  return s as LeaseId;
}

export function tenantId(s: string): TenantId {
  return s as TenantId;
}

export function digest(s: string): Digest {
  if (!DIGEST_FORMAT.test(s)) {
    throw new Error(`Invalid digest format: expected "sha256:<64 lowercase hex chars>", got "${s}"`);
  }
  return s as Digest;
}

export function correlationId(s: string): CorrelationId {
  return s as CorrelationId;
}

export function pullSecretId(s: string): PullSecretId {
  return s as PullSecretId;
}

export function serviceAccountId(s: string): ServiceAccountId {
  return s as ServiceAccountId;
}

export function executionId(s: string): ExecutionId {
  return s as ExecutionId;
}

export function stdinSeq(n: number): StdinSeq {
  return n as StdinSeq;
}

export function lspRequestId(n: number): LspRequestId {
  return n as LspRequestId;
}
