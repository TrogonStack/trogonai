import { describe, expect, expectTypeOf, it } from 'vitest';

import { SandboxCapabilities, SANDBOX_FEATURES } from '../src/capabilities.ts';
import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { Sandbox } from '../src/sandbox.ts';
import type { SandboxWith } from '../src/sandbox.ts';

describe('SandboxCapabilities', () => {
  it('reports membership and the known features', () => {
    const caps = SandboxCapabilities.from(['fork', 'write_stdin', 'future_feature']);
    expect(caps.has('fork')).toBe(true);
    expect(caps.has('drives')).toBe(false);
    expect(caps.features.sort()).toEqual(['fork', 'write_stdin']);
    expect(caps.raw.has('future_feature')).toBe(true);
  });

  it('require throws a non-retryable unsupported error naming the feature', () => {
    const caps: SandboxCapabilities = SandboxCapabilities.from(['fork']);
    expect(() => caps.require('fork')).not.toThrow();
    try {
      caps.require('drives');
      expect.unreachable();
    } catch (err) {
      expect(err).toMatchObject({ code: 'unsupported', retryable: false });
      expect((err as Error).message).toContain('drives');
    }
  });

  it('lists distinct feature names', () => {
    expect(new Set(SANDBOX_FEATURES).size).toBe(SANDBOX_FEATURES.length);
  });

  it('knows every name the broker can stamp on a grant', () => {
    // Mirrors SDK_FEATURE_NAMES in sandbox-core wire.rs plus the broker's read_logs.
    const brokerNames = [
      'pause_resume',
      'write_stdin',
      'restart_command',
      'users',
      'drives',
      'snapshots',
      'fork',
      'update_network_policy',
      'sandbox_domain',
      'port_preview',
      'expose_after_create',
      'groups',
      'change_tracking',
      'file_watch',
      'dir_transfer',
      'file_ops',
      'file_search',
      'file_archive',
      'terminal',
      'named_processes',
      'graceful_stop',
      'persistent_runtime',
      'browser',
      'output_streaming',
      'usage_gauges',
      'native_auto_stop',
      'native_passthrough',
      'read_logs',
    ];
    const known: ReadonlySet<string> = new Set(SANDBOX_FEATURES);
    expect(brokerNames.filter((name) => !known.has(name))).toEqual([]);
  });
});

describe('Sandbox capabilities', () => {
  it('is populated from the lease grant when the broker reports it', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    expect(sandbox.capabilities?.features).toEqual([
      'pause_resume',
      'named_processes',
      'restart_command',
      'snapshots',
      'change_tracking',
      'file_watch',
      'dir_transfer',
      'graceful_stop',
      'file_search',
    ]);
    expect(sandbox.supports('pause_resume')).toBe(true);
    expect(sandbox.supports('fork')).toBe(false);
  });

  it('is undefined and supports() is false when the broker reports nothing', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const attached = await Sandbox.connect(client, sandbox.sandboxId);
    expect(attached.capabilities).toBeUndefined();
    expect(attached.supports('pause_resume')).toBe(false);
  });

  it('withCapabilities sets them for handles that lack them', async () => {
    const client = new MemorySandboxClient();
    const created = await Sandbox.create(client, { tenant: tenantId('acme') });
    const attached = (await Sandbox.connect(client, created.sandboxId)).withCapabilities(
      SandboxCapabilities.from(['fork']),
    );
    expect(attached.supports('fork')).toBe(true);
  });

  it('supports narrows the handle type', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    if (sandbox.supports('fork')) {
      expectTypeOf(sandbox).toExtend<SandboxWith<'fork'>>();
      expectTypeOf(sandbox.capabilities).not.toBeUndefined();
    }
    expectTypeOf(sandbox.capabilities).toEqualTypeOf<SandboxCapabilities | undefined>();
  });
});
