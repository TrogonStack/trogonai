import { describe, expect, it } from 'vitest';

import { CODE_CLASSES, DRIVE_ERROR_CLASSES, FILE_OP_ERROR_CLASSES, READ_ONLY_TOOLS } from '../src/error-classes.ts';
import {
  LeaseDeniedError,
  NameConflictError,
  LEASE_ERROR_CLASSES,
  LEASE_ERROR_KINDS,
  SandboxWsError,
  TimeoutError,
  TransportError,
  errorFromCode,
} from '../src/index.ts';

describe('error classification tables', () => {
  it('classifies every lease error kind with a hint', () => {
    expect(Object.keys(LEASE_ERROR_CLASSES).sort()).toEqual([...LEASE_ERROR_KINDS].sort());
    for (const kind of LEASE_ERROR_KINDS) {
      const err = errorFromCode('lease_denied', 'denied', { kind });
      expect(err).toBeInstanceOf(LeaseDeniedError);
      expect(err.retryable, kind).toBe(LEASE_ERROR_CLASSES[kind].retryable);
      expect(err.outcomeUnknown, kind).toBe(false);
      expect(err.hint.length, kind).toBeGreaterThan(0);
      expect(err.variant).toBe(kind);
    }
  });

  it('surfaces a name conflict as a typed lease denial', () => {
    const err = errorFromCode('lease_denied', 'named lease is being created', { kind: 'name_conflict' });
    expect(err).toBeInstanceOf(NameConflictError);
    expect(err).toBeInstanceOf(LeaseDeniedError);
    expect(err.retryable).toBe(true);
  });

  it('marks only transient lease errors retryable', () => {
    const retryable = LEASE_ERROR_KINDS.filter((k) => LEASE_ERROR_CLASSES[k].retryable).sort();
    expect(retryable).toEqual(
      [
        'backend',
        'checkpoint_running_failed',
        'entity_unavailable',
        'image_not_ready',
        'lease_waking',
        'name_conflict',
        'no_provider_available',
        'no_worker_in_region',
        'provider_launch_failed',
        'provider_rejected_dispatch',
      ].sort(),
    );
  });

  it('classifies drive and file operation kinds', () => {
    for (const [kind, cls] of Object.entries(DRIVE_ERROR_CLASSES)) {
      const err = errorFromCode('drive_denied', 'm', { kind });
      expect(err.retryable, kind).toBe(cls.retryable);
    }
    expect(DRIVE_ERROR_CLASSES.storage_backend?.retryable).toBe(true);
    for (const [kind, cls] of Object.entries(FILE_OP_ERROR_CLASSES)) {
      const err = errorFromCode('file_op_failed', 'm', { kind });
      expect(err.retryable, kind).toBe(cls.retryable);
    }
    expect(FILE_OP_ERROR_CLASSES.io?.retryable).toBe(true);
  });

  it('gives every gateway code a classification and defaults unknown codes to permanent', () => {
    for (const code of Object.keys(CODE_CLASSES)) {
      expect(errorFromCode(code, 'm').hint.length, code).toBeGreaterThan(0);
    }
    const unknown = errorFromCode('brand_new_code', 'm');
    expect(unknown.retryable).toBe(false);
    expect(unknown.outcomeUnknown).toBe(false);
  });
});

describe('errorFromCode', () => {
  it('prefers fields sent by the gateway over the local tables', () => {
    const err = errorFromCode('lease_denied', 'm', {
      kind: 'invalid_spec',
      retryable: true,
      hint: 'from gateway',
      provider: 'local_docker',
      lease_id: 'lease-1',
    });
    expect(err.retryable).toBe(true);
    expect(err.hint).toBe('from gateway');
    expect(err.provider).toBe('local_docker');
    expect(err.leaseId).toBe('lease-1');
  });

  it('fills leaseId from the request when the wire error names none', () => {
    const err = errorFromCode('not_found', 'm', {}, { tool: 'sandbox_read_file', leaseId: 'lease-2' });
    expect(err.leaseId).toBe('lease-2');
  });

  it('keeps existing codes and classes stable', () => {
    expect(errorFromCode('timeout', 'm')).toBeInstanceOf(TimeoutError);
    expect(errorFromCode('transport', 'm')).toBeInstanceOf(TransportError);
    expect(errorFromCode('anything', 'm')).toBeInstanceOf(SandboxWsError);
    expect(errorFromCode('lease_denied', 'm').code).toBe('lease_denied');
  });

  it('reports an unknown outcome for a timeout on a mutating verb only', () => {
    const write = errorFromCode('timeout', 'm', {}, { tool: 'sandbox_write_files' });
    expect(write.outcomeUnknown).toBe(true);
    expect(write.retryable).toBe(true);
    const lease = errorFromCode('timeout', 'm', {}, { tool: 'sandbox_lease' });
    expect(lease.outcomeUnknown).toBe(true);
    const read = errorFromCode('timeout', 'm', {}, { tool: 'sandbox_read_file' });
    expect(read.outcomeUnknown).toBe(false);
    expect(read.retryable).toBe(true);
    for (const tool of READ_ONLY_TOOLS) {
      expect(errorFromCode('transport', 'm', {}, { tool }).outcomeUnknown, tool).toBe(false);
    }
  });

  it('carries a scrubbed copy of the cause', () => {
    const cause = new Error('socket reset');
    const err = new TransportError('send failed', { cause });
    expect(err.cause).toBeInstanceOf(Error);
    expect(err.cause).not.toBe(cause);
    expect((err.cause as Error).message).toBe('socket reset');
  });
});
