import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { CustodyExplainer } from '@/features/custody/components/custody-explainer';
import { CreateWebhookButton } from '@/features/gateway-sources/components/generic-webhooks';
import { SourcesStats, SourcesStatsSkeleton } from '@/features/gateway-sources/components/sources-stats';
import { SourcesTable, SourcesTableSkeleton } from '@/features/gateway-sources/components/sources-table';
import { PreviewBadge } from '@/features/gateway-sources/components/status-badges';
import { useSourceList } from '@/features/gateway-sources/source-list-query';

export function SourcesScreen() {
  const sources = useSourceList();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Gateway"
        title="Sources"
        badge={<PreviewBadge />}
        description="Receive events through a generic webhook or a provider integration, and manage the credentials that verify each delivery."
        actions={<CreateWebhookButton />}
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
