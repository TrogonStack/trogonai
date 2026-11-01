import { Link } from '@tanstack/react-router';
import { KeyRoundIcon } from 'lucide-react';

import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { buttonVariants } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import {
  AccessPanel,
  AdmissionPanel,
  BoundKeysPanel,
  ConnectionPanel,
  ExpiryPanel,
  RevisionHistoryPanel,
} from '@/features/custody/components/backend-panels';
import { AdmissionBadge, LifecycleBadge } from '@/features/custody/components/key-badges';
import { keyProviderLabels } from '@/features/custody/domain/provider';
import { useBackendQuery } from '@/features/custody/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading backend</span>
      <Skeleton className="h-16 w-96" />
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <Skeleton className="h-80" />
        <Skeleton className="h-64" />
      </div>
    </div>
  );
}

export function BackendDetailScreen({ backendId }: { backendId: string }) {
  const query = useBackendQuery(backendId);
  const back = <BackLink to="/keys/backends">All backends</BackLink>;

  if (query.isPending || (query.isFetching && query.data == null)) return <DetailSkeleton />;
  if (query.isError && query.data == null) {
    return <FaultAlert error={query.error} title="Could not load the backend" />;
  }

  const backend = query.data;
  if (!backend) {
    return (
      <NotFoundState back={back} title="Backend not found">
        No key backend has the id <span className="font-mono">{backendId}</span>.
      </NotFoundState>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      {query.error ? <FaultAlert error={query.error} title="Could not refresh the backend" /> : null}
      <DetailHeader
        title={backend.displayName}
        status={<LifecycleBadge lifecycle={backend.lifecycle} />}
        titleAside={<AdmissionBadge admission={backend.admission} />}
        identifier={<span className="font-mono">{backend.id}</span>}
        copyValue={backend.id}
        copyLabel="backend id"
        facts={[`Revision ${backend.revision}`, keyProviderLabels[backend.provider]]}
        actions={
          <Link to="/keys/keys/new" search={{ backend: backend.id }} className={buttonVariants({ size: 'lg' })}>
            <KeyRoundIcon data-icon="inline-start" />
            Bind a key
          </Link>
        }
      />
      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <ConnectionPanel backend={backend} />
          <ExpiryPanel backend={backend} />
          <BoundKeysPanel backend={backend} />
        </div>
        <div className="flex min-w-0 flex-col gap-4">
          <AdmissionPanel backend={backend} />
          <AccessPanel backend={backend} />
          <RevisionHistoryPanel backend={backend} />
        </div>
      </div>
    </div>
  );
}
