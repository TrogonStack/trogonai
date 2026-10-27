import { KeyRoundIcon, TriangleAlertIcon, VaultIcon } from 'lucide-react';

import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Stat, StatStrip } from '@/components/stat-strip';
import { Skeleton } from '@/components/ui/skeleton';
import { EnvironmentActions } from '@/features/environments/components/environment-actions';
import { EnvironmentKindBadge } from '@/features/environments/components/kind-badge';
import { CredentialsTable, CredentialsTableSkeleton } from '@/features/secrets/components/credentials-table';
import { VaultCards, VaultCardsSkeleton } from '@/features/secrets/components/vault-cards';

import { credentialsIn, useEnvironmentWorkspace, vaultsIn } from './use-environment-workspace';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading environment</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <VaultCardsSkeleton />
      <CredentialsTableSkeleton />
    </div>
  );
}

export function EnvironmentDetailScreen({ environmentId }: { environmentId: string }) {
  const { environments, vaults, credentials, error } = useEnvironmentWorkspace();

  const back = <BackLink to="/environments">All environments</BackLink>;

  if (error) return <FaultAlert error={error} title="Could not load the environment" />;
  if (!environments || !vaults || !credentials) return <DetailSkeleton />;

  const environment = environments.find((candidate) => candidate.environmentId === environmentId);
  if (!environment) {
    return (
      <NotFoundState back={back} title="Environment not found">
        No environment has the id <span className="font-mono">{environmentId}</span>.
      </NotFoundState>
    );
  }

  const ownVaults = vaultsIn(vaults, environment);
  const ownCredentials = credentialsIn(credentials, environment);

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        title={environment.displayName}
        status={<EnvironmentKindBadge builtIn={environment.builtIn} />}
        identifier={<span className="font-mono">{environment.environmentId}</span>}
        copyValue={environment.environmentId}
        copyLabel="environment id"
        facts={[`Revision ${environment.revision}`]}
        actions={<EnvironmentActions environment={environment} />}
      />
      {environment.description ? (
        <p className="text-muted-foreground max-w-prose text-sm">{environment.description}</p>
      ) : null}
      <StatStrip label="Environment summary">
        <Stat
          label="Vaults"
          value={environment.usage.vaults}
          caption={environment.usage.vaults === 0 ? 'Nothing is placed here' : 'Not yet destroyed'}
          icon={VaultIcon}
          led="[--led:var(--primary)]"
        />
        <Stat
          label="Credentials"
          value={environment.usage.credentials}
          caption="Inherited from their vaults"
          icon={KeyRoundIcon}
          led="[--led:var(--success)]"
        />
        <Stat
          label="Needs attention"
          value={environment.usage.needsAttention}
          caption={environment.usage.needsAttention === 0 ? 'Nothing to act on' : 'Open a credential to resolve'}
          icon={TriangleAlertIcon}
          led="[--led:var(--danger)]"
          blink={environment.usage.needsAttention > 0}
        />
      </StatStrip>
      {ownVaults.length > 0 ? (
        <VaultCards vaults={ownVaults} />
      ) : (
        <section aria-label="Vaults" className="flex flex-col gap-3">
          <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Vaults
          </h2>
          <p className="text-muted-foreground border-border rounded-lg border border-dashed p-6 text-center text-sm">
            No vault lives here yet. Create one and pick {environment.displayName} to see it show up.
          </p>
        </section>
      )}
      <CredentialsTable credentials={ownCredentials} vaults={vaults} showEnvironmentFacet={false} />
    </div>
  );
}
