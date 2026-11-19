import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, OperationCancellationError, Sandbox, tenantId } from '../src/index.ts';
import type { CancelOperationResponse, GetOperationResponse, LeaseGranted, LeaseRequest } from '../src/index.ts';

const tenant = tenantId('acme');
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function fixture() {
  const client = new MemorySandboxClient();
  const submission = deferred<LeaseRequest>();
  const granting = deferred<LeaseGranted>();
  const cancellation = deferred<CancelOperationResponse>();
  const requests: LeaseRequest[] = [];
  const cancelled: string[] = [];
  const deleted: string[] = [];
  const controls = { supported: true, capabilityCalls: 0 };
  const grant: LeaseGranted = {
    lease_id: 'lease-known' as LeaseGranted['lease_id'],
    provider: 'e2b',
    sandbox_id: 'provider-known',
    evidence_subject: 'sandbox.evidence.lease-known',
    deadline: { secs_since_epoch: 1_900_000_000, nanos_since_epoch: 0 },
    created: true,
  };
  client.operationCapabilities = async () => {
    controls.capabilityCalls += 1;
    return { supports_create_cancellation: controls.supported };
  };
  client.lease = async (request) => {
    requests.push(request);
    submission.resolve(request);
    return granting.promise;
  };
  client.cancelOperation = async (id) => {
    cancelled.push(id);
    return cancellation.promise;
  };
  client.getOperation = async (id): Promise<GetOperationResponse> => ({
    outcome: 'found',
    operation: {
      id,
      tenant,
      kind: 'create',
      state: 'succeeded',
      target: grant.lease_id,
      result: { result: 'granted', ...grant, operation_id: id },
    },
  });
  client.deleteLease = async (id) => {
    deleted.push(id);
  };
  return { client, controls, grant, submission, granting, cancellation, requests, cancelled, deleted };
}

describe('Sandbox.create operation cancellation', () => {
  it('does not contact capabilities or lease when pre-aborted', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('cancel before create');
    controller.abort(reason);
    await expect(Sandbox.create(f.client, { tenant, signal: controller.signal })).rejects.toBe(reason);
    expect(f.requests).toEqual([]);
    expect(f.controls.capabilityCalls).toBe(0);
  });

  it('fails closed on a broker without create cancellation before requesting a lease', async () => {
    const f = fixture();
    f.controls.supported = false;
    f.granting.resolve(f.grant);
    await expect(Sandbox.create(f.client, { tenant, signal: new AbortController().signal })).rejects.toMatchObject({
      code: 'unsupported',
    });
    expect(f.controls.capabilityCalls).toBe(1);
    expect(f.requests).toEqual([]);
  });

  it('does not dispatch a lease if cancellation wins during capability preflight', async () => {
    const f = fixture();
    const capability = deferred<{ supports_create_cancellation: boolean }>();
    f.client.operationCapabilities = async () => capability.promise;
    const controller = new AbortController();
    const reason = new Error('cancel capability preflight');
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal }).catch((error: unknown) => error);
    controller.abort(reason);
    const observed = await Promise.race([pending, tick().then(() => 'pending')]);
    capability.resolve({ supports_create_cancellation: true });
    f.granting.resolve(f.grant);
    await pending;
    await tick();
    expect(observed).toBe(reason);
    expect(f.requests).toEqual([]);
    expect(f.cancelled).toEqual([]);
  });

  it('records an operation identity before dispatch and waits for confirmed cancellation', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('cancel provisioning');
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal }).catch((error: unknown) => error);
    const request = await f.submission.promise;
    controller.abort(reason);
    const observed = await Promise.race([pending, tick().then(() => 'pending')]);
    f.cancellation.resolve({ outcome: 'canceled' });
    const error = await pending;
    expect(observed).toBe('pending');
    expect(request.operation_id).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
    expect(f.cancelled).toEqual([request.operation_id]);
    expect(error).toBeInstanceOf(OperationCancellationError);
    expect(error).toMatchObject({
      code: 'operation_cancelled',
      operationId: request.operation_id,
      canceled: true,
      cleanupPending: false,
      outcomeUnknown: false,
      cause: reason,
    });
    expect(f.deleted).toEqual([]);
  });

  it('leaves durable pending cancellation observable after its local budget', async () => {
    const f = fixture();
    f.cancellation.resolve({ outcome: 'pending' });
    const controller = new AbortController();
    const reason = new Error('cancel a slow worker');
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal, cancellationTimeoutMs: 20 });
    const request = await f.submission.promise;
    controller.abort(reason);
    await expect(pending).rejects.toMatchObject({
      code: 'operation_cancellation_pending',
      operationId: request.operation_id,
      canceled: false,
      cleanupPending: true,
      outcomeUnknown: true,
      cause: reason,
    });
    expect(f.cancelled.length).toBeGreaterThan(0);
    expect(f.cancelled.every((id) => id === request.operation_id)).toBe(true);
    expect(f.deleted).toEqual([]);
  });

  it.each([true, false])('reports a completed operation and preserves its lease (created=%s)', async (created) => {
    const f = fixture();
    f.grant.created = created;
    f.cancellation.resolve({ outcome: 'already_completed', state: 'succeeded' });
    const controller = new AbortController();
    const reason = new Error('cancel after operation won');
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal });
    const request = await f.submission.promise;
    controller.abort(reason);
    await expect(pending).rejects.toMatchObject({
      code: 'operation_already_completed',
      operationId: request.operation_id,
      canceled: false,
      cleanupPending: false,
      outcomeUnknown: false,
      leaseId: f.grant.lease_id,
      cause: reason,
    });
    expect(f.deleted).toEqual([]);
  });

  it.each(['foreign_id', 'foreign_tenant', 'foreign_grant', 'not_found'] as const)(
    'does not claim completion from %s status',
    async (invalid) => {
      const f = fixture();
      const get = f.client.getOperation.bind(f.client);
      f.client.getOperation = async (id) => {
        const response = await get(id);
        if (invalid === 'not_found') return { outcome: 'not_found' };
        if (response.outcome === 'found') {
          if (invalid === 'foreign_id') response.operation.id = 'foreign-operation';
          if (invalid === 'foreign_tenant') response.operation.tenant = 'foreign';
          if (invalid === 'foreign_grant' && response.operation.result?.result === 'granted')
            response.operation.result.operation_id = 'foreign-operation';
        }
        return response;
      };
      f.cancellation.resolve({ outcome: 'already_completed', state: 'succeeded' });
      const controller = new AbortController();
      const pending = Sandbox.create(f.client, { tenant, signal: controller.signal, cancellationTimeoutMs: 30 });
      await f.submission.promise;
      controller.abort();
      await expect(pending).rejects.toMatchObject({ cleanupPending: true, outcomeUnknown: true });
      expect(f.deleted).toEqual([]);
    },
  );

  it('reports an unknown cancellation receipt without deleting a guessed late grant', async () => {
    const f = fixture();
    f.client.cancelOperation = async () => {
      throw new Error('control connection lost');
    };
    const controller = new AbortController();
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal, cancellationTimeoutMs: 20 });
    await f.submission.promise;
    controller.abort();
    await expect(pending).rejects.toMatchObject({ cleanupPending: true, outcomeUnknown: true });
    f.granting.resolve(f.grant);
    await tick();
    expect(f.deleted).toEqual([]);
  });

  it('asks the broker to compensate a newly-owned create whose reply is still delayed', async () => {
    const f = fixture();
    const opted: Array<{ id: string; compensateCreated?: boolean }> = [];
    f.client.cancelOperation = async (id, opts = {}) => {
      opted.push({ id, ...opts });
      return opts.compensateCreated === true
        ? { outcome: 'canceled' }
        : { outcome: 'already_completed', state: 'succeeded' };
    };
    const controller = new AbortController();
    const reason = new Error('cancel late owned creation');
    const pending = Sandbox.create(f.client, { tenant, signal: controller.signal });
    const request = await f.submission.promise;
    controller.abort(reason);
    await expect(pending).rejects.toMatchObject({
      code: 'operation_cancelled',
      operationId: request.operation_id,
      canceled: true,
      cleanupPending: false,
      cause: reason,
    });
    expect(opted).toEqual([{ id: request.operation_id, compensateCreated: true }]);
    expect(f.deleted).toEqual([]);
  });

  it('does not cancel a returned sandbox when its creation signal aborts later', async () => {
    const f = fixture();
    f.granting.resolve(f.grant);
    const controller = new AbortController();
    const sandbox = await Sandbox.create(f.client, { tenant, signal: controller.signal });
    controller.abort();
    await tick();
    expect(sandbox.sandboxId).toBe(f.grant.lease_id);
    expect(f.cancelled).toEqual([]);
    expect(f.deleted).toEqual([]);
  });
});
