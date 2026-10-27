import { toJsonString } from '@bufbuild/protobuf';
import {
  CredentialEventSchema,
  CredentialLifecycle,
  MaterialVerificationOutcome,
  SecretMaterialKind,
  SecretPurpose,
  SecretVersionState,
  VaultLifecycle,
  CredentialPolicyReason,
} from '@trogonai/proto';
import { describe, expect, it } from 'vitest';

import { mintEnvironmentId } from '@/features/environments/domain/environment-id';

import { credentialCommands, vaultCommands, type MaterialInput } from '../domain/commands';
import { SecretFault } from '../domain/faults';
import { compatibleKinds, selectablePurposes } from '../domain/policy';
import { toCredentialView, toVaultView } from '../domain/view';
import { createFakeSecretsClient } from './index';
import { seedIds } from './seed';

const HOUR = 3_600_000;
const stagingEnvironment = mintEnvironmentId('staging');

function newClient(start = new Date('2026-03-01T12:00:00Z')) {
  let now = start;
  const client = createFakeSecretsClient({ seeded: false, latencyMs: 0, clock: () => now });
  return {
    client,
    advance: (milliseconds: number) => {
      now = new Date(now.getTime() + milliseconds);
    },
  };
}

async function withVault() {
  const context = newClient();
  const vault = await context.client.dispatchVault(
    vaultCommands.create({ vaultId: 'vlt_test', displayName: 'Test', environment: stagingEnvironment }),
  );
  return { ...context, vault };
}

const imported = (material: string): MaterialInput => ({ case: 'imported', material });
const generated: MaterialInput = { case: 'generated' };

async function rejection(promise: Promise<unknown>): Promise<SecretFault> {
  try {
    await promise;
  } catch (error) {
    if (error instanceof SecretFault) return error;
    throw error;
  }
  throw new Error('The command was accepted.');
}

describe('purpose and kind matrix', () => {
  const kinds = [
    SecretMaterialKind.API_TOKEN,
    SecretMaterialKind.BASIC_AUTHENTICATION,
    SecretMaterialKind.OAUTH_ACCESS_TOKEN,
    SecretMaterialKind.OAUTH_REFRESH_TOKEN,
    SecretMaterialKind.WEBHOOK_SHARED_SECRET,
    SecretMaterialKind.WEBHOOK_DESTINATION,
  ];

  it('accepts exactly the compatible kinds and rejects the rest', async () => {
    const { client, vault } = await withVault();
    for (const purpose of selectablePurposes) {
      for (const kind of kinds) {
        const compatible = compatibleKinds(purpose).includes(kind);
        const attempt = client.dispatchCredential(
          credentialCommands.create({
            vaultId: vault.vaultId,
            displayName: 'matrix',
            purpose,
            kind,
            material: imported(
              kind === SecretMaterialKind.WEBHOOK_DESTINATION
                ? 'https://example.test/hook'
                : 'user:matrix-password-value',
            ),
          }),
        );
        if (compatible) {
          await expect(attempt).resolves.toBeDefined();
        } else {
          expect((await rejection(attempt)).reason).toBe('PURPOSE_MISMATCH');
        }
      }
    }
  });

  it('refuses to generate material for a non webhook purpose', async () => {
    const { client, vault } = await withVault();
    const fault = await rejection(
      client.dispatchCredential(
        credentialCommands.create({
          vaultId: vault.vaultId,
          displayName: 'bad',
          purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
          kind: SecretMaterialKind.API_TOKEN,
          material: generated,
        }),
      ),
    );
    expect(fault.reason).toBe('PURPOSE_MISMATCH');
  });
});

describe('generated material', () => {
  it('is revealed once on creation and never on replay or read', async () => {
    const { client, vault } = await withVault();
    const command = credentialCommands.create({
      vaultId: vault.vaultId,
      displayName: 'Hook',
      purpose: SecretPurpose.WEBHOOK_VERIFICATION,
      kind: SecretMaterialKind.WEBHOOK_SHARED_SECRET,
      material: generated,
    });
    const first = await client.dispatchCredential(command, { commandId: 'cmd-1' });
    expect(first.revealed?.value).toMatch(/^whsec_/);
    expect(first.credential.lifecycle).toBe(CredentialLifecycle.ACTIVE);

    const replay = await client.dispatchCredential(command, { commandId: 'cmd-1' });
    expect(replay.revealed).toBeUndefined();
    const read = await client.getCredential(first.credential.secret);
    expect(JSON.stringify(read)).not.toContain(first.revealed!.value);
    const history = await client.credentialHistory(first.credential.secret);
    for (const stored of history) {
      expect(toJsonString(CredentialEventSchema, stored.event)).not.toContain(first.revealed!.value);
    }
  });

  it('is not reported for an imported credential', async () => {
    const { client, vault } = await withVault();
    const result = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretMaterialKind.API_TOKEN,
        material: imported('sk-super-private-value'),
      }),
    );
    expect(result.revealed).toBeUndefined();
  });
});

describe('material never persists', () => {
  it('keeps submitted strings out of every stored event', async () => {
    const { client, vault } = await withVault();
    const first = 'sk-first-private-material-1';
    const second = 'sk-second-private-material-2';
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretMaterialKind.API_TOKEN,
        material: imported(first),
      }),
    );
    await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: imported(second) }),
      { expectedRevision: created.credential.revision },
    );
    const history = await client.credentialHistory(created.credential.secret);
    const serialized = history.map((stored) => toJsonString(CredentialEventSchema, stored.event)).join('\n');
    expect(serialized.length).toBeGreaterThan(0);
    expect(serialized).not.toContain(first);
    expect(serialized).not.toContain(second);
    expect(JSON.stringify(await client.getCredential(created.credential.secret))).not.toContain(first);
  });
});

describe('rotation', () => {
  it('keeps the SecretRef and bumps the version', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretMaterialKind.API_TOKEN,
        material: imported('sk-initial-material'),
      }),
    );
    expect(created.credential.activeVersion).toBe(1);
    const rotated = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: imported('sk-rotated-material') }),
      { expectedRevision: created.credential.revision },
    );
    expect(rotated.credential.secret).toBe(created.credential.secret);
    expect(rotated.credential.activeVersion).toBe(2);
    expect(rotated.credential.versions.find((entry) => entry.version === 1)?.state).toBe(
      SecretVersionState.SOFT_DELETED,
    );
  });

  it('keeps the prior version active when verification fails', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretMaterialKind.API_TOKEN,
        material: imported('sk-initial-material'),
      }),
    );
    const failed = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, {
        material: imported('sk-rejected-material'),
        simulation: MaterialVerificationOutcome.AUTHENTICATION_REJECTED,
      }),
      { expectedRevision: created.credential.revision },
    );
    expect(failed.credential.activeVersion).toBe(1);
    expect(failed.credential.lifecycle).toBe(CredentialLifecycle.ACTIVE);
    expect(failed.credential.write?.status).toBe('REJECTED');
    const view = toCredentialView(failed.credential);
    expect(view.saga?.steps.some((step) => step.status === 'failed')).toBe(true);
    expect(view.actions.rotate.available).toBe(true);
  });

  it('aborts provisioning when the first write fails verification', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-rejected-material'),
        simulation: MaterialVerificationOutcome.AUTHENTICATION_REJECTED,
      }),
    );
    expect(created.credential.lifecycle).toBe(CredentialLifecycle.PROVISIONING_ABORTED);
  });

  it('holds a pending write until verification completes', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-pending-material'),
        simulation: 'PENDING',
      }),
    );
    expect(created.credential.lifecycle).toBe(CredentialLifecycle.PROVISIONING);
    const blocked = await rejection(
      client.dispatchCredential(
        credentialCommands.rotate(created.credential.secret, { material: imported('sk-other-material') }),
        { expectedRevision: created.credential.revision },
      ),
    );
    expect(blocked.reason).toBe('INVALID_LIFECYCLE');
    const done = await client.dispatchCredential(
      credentialCommands.completeVerification(created.credential.secret, MaterialVerificationOutcome.PASSED),
      { expectedRevision: created.credential.revision },
    );
    expect(done.credential.lifecycle).toBe(CredentialLifecycle.ACTIVE);
    expect(done.credential.activeVersion).toBe(1);
  });
});

const SecretKindApi = SecretMaterialKind.API_TOKEN;

describe('webhook overlap', () => {
  async function webhook() {
    const context = await withVault();
    const created = await context.client.dispatchCredential(
      credentialCommands.create({
        vaultId: context.vault.vaultId,
        displayName: 'Hook',
        purpose: SecretPurpose.WEBHOOK_VERIFICATION,
        kind: SecretMaterialKind.WEBHOOK_SHARED_SECRET,
        material: generated,
      }),
    );
    return { ...context, created };
  }

  it('opens an overlap window for the previous version', async () => {
    const { client, created } = await webhook();
    const rotated = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: generated, overlapHours: 6 }),
      { expectedRevision: created.credential.revision },
    );
    expect(rotated.credential.overlap?.version).toBe(1);
    expect(rotated.revealed?.version).toBe(2);
    const view = toCredentialView(rotated.credential);
    expect(view.versions.find((entry) => entry.version === 1)?.role).toBe('OVERLAP');
    expect(view.actions.rotate.available).toBe(false);
  });

  it('rejects an overlap longer than 24 hours', async () => {
    const { client, created } = await webhook();
    const fault = await rejection(
      client.dispatchCredential(
        credentialCommands.rotate(created.credential.secret, { material: generated, overlapHours: 25 }),
        { expectedRevision: created.credential.revision },
      ),
    );
    expect(fault.reason).toBe('COMMAND_INVALID');
  });

  it('accepts exactly 24 hours', async () => {
    const { client, created } = await webhook();
    const rotated = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: generated, overlapHours: 24 }),
      { expectedRevision: created.credential.revision },
    );
    expect(rotated.credential.overlap).toBeDefined();
  });

  it('refuses an overlap for a non webhook purpose', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-initial-material'),
      }),
    );
    const fault = await rejection(
      client.dispatchCredential(
        credentialCommands.rotate(created.credential.secret, {
          material: imported('sk-rotated-material'),
          overlapHours: 2,
        }),
        { expectedRevision: created.credential.revision },
      ),
    );
    expect(fault.reason).toBe('PURPOSE_MISMATCH');
  });

  it('ends the overlap when it expires', async () => {
    const { client, advance, created } = await webhook();
    const rotated = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: generated, overlapHours: 2 }),
      { expectedRevision: created.credential.revision },
    );
    expect(rotated.credential.overlap).toBeDefined();
    advance(3 * HOUR);
    const after = await client.getCredential(created.credential.secret);
    expect(after?.overlap).toBeUndefined();
  });

  it('ends the overlap on revocation', async () => {
    const { client, created } = await webhook();
    const rotated = await client.dispatchCredential(
      credentialCommands.rotate(created.credential.secret, { material: generated, overlapHours: 6 }),
      { expectedRevision: created.credential.revision },
    );
    const revoked = await client.dispatchCredential(
      credentialCommands.revoke(created.credential.secret, CredentialPolicyReason.COMPROMISE),
      { expectedRevision: rotated.credential.revision },
    );
    expect(revoked.credential.overlap).toBeUndefined();
    expect(revoked.credential.lifecycle).toBe(CredentialLifecycle.REVOKED);
  });
});

describe('revocation', () => {
  it('is irreversible', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-initial-material'),
      }),
    );
    const revoked = await client.dispatchCredential(
      credentialCommands.revoke(created.credential.secret, CredentialPolicyReason.COMPROMISE),
      { expectedRevision: created.credential.revision },
    );
    expect(revoked.credential.lifecycle).toBe(CredentialLifecycle.REVOKED);
    const enable = await rejection(
      client.dispatchCredential(credentialCommands.enable(created.credential.secret), {
        expectedRevision: revoked.credential.revision,
      }),
    );
    expect(enable.reason).toBe('INVALID_LIFECYCLE');
    const rotate = await rejection(
      client.dispatchCredential(
        credentialCommands.rotate(created.credential.secret, { material: imported('sk-after-revoke') }),
        { expectedRevision: revoked.credential.revision },
      ),
    );
    expect(rotate.reason).toBe('INVALID_LIFECYCLE');
    const view = toCredentialView(revoked.credential);
    expect(view.actions.enable.available).toBe(false);
    expect(view.actions.revoke.available).toBe(false);
  });
});

describe('concurrency', () => {
  it('rejects a stale revision', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-initial-material'),
      }),
    );
    const fault = await rejection(
      client.dispatchCredential(credentialCommands.disable(created.credential.secret), {
        expectedRevision: created.credential.revision - 1,
      }),
    );
    expect(fault.reason).toBe('REVISION_CONFLICT');
  });
});

describe('vault with credentials', () => {
  it('blocks destruction while credentials are live and allows it afterwards', async () => {
    const { client, vault } = await withVault();
    const created = await client.dispatchCredential(
      credentialCommands.create({
        vaultId: vault.vaultId,
        displayName: 'Key',
        purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
        kind: SecretKindApi,
        material: imported('sk-initial-material'),
      }),
    );
    const blocked = await rejection(
      client.dispatchVault(vaultCommands.transition(vault.vaultId, 'SCHEDULE_DESTRUCTION'), {
        expectedRevision: vault.revision,
      }),
    );
    expect(blocked.reason).toBe('INVALID_LIFECYCLE');
    const view = toVaultView((await client.getVault(vault.vaultId))!, await client.listCredentials());
    expect(view.actions.scheduleDestruction.available).toBe(false);
    expect(view.actions.scheduleDestruction.reason).toContain('1 live credential');

    const revoked = await client.dispatchCredential(
      credentialCommands.revoke(created.credential.secret, CredentialPolicyReason.OPERATOR_REQUESTED),
      { expectedRevision: created.credential.revision },
    );
    const scheduled = await client.dispatchCredential(
      credentialCommands.scheduleDestruction(created.credential.secret, new Date(0)),
      { expectedRevision: revoked.credential.revision },
    );
    const destroyed = await client.dispatchCredential(
      credentialCommands.confirmDestruction(created.credential.secret),
      { expectedRevision: scheduled.credential.revision },
    );
    expect(destroyed.credential.lifecycle).toBe(CredentialLifecycle.DESTROYED);
    const vaultNow = await client.dispatchVault(vaultCommands.transition(vault.vaultId, 'SCHEDULE_DESTRUCTION'), {
      expectedRevision: vault.revision,
    });
    expect(vaultNow.lifecycle).toBe(VaultLifecycle.DESTROY_PENDING);
  });

  it('takes no new credential once the vault is disabled', async () => {
    const { client, vault } = await withVault();
    await client.dispatchVault(vaultCommands.transition(vault.vaultId, 'DISABLE'), {
      expectedRevision: vault.revision,
    });
    const fault = await rejection(
      client.dispatchCredential(
        credentialCommands.create({
          vaultId: vault.vaultId,
          displayName: 'Key',
          purpose: SecretPurpose.PROVIDER_AUTHENTICATION,
          kind: SecretKindApi,
          material: imported('sk-initial-material'),
        }),
      ),
    );
    expect(fault.reason).toBe('INVALID_LIFECYCLE');
  });
});

describe('seed', () => {
  it('provides vaults and credentials across the lifecycle', async () => {
    const client = createFakeSecretsClient({ latencyMs: 0 });
    const vaults = await client.listVaults();
    const credentials = await client.listCredentials();
    expect(vaults).toHaveLength(3);
    expect(credentials).toHaveLength(8);
    const lifecycles = new Set(credentials.map((credential) => credential.lifecycle));
    for (const lifecycle of [
      CredentialLifecycle.ACTIVE,
      CredentialLifecycle.DISABLED,
      CredentialLifecycle.DESTROY_PENDING,
      CredentialLifecycle.DESTROYED,
      CredentialLifecycle.RECONNECT_REQUIRED,
      CredentialLifecycle.PROVISIONING,
    ]) {
      expect(lifecycles.has(lifecycle)).toBe(true);
    }
    const github = await client.getCredential(seedIds.credentials.github);
    expect(github?.overlap).toBeDefined();
    const openai = await client.getCredential(seedIds.credentials.openai);
    expect(openai?.activeVersion).toBe(3);
    expect(openai?.uses.length).toBeGreaterThan(3);
    expect(openai?.leases.length).toBe(2);
  });
});
