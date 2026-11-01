import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Skeleton } from '@/components/ui/skeleton';
import { EnvironmentChip, MetaChip, VaultLifecycleBadge } from '@/features/secrets/components/badges';
import { CredentialsTable, CredentialsTableSkeleton } from '@/features/secrets/components/credentials-table';
import { VaultActions } from '@/features/secrets/components/vault-actions';
import { VaultCustodyBadge } from '@/features/secrets/components/vault-custody-badge';
import { useCredentialsQuery, useVaultQuery, useVaultsQuery } from '@/features/secrets/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading vault</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <CredentialsTableSkeleton />
    </div>
  );
}

export function VaultDetailScreen({ vaultId }: { vaultId: string }) {
  const query = useVaultQuery(vaultId);
  const vaults = useVaultsQuery();
  const credentials = useCredentialsQuery();

  const back = <BackLink to="/secrets">All secrets</BackLink>;

  if (query.isPending) return <DetailSkeleton />;

  if (query.isError) return <FaultAlert error={query.error} title="Could not load the vault" />;

  const vault = query.data;
  if (!vault) {
    return (
      <NotFoundState back={back} title="Vault not found">
        No vault has the id <span className="font-mono">{vaultId}</span>.
      </NotFoundState>
    );
  }

  const own = credentials.data?.filter((credential) => credential.vaultId === vault.vaultId);

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        title={vault.displayName}
        status={<VaultLifecycleBadge lifecycle={vault.lifecycle} />}
        titleAside={
          <>
            <EnvironmentChip environment={vault.environment} linked />
            <VaultCustodyBadge tier={vault.custody} linked />
            <MetaChip title="The environment is fixed at creation">Immutable environment</MetaChip>
          </>
        }
        identifier={<span className="font-mono">{vault.vaultId}</span>}
        copyValue={vault.vaultId}
        copyLabel="vault id"
        facts={[`Revision ${vault.revision}`, `${vault.liveCredentialCount} live of ${vault.credentialCount}`]}
        actions={<VaultActions vault={vault} />}
      />
      {credentials.isError ? <FaultAlert error={credentials.error} title="Could not load credentials" /> : null}
      {own && vaults.data ? (
        <CredentialsTable
          credentials={own}
          vaults={vaults.data}
          vaultId={vault.vaultId}
          canAdd={vault.actions.addCredential.available}
        />
      ) : credentials.isError ? null : (
        <CredentialsTableSkeleton />
      )}
    </div>
  );
}
