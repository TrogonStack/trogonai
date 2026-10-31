import { Link } from '@tanstack/react-router';
import { KeyRoundIcon, PauseCircleIcon, ServerIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { BackendsTable } from '@/features/custody/components/backends-table';
import { CustodyExplainer } from '@/features/custody/components/custody-explainer';
import { KeysStats, KeysStatsSkeleton } from '@/features/custody/components/keys-stats';
import { KeysSubnav } from '@/features/custody/components/keys-subnav';
import { PurposeCustodyPanel } from '@/features/custody/components/purpose-custody-panel';
import { useCustodyQuery } from '@/features/custody/queries';

function PausedBadge({ count }: { count: number }) {
  return (
    <span className="border-warning/35 bg-warning-soft text-warning-ink inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium">
      <PauseCircleIcon className="size-3.5" aria-hidden="true" />
      {count} paused
    </span>
  );
}

export function KeysScreen() {
  const query = useCustodyQuery();
  const paused = query.data?.stats.pausedBackends ?? 0;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Security"
        title="Encryption keys"
        description="Which backends the platform may reach to protect your data, and what happens when you pause one."
        badge={paused > 0 ? <PausedBadge count={paused} /> : undefined}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Link to="/keys/keys/new" className={buttonVariants({ variant: 'outline', size: 'lg' })}>
              <KeyRoundIcon data-icon="inline-start" />
              Bind a key
            </Link>
            <Link to="/keys/backends/new" className={buttonVariants({ size: 'lg' })}>
              <ServerIcon data-icon="inline-start" />
              Register a backend
            </Link>
          </div>
        }
      />
      <KeysSubnav />
      {query.error ? <FaultAlert error={query.error} title="Could not load encryption keys" /> : null}
      {query.data ? (
        <>
          <KeysStats stats={query.data.stats} />
          <PurposeCustodyPanel purposes={query.data.purposes} />
          <BackendsTable backends={query.data.backends} />
          <CustodyExplainer />
        </>
      ) : query.error ? null : (
        <div role="status" aria-busy="true" className="flex flex-col gap-6">
          <span className="sr-only">Loading encryption keys</span>
          <KeysStatsSkeleton />
          <Skeleton className="h-64" />
        </div>
      )}
    </div>
  );
}
