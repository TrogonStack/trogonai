import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { ActivityIcon, ShapesIcon, ShieldIcon, XIcon } from 'lucide-react';
import { useMemo, useState } from 'react';

import {
  ActionColumnHeader,
  DataTable,
  RowChevron,
  TableEmptyState,
  TableFooter,
  TableSkeleton,
} from '@/components/data-table';
import {
  FilterBar,
  applyFilters,
  emptyFilterState,
  facetFromRows,
  type FilterFacet,
  type FilterState,
  type TextMatcher,
} from '@/components/filter-bar';
import { StatusDot } from '@/components/lifecycle-badge';
import { Button } from '@/components/ui/button';
import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { custodyTierLabels, type CustodyTier } from '@/features/custody/domain/custody';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { statusLabels, statusTones } from '../domain/labels';
import { sourceKindLabels, type SourceKind } from '../domain/source-kind';
import type { IntegrationStatus, SourceView } from '../domain/view';
import { Sparkline } from './activity-chart';
import { SourceKindIcon } from './source-kind-icon';
import { SourceStatusBadge } from './status-badges';

const columnHelper = createColumnHelper<SourceView>();

const matchesText: TextMatcher<SourceView> = (source, needle) =>
  [source.displayName, source.id, sourceKindLabels[source.kind], source.subjectPrefix]
    .join(' ')
    .toLowerCase()
    .includes(needle);

const columns = [
  columnHelper.accessor('displayName', {
    id: 'name',
    header: 'Integration',
    cell: ({ row }) => (
      <div className="flex min-w-0 items-center gap-2.5">
        <SourceKindIcon kind={row.original.kind} />
        <div className="flex min-w-0 flex-col">
          <Link
            to="/sources/$integrationId"
            params={{ integrationId: row.original.id }}
            className="block max-w-56 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
          >
            {row.original.displayName}
          </Link>
          <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">{row.original.id}</span>
        </div>
      </div>
    ),
  }),
  columnHelper.accessor('status', {
    header: 'Status',
    cell: ({ getValue }) => <SourceStatusBadge status={getValue()} />,
  }),
  columnHelper.accessor('credentialSummary', {
    header: 'Credentials',
    enableSorting: false,
    cell: ({ row }) => (
      <span className="flex flex-col">
        <span className="text-[13px] leading-5">{row.original.credentialSummary}</span>
        <span className="text-muted-foreground text-xs leading-4">
          {row.original.slots.length} {row.original.slots.length === 1 ? 'slot' : 'slots'}
        </span>
      </span>
    ),
  }),
  columnHelper.accessor('tier', {
    header: 'Custody',
    cell: ({ getValue }) => <CustodyTierBadge tier={getValue()} />,
  }),
  columnHelper.accessor((row) => row.lastDeliveryAt?.getTime() ?? 0, {
    id: 'last-delivery',
    header: 'Last delivery',
    cell: ({ row }) =>
      row.original.lastDeliveryAt ? (
        <RelativeTime date={row.original.lastDeliveryAt} className="text-muted-foreground text-xs" />
      ) : (
        <span className="text-muted-foreground">Never</span>
      ),
  }),
  columnHelper.accessor('deliveries24h', {
    header: 'Last 24 hours',
    cell: ({ row }) => (
      <span className="flex items-center gap-3">
        <Sparkline hourly={row.original.hourly} />
        <span className="font-mono text-xs tabular-nums">{row.original.deliveries24h}</span>
      </span>
    ),
  }),
  columnHelper.display({
    id: 'open',
    header: () => <ActionColumnHeader />,
    cell: () => <RowChevron />,
  }),
];

function buildFacets(sources: readonly SourceView[]): FilterFacet<SourceView>[] {
  const kind = facetFromRows({
    key: 'kind',
    label: 'Kind',
    icon: <ShapesIcon aria-hidden="true" />,
    rows: sources,
    accessor: (row) => row.kind,
    describe: (value) => ({ label: sourceKindLabels[value as SourceKind] }),
  });
  const status = facetFromRows({
    key: 'status',
    label: 'Status',
    icon: <ActivityIcon aria-hidden="true" />,
    rows: sources,
    accessor: (row) => row.status,
    describe: (value) => ({
      label: statusLabels[value as IntegrationStatus],
      icon: <StatusDot tone={statusTones[value as IntegrationStatus]} />,
    }),
  });
  const tier = facetFromRows({
    key: 'tier',
    label: 'Custody',
    icon: <ShieldIcon aria-hidden="true" />,
    rows: sources,
    accessor: (row) => row.tier,
    describe: (value) => ({ label: custodyTierLabels[value as CustodyTier] }),
  });
  return [kind, status, tier].filter((facet) => facet.values.length > 0);
}

export function SourcesTableSkeleton() {
  return <TableSkeleton label="Loading sources" />;
}

export function SourcesTable({ sources }: { sources: readonly SourceView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(sources), [sources]);
  const visible = useMemo(() => applyFilters(sources, filters, facets, matchesText), [sources, filters, facets]);
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter sources"
          placeholder="Filter by name, kind, status or custody tier"
          rows={sources}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        sources.length > 0 ? (
          <TableEmptyState title="No matches" message="No integrations match the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState title="No integrations yet" message="No gateway source is configured for this project." />
        )
      }
      footer={<TableFooter shown={visible.length} total={sources.length} noun="integration" />}
      muted={(source) => source.status === 'paused'}
      onOpen={(source) => void navigate({ to: '/sources/$integrationId', params: { integrationId: source.id } })}
    />
  );
}
