import { describe, expect, it } from 'vitest';

import { createFakeBackend } from './fake-backend';
import { environmentCommands } from './features/environments/domain/commands';
import { mintEnvironmentId } from './features/environments/domain/environment-id';
import { seedProjectIds } from './features/projects/fake/seed';
import { vaultCommands } from './features/secrets/domain/commands';

const demo = seedProjectIds.demo;

const lab = mintEnvironmentId('lab');

describe('fake backend composition', () => {
  it('rejects a vault in an unregistered environment', async () => {
    const secrets = createFakeBackend().secrets.forProject(demo);
    await expect(
      secrets.dispatchVault(
        vaultCommands.create({ vaultId: 'vlt_x', displayName: 'Lost', environment: mintEnvironmentId('moon') }),
      ),
    ).rejects.toMatchObject({ reason: 'ENVIRONMENT_NOT_FOUND' });
  });

  it('accepts a vault in a registered custom environment', async () => {
    const backend = createFakeBackend();
    const secrets = backend.secrets.forProject(demo);
    const environments = backend.environments.forProject(demo);
    await environments.dispatch(environmentCommands.create({ environmentId: lab, displayName: 'Lab' }));
    const vault = await secrets.dispatchVault(
      vaultCommands.create({ vaultId: 'vlt_lab', displayName: 'Lab vault', environment: lab }),
    );
    expect(vault.environment).toBe('lab');
  });

  it('blocks deleting an environment until its vaults are destroyed', async () => {
    const backend = createFakeBackend();
    const secrets = backend.secrets.forProject(demo);
    const environments = backend.environments.forProject(demo);
    await environments.dispatch(environmentCommands.create({ environmentId: lab, displayName: 'Lab' }));
    await secrets.dispatchVault(
      vaultCommands.create({ vaultId: 'vlt_lab', displayName: 'Lab vault', environment: lab }),
    );

    await expect(environments.dispatch(environmentCommands.delete(lab), { expectedRevision: 1 })).rejects.toMatchObject(
      { reason: 'ENVIRONMENT_IN_USE' },
    );

    await secrets.dispatchVault(vaultCommands.transition('vlt_lab', 'SCHEDULE_DESTRUCTION'), { expectedRevision: 1 });
    await expect(environments.dispatch(environmentCommands.delete(lab), { expectedRevision: 1 })).rejects.toMatchObject(
      { reason: 'ENVIRONMENT_IN_USE' },
    );

    await secrets.dispatchVault(vaultCommands.transition('vlt_lab', 'CONFIRM_DESTROYED'), { expectedRevision: 2 });
    const deleted = await environments.dispatch(environmentCommands.delete(lab), { expectedRevision: 1 });
    expect(deleted.lifecycle).toBe('DELETED');
  });

  it('keeps the seeded staging environment in use', async () => {
    const environments = createFakeBackend().environments.forProject(demo);
    await expect(
      environments.dispatch(environmentCommands.delete(mintEnvironmentId('staging')), { expectedRevision: 1 }),
    ).rejects.toMatchObject({ reason: 'ENVIRONMENT_IN_USE' });
  });

  it('keeps each project with its own vaults', async () => {
    const backend = createFakeBackend();
    const demoVaults = await backend.secrets.forProject(demo).listVaults();
    const beaconVaults = await backend.secrets.forProject(seedProjectIds.beacon).listVaults();
    expect(demoVaults.length).toBeGreaterThan(1);
    expect(beaconVaults).toHaveLength(1);
    expect(beaconVaults.map((vault) => vault.vaultId)).not.toContain(demoVaults[0]!.vaultId);
  });

  it('gives a project without a seed only the built-in environments', async () => {
    const backend = createFakeBackend();
    const environments = await backend.environments.forProject(seedProjectIds.atlas).list();
    expect(environments.map((environment) => environment.environmentId).sort()).toEqual([
      'development',
      'preview',
      'production',
    ]);
    expect(await backend.secrets.forProject(seedProjectIds.atlas).listVaults()).toEqual([]);
  });

  it('does not leak a custom environment or a vault into another project', async () => {
    const backend = createFakeBackend();
    await backend.environments
      .forProject(demo)
      .dispatch(environmentCommands.create({ environmentId: lab, displayName: 'Lab' }));
    await backend.secrets
      .forProject(demo)
      .dispatchVault(vaultCommands.create({ vaultId: 'vlt_lab', displayName: 'Lab vault', environment: lab }));

    const beacon = seedProjectIds.beacon;
    const beaconEnvironments = await backend.environments.forProject(beacon).list();
    expect(beaconEnvironments.map((environment) => environment.environmentId)).not.toContain('lab');
    await expect(
      backend.secrets
        .forProject(beacon)
        .dispatchVault(vaultCommands.create({ vaultId: 'vlt_other', displayName: 'Other', environment: lab })),
    ).rejects.toMatchObject({ reason: 'ENVIRONMENT_NOT_FOUND' });
  });

  it('counts the resources each project owns', async () => {
    const backend = createFakeBackend();
    expect(await backend.projects.resources(demo)).toEqual({ vaults: 3, credentials: 8, environments: 5 });
    expect(await backend.projects.resources(seedProjectIds.beacon)).toEqual({
      vaults: 1,
      credentials: 2,
      environments: 3,
    });
  });
});
