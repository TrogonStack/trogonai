import { VaultLifecycle } from '@trogonai/proto';
import { useMemo } from 'react';

import { toEnvironmentView, type EnvironmentView } from '@/features/environments/domain/view';
import { useEnvironmentsQuery } from '@/features/environments/queries';
import type { CredentialView, VaultView } from '@/features/secrets/domain/view';
import { useCredentialsQuery, useVaultsQuery } from '@/features/secrets/queries';

export interface EnvironmentWorkspace {
  readonly environments: readonly EnvironmentView[] | undefined;
  readonly vaults: readonly VaultView[] | undefined;
  readonly credentials: readonly CredentialView[] | undefined;
  readonly error: unknown;
}

export function vaultsIn(vaults: readonly VaultView[], environment: EnvironmentView): readonly VaultView[] {
  return vaults.filter(
    (vault) => vault.environment === environment.environmentId && vault.lifecycle !== VaultLifecycle.DESTROYED,
  );
}

export function credentialsIn(
  credentials: readonly CredentialView[],
  environment: EnvironmentView,
): readonly CredentialView[] {
  return credentials.filter((credential) => credential.environment === environment.environmentId);
}

export function useEnvironmentWorkspace(): EnvironmentWorkspace {
  const registry = useEnvironmentsQuery();
  const vaults = useVaultsQuery();
  const credentials = useCredentialsQuery();

  const environments = useMemo(() => {
    if (!registry.data || !vaults.data || !credentials.data) return undefined;
    const vaultList = vaults.data;
    const credentialList = credentials.data;
    return [...registry.data]
      .sort(
        (left, right) =>
          Number(right.builtIn) - Number(left.builtIn) || left.createdAt.getTime() - right.createdAt.getTime(),
      )
      .map((state) => {
        const bare = toEnvironmentView(state);
        const members = credentialsIn(credentialList, bare);
        return toEnvironmentView(state, {
          vaults: vaultsIn(vaultList, bare).length,
          credentials: members.length,
          needsAttention: members.filter((credential) => credential.needsAttention).length,
        });
      });
  }, [registry.data, vaults.data, credentials.data]);

  return {
    environments,
    vaults: vaults.data,
    credentials: credentials.data,
    error: registry.error ?? vaults.error ?? credentials.error,
  };
}
