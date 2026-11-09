import { VaultLifecycle, type VaultEvent } from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { mintEnvironmentId } from '@/features/environments/domain/environment-id';

import { vaultCommands, type VaultCommand } from './commands';
import type { DecisionContext } from './context';
import { decideVault, foldVault } from './vault';

const stagingEnvironment = mintEnvironmentId('staging');

function context(overrides: Partial<DecisionContext> = {}): DecisionContext {
  let counter = 0;
  return {
    now: new Date('2026-01-01T00:00:00Z'),
    projectId: 'proj-test',
    newId: (prefix) => `${prefix}_${++counter}`,
    vault: () => undefined,
    liveCredentialCount: () => 0,
    ...overrides,
  };
}

function history() {
  const events: VaultEvent[] = [];
  const ctx = context();
  return {
    ctx,
    state: () => foldVault(events),
    run(command: VaultCommand) {
      const current = foldVault(events);
      const decision = decideVault(current, command, ctx);
      if (!decision.ok) throw decision.fault;
      events.push(...decision.events);
      return foldVault(events)!;
    },
  };
}

describe('vault decider', () => {
  const create = () => vaultCommands.create({ vaultId: 'vlt_a', displayName: 'Core', environment: stagingEnvironment });

  it('creates an active vault with an immutable environment', () => {
    const vault = history().run(create());
    expect(vault.lifecycle).toBe(VaultLifecycle.ACTIVE);
    expect(vault.environment).toBe('staging');
  });

  it('rejects an empty display name', () => {
    const decision = decideVault(
      undefined,
      vaultCommands.create({ vaultId: 'vlt_a', displayName: '   ', environment: stagingEnvironment }),
      context(),
    );
    expect(decision.ok).toBe(false);
  });

  it('walks disable, enable, schedule, confirm', () => {
    const vault = history();
    vault.run(create());
    expect(vault.run(vaultCommands.transition('vlt_a', 'DISABLE')).lifecycle).toBe(VaultLifecycle.DISABLED);
    expect(vault.run(vaultCommands.transition('vlt_a', 'ENABLE')).lifecycle).toBe(VaultLifecycle.ACTIVE);
    expect(vault.run(vaultCommands.transition('vlt_a', 'SCHEDULE_DESTRUCTION')).lifecycle).toBe(
      VaultLifecycle.DESTROY_PENDING,
    );
    expect(vault.run(vaultCommands.transition('vlt_a', 'CONFIRM_DESTROYED')).lifecycle).toBe(VaultLifecycle.DESTROYED);
  });

  it('blocks destruction while live credentials exist', () => {
    const vault = history();
    vault.run(create());
    const live = { ...vault.ctx, liveCredentialCount: () => 2 };
    const decision = decideVault(vault.state(), vaultCommands.transition('vlt_a', 'SCHEDULE_DESTRUCTION'), live);
    expect(decision.ok).toBe(false);
    if (!decision.ok) {
      expect(decision.fault.reason).toBe('INVALID_LIFECYCLE');
      expect(decision.fault.message).toContain('2 live credentials');
    }
  });

  it('refuses a transition the lifecycle does not permit', () => {
    const vault = history();
    vault.run(create());
    const decision = decideVault(vault.state(), vaultCommands.transition('vlt_a', 'ENABLE'), vault.ctx);
    expect(decision.ok).toBe(false);
  });
});
