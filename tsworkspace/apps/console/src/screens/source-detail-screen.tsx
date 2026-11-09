import { TriangleAlertIcon } from 'lucide-react';

import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Skeleton } from '@/components/ui/skeleton';
import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { ActivityPanel } from '@/features/gateway-sources/components/activity-panel';
import { CustodyPanel } from '@/features/gateway-sources/components/custody-panel';
import { RegistrationPanel } from '@/features/gateway-sources/components/registration-panel';
import { SlotsPanel } from '@/features/gateway-sources/components/slots-panel';
import { SourceActions } from '@/features/gateway-sources/components/source-actions';
import { SourceKindIcon } from '@/features/gateway-sources/components/source-kind-icon';
import { PreviewBadge, SourceStatusBadge } from '@/features/gateway-sources/components/status-badges';
import { sourceKindLabels } from '@/features/gateway-sources/domain/source-kind';
import { useSourceQuery } from '@/features/gateway-sources/queries';
import { MetaChip } from '@/features/secrets/components/badges';
import { useCredentialsQuery } from '@/features/secrets/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading source</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <Skeleton className="h-96" />
        <Skeleton className="h-52" />
      </div>
    </div>
  );
}

export function SourceDetailScreen({ integrationId }: { integrationId: string }) {
  const query = useSourceQuery(integrationId);
  const credentials = useCredentialsQuery();

  const back = <BackLink to="/sources">All sources</BackLink>;

  if (query.isPending) return <DetailSkeleton />;

  if (query.isError) return <FaultAlert error={query.error} title="Could not load the source" />;

  const source = query.data;
  if (!source) {
    return (
      <NotFoundState back={back} title="Source not found">
        No integration has the id <span className="font-mono">{integrationId}</span>.
      </NotFoundState>
    );
  }

  const knownSecrets = new Set(credentials.data?.map((credential) => credential.secret) ?? []);

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        leading={<SourceKindIcon kind={source.kind} className="size-12" />}
        title={source.displayName}
        status={<SourceStatusBadge status={source.status} />}
        titleAside={
          <>
            <CustodyTierBadge tier={source.tier} />
            <PreviewBadge />
          </>
        }
        identifier={<span className="font-mono">{source.id}</span>}
        copyValue={source.id}
        copyLabel="integration id"
        facts={[`Revision ${source.revision}`, sourceKindLabels[source.kind]]}
        chipsLabel="Routing"
        chips={
          <MetaChip title="NATS subject prefix" mono>
            {source.subjectPrefix}
          </MetaChip>
        }
        actions={<SourceActions source={source} />}
      />

      {source.attention ? (
        <div
          role="status"
          className="border-warning/35 bg-warning-soft text-warning-ink flex items-start gap-2.5 rounded-lg border px-4 py-3 text-[13px]"
        >
          <TriangleAlertIcon className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          {source.attention}
        </div>
      ) : null}

      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <SlotsPanel slots={source.slots} knownSecrets={knownSecrets} />
          <ActivityPanel source={source} />
        </div>
        <div className="flex min-w-0 flex-col gap-4">
          <RegistrationPanel registration={source.registration} />
          <CustodyPanel custody={source.custody} />
        </div>
      </div>
    </div>
  );
}
