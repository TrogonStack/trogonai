import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { CreateVaultDialog } from '@/features/secrets/components/create-vault-dialog';
import { CredentialsTable, CredentialsTableSkeleton } from '@/features/secrets/components/credentials-table';
import { SecretsStats, SecretsStatsSkeleton } from '@/features/secrets/components/secrets-stats';
import { VaultCards, VaultCardsSkeleton } from '@/features/secrets/components/vault-cards';
import { useCredentialsQuery, useVaultsQuery } from '@/features/secrets/queries';

export function SecretsScreen() {
  const vaults = useVaultsQuery();
  const credentials = useCredentialsQuery();
  const error = vaults.error ?? credentials.error;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Secret management"
        title="Secrets"
        description="Credentials held in vaults. The console tracks references and lifecycle. Secret material is write-only and never readable."
        actions={
          <>
            <CreateVaultDialog />
            <Link to="/secrets/new" className={buttonVariants({ size: 'lg' })}>
              <PlusIcon data-icon="inline-start" />
              Add credential
            </Link>
          </>
        }
      />
      {error ? <FaultAlert error={error} title="Could not load secrets" /> : null}
      {vaults.data && credentials.data ? (
        <>
          <SecretsStats credentials={credentials.data} />
          <VaultCards vaults={vaults.data} />
          <CredentialsTable credentials={credentials.data} vaults={vaults.data} />
        </>
      ) : error ? null : (
        <>
          <SecretsStatsSkeleton />
          <VaultCardsSkeleton />
          <CredentialsTableSkeleton />
        </>
      )}
    </div>
  );
}
