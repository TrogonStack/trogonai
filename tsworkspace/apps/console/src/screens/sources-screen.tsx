import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { CustodyExplainer } from '@/features/custody/components/custody-explainer';
import { SourcesStats, SourcesStatsSkeleton } from '@/features/gateway-sources/components/sources-stats';
import { SourcesTable, SourcesTableSkeleton } from '@/features/gateway-sources/components/sources-table';
import { PreviewBadge } from '@/features/gateway-sources/components/status-badges';
import { useSourcesQuery } from '@/features/gateway-sources/queries';

export function SourcesScreen() {
  const sources = useSourcesQuery();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Gateway"
        title="Sources"
        badge={<PreviewBadge />}
        description="The webhook integrations the gateway receives from, the credentials that verify them and who holds the keys behind those credentials."
      />
      {sources.error ? <FaultAlert error={sources.error} title="Could not load sources" /> : null}
      {sources.data ? (
        <>
          <SourcesStats sources={sources.data} />
          <SourcesTable sources={sources.data} />
          <CustodyExplainer />
        </>
      ) : sources.error ? null : (
        <>
          <SourcesStatsSkeleton />
          <SourcesTableSkeleton />
        </>
      )}
    </div>
  );
}
