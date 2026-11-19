import { describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { LeaseGranted } from '../src/index.ts';

const tenant = tenantId('acme');
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function initializationClient(created: LeaseGranted['created']) {
  const client = new MemorySandboxClient();
  const lease = client.lease.bind(client);
  let granted: LeaseGranted | undefined;
  client.lease = async (request) => {
    granted = { ...(await lease(request)), created };
    return granted;
  };
  client.operationCapabilities = async () => ({ supports_create_cancellation: true });
  client.cancelOperation = async () => ({ outcome: 'already_completed', state: 'succeeded' });
  client.getOperation = async (id) => {
    await tick();
    return {
      outcome: 'found',
      operation: {
        id,
        tenant,
        kind: 'create',
        state: 'succeeded',
        target: granted!.lease_id,
        result: { result: 'granted', ...granted!, operation_id: id },
      },
    };
  };
  const stopped = vi.spyOn(client, 'stopLease');
  const deleted = vi.spyOn(client, 'deleteLease');
  return { client, stopped, deleted, grant: () => granted! };
}

describe('creation initialization ownership', () => {
  it('observes cancellation triggered synchronously by lease submission', async () => {
    const state = initializationClient(true);
    const lease = state.client.lease.bind(state.client);
    const controller = new AbortController();
    const reason = new Error('cancel on submission');
    state.client.lease = async (request) => {
      controller.abort(reason);
      return lease(request);
    };
    await expect(Sandbox.create(state.client, { tenant, signal: controller.signal })).rejects.toMatchObject({
      code: 'operation_already_completed',
      canceled: false,
      cleanupPending: false,
      cause: reason,
    });
    await tick();
    expect(state.deleted).not.toHaveBeenCalled();
  });

  it.each([false, undefined])(
    'preserves a reused or unknown-ownership grant (%s) when baseline initialization fails',
    async (created) => {
      const state = initializationClient(created);
      const failure = new Error('baseline unavailable');
      state.client.checkpointFiles = async () => {
        throw failure;
      };
      await expect(Sandbox.create(state.client, { tenant, trackChanges: true })).rejects.toBe(failure);
      expect(state.stopped).not.toHaveBeenCalled();
      expect(state.deleted).not.toHaveBeenCalled();
      expect((await Sandbox.connect(state.client, state.grant().lease_id)).sandboxId).toBe(state.grant().lease_id);
    },
  );

  it('deletes only a newly-created grant when baseline initialization fails', async () => {
    const state = initializationClient(true);
    const failure = new Error('baseline unavailable');
    state.client.checkpointFiles = async () => {
      throw failure;
    };
    await expect(Sandbox.create(state.client, { tenant, trackChanges: true })).rejects.toBe(failure);
    expect(state.deleted).toHaveBeenCalledExactlyOnceWith(state.grant().lease_id);
  });

  it('preserves the initialization error when owned-grant cleanup also fails', async () => {
    const state = initializationClient(true);
    const failure = new Error('baseline unavailable');
    state.client.checkpointFiles = async () => {
      throw failure;
    };
    state.deleted.mockRejectedValue(new Error('cleanup unavailable'));
    await expect(Sandbox.create(state.client, { tenant, trackChanges: true })).rejects.toBe(failure);
    expect(state.deleted).toHaveBeenCalledExactlyOnceWith(state.grant().lease_id);
  });

  it.each([true, false, undefined])(
    'reports an already-completed create while baseline initialization is pending (%s)',
    async (created) => {
      const state = initializationClient(created);
      const entered = deferred<void>();
      const checkpoint = deferred<{ checkpointId: string; entries: number }>();
      state.client.checkpointFiles = async () => {
        entered.resolve();
        return checkpoint.promise;
      };
      const controller = new AbortController();
      const reason = new Error('cancel initialization');
      const pending = Sandbox.create(state.client, { tenant, trackChanges: true, signal: controller.signal }).then(
        () => ({ outcome: 'created' as const }),
        (error: unknown) => ({ outcome: 'aborted' as const, error }),
      );
      await entered.promise;
      controller.abort(reason);
      await tick();
      checkpoint.resolve({ checkpointId: 'late-baseline', entries: 0 });
      const completed = await pending;
      expect(completed).toMatchObject({
        outcome: 'aborted',
        error: {
          code: 'operation_already_completed',
          canceled: false,
          cleanupPending: false,
          leaseId: state.grant().lease_id,
          cause: reason,
        },
      });
      expect(state.deleted).not.toHaveBeenCalled();
      expect(state.stopped).not.toHaveBeenCalled();
    },
  );
});
