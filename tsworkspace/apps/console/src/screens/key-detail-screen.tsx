import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { StatusPill } from '@/components/lifecycle-badge';
import { NotFoundState } from '@/components/not-found-state';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Skeleton } from '@/components/ui/skeleton';
import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { DefaultSwitch } from '@/features/custody/components/key-actions';
import { KeyStateBadge, PlatformManagedBadge } from '@/features/custody/components/key-badges';
import {
  KeyFactsPanel,
  KeyStatePanel,
  PayloadsPanel,
  UsagePanel,
  UsageRecordsPanel,
} from '@/features/custody/components/key-panels';
import { useKeyQuery } from '@/features/custody/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading key</span>
      <Skeleton className="h-16 w-96" />
      <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <Skeleton className="h-80" />
        <Skeleton className="h-64" />
      </div>
    </div>
  );
}

export function KeyDetailScreen({ keyRef }: { keyRef: string }) {
  const query = useKeyQuery(keyRef);
  const back = <BackLink to="/keys/keys">All keys</BackLink>;

  if (query.isPending) return <DetailSkeleton />;
  if (query.isError && query.data === undefined)
    return <FaultAlert error={query.error} title="Could not load the key" />;

  const view = query.data;
  if (!view) {
    return (
      <NotFoundState back={back} title="Key not found">
        No key has the reference <span className="font-mono">{keyRef}</span>.
      </NotFoundState>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      {query.error ? <FaultAlert error={query.error} title="Could not refresh the key" /> : null}
      <div>{back}</div>
      <DetailHeader
        title={view.keyRef}
        status={<KeyStateBadge state={view.state} />}
        titleAside={
          <>
            <CustodyTierBadge tier={view.tier} />
            {view.isDefault ? <StatusPill tone="info" label="Default" /> : null}
          </>
        }
        identifier={view.customer ? view.customer.backend.displayName : <PlatformManagedBadge />}
        copyValue={view.keyRef}
        copyLabel="key reference"
        facts={[view.purposeLabel]}
        actions={null}
      />

      <Card>
        <CardHeader>
          <CardTitle>Default for purpose</CardTitle>
        </CardHeader>
        <CardContent>
          <DefaultSwitch view={view} />
        </CardContent>
      </Card>

      <div className="grid items-start gap-4 xl:grid-cols-[minmax(0,1fr)_24rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <UsagePanel view={view} />
          {view.customer ? <UsageRecordsPanel view={view} /> : null}
          <PayloadsPanel view={view} />
        </div>
        <div className="flex min-w-0 flex-col gap-4">
          <KeyFactsPanel view={view} />
          <KeyStatePanel view={view} />
        </div>
      </div>
    </div>
  );
}
