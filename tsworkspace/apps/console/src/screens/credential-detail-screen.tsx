import { CredentialLifecycle } from '@trogonai/proto';
import { TriangleAlertIcon, VaultIcon } from 'lucide-react';

import { BackLink } from '@/components/back-link';
import { CountTag } from '@/components/count-tag';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ActivityTimeline } from '@/features/secrets/components/activity-timeline';
import { CredentialLifecycleBadge, EnvironmentChip, MetaChip } from '@/features/secrets/components/badges';
import { CredentialActions } from '@/features/secrets/components/credential-actions';
import { LeasesTable } from '@/features/secrets/components/leases-table';
import { formatCompact, formatTimestamp } from '@/features/secrets/components/time';
import { UsesTable } from '@/features/secrets/components/uses-table';
import { VaultCustodyBadge } from '@/features/secrets/components/vault-custody-badge';
import { VersionLadder } from '@/features/secrets/components/version-ladder';
import { WriteSaga } from '@/features/secrets/components/write-saga';
import { kindLabels, originLabels, purposeLabels } from '@/features/secrets/domain/labels';
import { useCredentialHistoryQuery, useCredentialQuery, useVaultQuery } from '@/features/secrets/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading credential</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <Skeleton className="h-10 w-full" />
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <Skeleton className="h-96" />
        <Skeleton className="h-52" />
      </div>
    </div>
  );
}

export function CredentialDetailScreen({ secretRef }: { secretRef: string }) {
  const query = useCredentialQuery(secretRef);
  const history = useCredentialHistoryQuery(secretRef);
  const vault = useVaultQuery(query.data?.vaultId ?? '');

  const back = <BackLink to="/secrets">All secrets</BackLink>;

  if (query.isPending) return <DetailSkeleton />;

  if (query.isError) return <FaultAlert error={query.error} title="Could not load the credential" />;

  const credential = query.data;
  if (!credential) {
    return (
      <NotFoundState back={back} title="Credential not found">
        No credential has the secret reference <span className="font-mono">{secretRef}</span>.
      </NotFoundState>
    );
  }

  const destroyed = credential.lifecycle === CredentialLifecycle.DESTROYED;

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-2">
        {back}
        <BackLink
          to="/secrets/vaults/$vaultId"
          params={{ vaultId: credential.vaultId }}
          icon={<VaultIcon data-icon="inline-start" />}
        >
          {vault.data ? `${vault.data.displayName} vault` : 'Vault'}
        </BackLink>
      </div>
      <DetailHeader
        title={credential.displayName}
        status={<CredentialLifecycleBadge lifecycle={credential.lifecycle} />}
        titleAside={vault.data ? <VaultCustodyBadge tier={vault.data.custody} linked /> : undefined}
        identifier={
          <span className="min-w-0 truncate font-mono" title={credential.secret}>
            {credential.secret}
          </span>
        }
        copyValue={credential.secret}
        copyLabel="secret reference"
        facts={[`Revision ${credential.revision}`]}
        chipsLabel="Immutable policy"
        chips={
          <>
            <MetaChip title="Purpose">{purposeLabels[credential.purpose]}</MetaChip>
            <MetaChip title="Kind">{kindLabels[credential.kind]}</MetaChip>
            <EnvironmentChip environment={credential.environment} linked />
            {credential.origin !== undefined ? (
              <MetaChip title="Origin of the current material">{originLabels[credential.origin]}</MetaChip>
            ) : null}
          </>
        }
        actions={<CredentialActions credential={credential} />}
      />

      {credential.needsAttention && credential.attentionReason ? (
        <div
          role="status"
          className="border-warning/35 bg-warning-soft text-warning-ink flex items-start gap-2.5 rounded-lg border px-4 py-3 text-[13px]"
        >
          <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          {credential.attentionReason}
        </div>
      ) : null}

      {credential.destruction && !destroyed ? (
        <div role="status" className="border-border bg-surface-deep/60 rounded-lg border px-4 py-3 text-[13px]">
          Destruction is scheduled for{' '}
          <time
            dateTime={credential.destruction.executeAfter.toISOString()}
            title={formatTimestamp(credential.destruction.executeAfter)}
            className="font-mono"
          >
            {formatCompact(credential.destruction.executeAfter)}
          </time>
          {credential.destruction.due ? '. It is due.' : '.'}
          {credential.liveLeaseCount > 0
            ? ` ${credential.liveLeaseCount} live ${credential.liveLeaseCount === 1 ? 'lease blocks' : 'leases block'} confirmation.`
            : ''}
        </div>
      ) : null}

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="activity">
            Activity
            <CountTag count={history.data?.length} />
          </TabsTrigger>
          <TabsTrigger value="uses">
            Uses
            <CountTag count={credential.uses.length} />
          </TabsTrigger>
          <TabsTrigger value="leases">
            Leases
            <CountTag count={credential.leases.length} />
          </TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
            <Card>
              <CardHeader>
                <CardTitle>Versions</CardTitle>
              </CardHeader>
              <CardContent>
                <VersionLadder credential={credential} />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle>Write progress</CardTitle>
              </CardHeader>
              <CardContent>
                <WriteSaga credential={credential} />
              </CardContent>
            </Card>
          </div>
        </TabsContent>
        <TabsContent value="activity">
          {history.data ? (
            <ActivityTimeline events={history.data} />
          ) : (
            <Skeleton role="status" aria-label="Loading activity" className="h-48 w-full" />
          )}
        </TabsContent>
        <TabsContent value="uses">
          <UsesTable uses={credential.uses} />
        </TabsContent>
        <TabsContent value="leases">
          <LeasesTable leases={credential.leases} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
