import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { ConnectionStats, ConnectionStatsSkeleton } from '@/features/provider-connections/components/connection-stats';
import {
  ConnectionsTable,
  ConnectionsTableSkeleton,
} from '@/features/provider-connections/components/connections-table';
import { useConnectionsQuery } from '@/features/provider-connections/queries';

export function ProvidersScreen() {
  const query = useConnectionsQuery();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Model access"
        title="Provider connections"
        description="Registered model provider connections, their credential bindings and routing state."
        actions={
          <Link to="/providers/new" className={buttonVariants({ size: 'lg' })}>
            <PlusIcon data-icon="inline-start" />
            New connection
          </Link>
        }
      />
      {query.isError ? <FaultAlert error={query.error} title="Could not load connections" /> : null}
      {query.isPending ? (
        <>
          <ConnectionStatsSkeleton />
          <ConnectionsTableSkeleton />
        </>
      ) : query.data ? (
        <>
          <ConnectionStats connections={query.data} />
          <ConnectionsTable connections={query.data} />
        </>
      ) : null}
    </div>
  );
}
