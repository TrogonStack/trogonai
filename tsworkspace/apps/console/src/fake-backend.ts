import { VaultLifecycle } from '@trogonai/proto';

import { createFakeTenantCustodyClient } from './features/custody/fake';
import type { InMemoryTenantCustodyClient } from './features/custody/fake/in-memory-client';
import { createFakeEnvironmentsClient } from './features/environments/fake';
import type { InMemoryEnvironmentsClient } from './features/environments/fake/in-memory-client';
import { populateBuiltInEnvironmentSeed } from './features/environments/fake/seed';
import { createFakeHierarchyClient } from './features/hierarchy/fake';
import type { InMemoryHierarchyClient } from './features/hierarchy/fake/in-memory-client';
import { workspaceNodeDirectory } from './features/hierarchy/fake/workspace-nodes';
import type { ProjectResources, ProjectResourceSummary } from './features/projects/client';
import type { ProjectId } from './features/projects/domain/project-id';
import { createFakeProjectsClient } from './features/projects/fake';
import type { InMemoryProjectsClient } from './features/projects/fake/in-memory-client';
import { seedProjectIds } from './features/projects/fake/seed';
import { createFakeProviderConnectionsClient } from './features/provider-connections/fake';
import type { InMemoryProviderConnectionsClient } from './features/provider-connections/fake/in-memory-client';
import { createFakeSecretsClient } from './features/secrets/fake';
import type { InMemorySecretsClient } from './features/secrets/fake/in-memory-client';
import { populateBeaconSeed, populateSeed } from './features/secrets/fake/seed';

export interface FakeBackendOptions {
  readonly latencyMs?: number;
  readonly seeded?: boolean;
  readonly clock?: () => Date;
}

export interface FakeBackend {
  readonly providerConnections: InMemoryProviderConnectionsClient;
  readonly hierarchy: InMemoryHierarchyClient;
  readonly projects: InMemoryProjectsClient;
  readonly secrets: { forProject(projectId: ProjectId): InMemorySecretsClient };
  readonly environments: { forProject(projectId: ProjectId): InMemoryEnvironmentsClient };
  readonly keyCustody: InMemoryTenantCustodyClient;
}

interface ProjectWorkspace {
  readonly secrets: InMemorySecretsClient;
  readonly environments: InMemoryEnvironmentsClient;
}

export function createFakeBackend(options: FakeBackendOptions = {}): FakeBackend {
  const workspaces = new Map<ProjectId, ProjectWorkspace>();

  const workspaceOf = (projectId: ProjectId): ProjectWorkspace => {
    const existing = workspaces.get(projectId);
    if (existing) return existing;

    const registry: { environments?: InMemoryEnvironmentsClient; secrets?: InMemorySecretsClient } = {};
    const isDemo = projectId === seedProjectIds.demo;

    const secrets = createFakeSecretsClient({
      ...options,
      projectId,
      seeded: (options.seeded ?? true) && (isDemo || projectId === seedProjectIds.beacon),
      populate: isDemo ? populateSeed : populateBeaconSeed,
      environments: {
        exists: async (environmentId) => (await registry.environments?.get(environmentId)) !== undefined,
      },
    });
    registry.secrets = secrets;

    const environments = createFakeEnvironmentsClient({
      ...options,
      projectId,
      ...(isDemo ? {} : { populate: populateBuiltInEnvironmentSeed }),
      usage: {
        blockingVaults: async (environmentId) =>
          ((await registry.secrets?.listVaults()) ?? []).filter(
            (vault) => vault.environment === environmentId && vault.lifecycle !== VaultLifecycle.DESTROYED,
          ).length,
      },
    });
    registry.environments = environments;

    const workspace = { secrets, environments };
    workspaces.set(projectId, workspace);
    return workspace;
  };

  const resources: ProjectResources = {
    async summarize(projectId): Promise<ProjectResourceSummary> {
      const { secrets, environments } = workspaceOf(projectId);
      const [vaults, credentials, registered] = await Promise.all([
        secrets.listVaults(),
        secrets.listCredentials(),
        environments.list(),
      ]);
      return {
        vaults: vaults.filter((vault) => vault.lifecycle !== VaultLifecycle.DESTROYED).length,
        credentials: credentials.length,
        environments: registered.length,
      };
    },
  };

  const registry: { projects?: InMemoryProjectsClient } = {};

  const hierarchy = createFakeHierarchyClient({
    ...options,
    attachments: { attachedTo: async (nodeId) => (await registry.projects?.attachedTo(nodeId)) ?? 0 },
  });

  const projects = createFakeProjectsClient({ ...options, nodes: workspaceNodeDirectory(hierarchy), resources });
  registry.projects = projects;

  return {
    providerConnections: createFakeProviderConnectionsClient(options),
    hierarchy,
    projects,
    secrets: { forProject: (projectId) => workspaceOf(projectId).secrets },
    environments: { forProject: (projectId) => workspaceOf(projectId).environments },
    keyCustody: createFakeTenantCustodyClient(options),
  };
}
