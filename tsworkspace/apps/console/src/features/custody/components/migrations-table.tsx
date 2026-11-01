import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { ActivityIcon, ArrowRightIcon, TargetIcon } from 'lucide-react';
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
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { custodyTierLabels } from '../domain/custody';
import { migrationDirectionLabels, migrationStatusLabels, type MigrationStatus } from '../domain/migration';
import type { MigrationView } from '../domain/view';
import { MigrationStatusBadge } from './key-badges';
import { MigrationProgressBar } from './migration-progress';

const columnHelper = createColumnHelper<MigrationView>();

const matchesText: TextMatcher<MigrationView> = (view, needle) =>
  [view.migration.id, view.purposeLabel, custodyTierLabels[view.from.tier], custodyTierLabels[view.to.tier]]
    .join(' ')
    .toLowerCase()
    .includes(needle);

function buildFacets(migrations: readonly MigrationView[]): FilterFacet<MigrationView>[] {
  const labelOfPurpose = new Map(migrations.map((view) => [view.purpose as string, view.purposeLabel] as const));
  const status = facetFromRows({
    key: 'status',
    label: 'Status',
    icon: <ActivityIcon aria-hidden="true" />,
    rows: migrations,
    accessor: (row) => row.migration.status,
    describe: (value) => ({ label: migrationStatusLabels[value as MigrationStatus] }),
  });
  const purpose = facetFromRows({
    key: 'purpose',
    label: 'Purpose',
    icon: <TargetIcon aria-hidden="true" />,
    rows: migrations,
    accessor: (row) => row.purpose,
    describe: (value) => ({ label: labelOfPurpose.get(value) ?? value }),
  });
  return [status, purpose].filter((facet) => facet.values.length > 0);
}

const columns = [
  columnHelper.accessor((row) => row.migration.id, {
    id: 'id',
    header: 'Migration',
    cell: ({ row }) => (
      <Link
        to="/keys/migrations/$migrationId"
        params={{ migrationId: row.original.migration.id }}
        className="font-mono text-[13px] font-medium outline-none hover:underline focus-visible:underline"
      >
        {row.original.migration.id}
      </Link>
    ),
  }),
  columnHelper.accessor('purposeLabel', { id: 'purpose', header: 'Purpose' }),
  columnHelper.display({
    id: 'custody',
    header: 'From and to',
    cell: ({ row }) => (
      <span className="flex items-center gap-2 text-xs">
        {custodyTierLabels[row.original.from.tier]}
        <ArrowRightIcon className="text-muted-foreground size-3" aria-hidden="true" />
        {custodyTierLabels[row.original.to.tier]}
      </span>
    ),
  }),
  columnHelper.accessor((row) => migrationDirectionLabels[row.direction], {
    id: 'direction',
    header: 'Direction',
    cell: ({ getValue }) => <span className="text-muted-foreground text-xs">{getValue()}</span>,
  }),
  columnHelper.accessor((row) => row.migration.status, {
    id: 'status',
    header: 'Status',
    cell: ({ row }) => <MigrationStatusBadge status={row.original.migration.status} />,
  }),
  columnHelper.accessor('percent', {
    header: 'Progress',
    cell: ({ row }) => <MigrationProgressBar view={row.original} compact />,
  }),
  columnHelper.accessor((row) => row.migration.startedAt.getTime(), {
    id: 'started',
    header: 'Started',
    cell: ({ row }) => (
      <RelativeTime date={row.original.migration.startedAt} className="text-muted-foreground text-xs" />
    ),
  }),
  columnHelper.display({ id: 'open', header: () => <ActionColumnHeader />, cell: () => <RowChevron /> }),
];

export function MigrationsTableSkeleton() {
  return <TableSkeleton label="Loading migrations" />;
}

export function MigrationsTable({ migrations }: { migrations: readonly MigrationView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(migrations), [migrations]);
  const visible = useMemo(() => applyFilters(migrations, filters, facets, matchesText), [migrations, filters, facets]);
  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter migrations"
          placeholder="Filter by id, purpose, custody or status"
          rows={migrations}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        <TableEmptyState
          title="No migrations"
          message="No migration matches. Change custody for a purpose to move its payloads to another key."
        />
      }
      footer={<TableFooter shown={visible.length} total={migrations.length} noun="migration" />}
      onOpen={(view) =>
        void navigate({ to: '/keys/migrations/$migrationId', params: { migrationId: view.migration.id } })
      }
    />
  );
}
