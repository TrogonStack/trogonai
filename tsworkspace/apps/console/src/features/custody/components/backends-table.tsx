import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { KeyBackendAdmission, KeyBackendLifecycle, KeyProvider } from '@trogonai/proto';
import { HeartPulseIcon, PauseCircleIcon, ServerIcon, XIcon } from 'lucide-react';
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
import { Button } from '@/components/ui/button';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { admissionLabels } from '../domain/backend';
import { keyProviderLabels } from '../domain/provider';
import type { BackendView } from '../domain/view';
import { AdmissionBadge, ConnectionBadge, LifecycleBadge } from './key-badges';

const columnHelper = createColumnHelper<BackendView>();

const matchesText: TextMatcher<BackendView> = (backend, needle) =>
  [backend.displayName, backend.id, keyProviderLabels[backend.provider]].join(' ').toLowerCase().includes(needle);

const connectionFacetLabels = { unknown: 'Not observed', connected: 'Connected', failed: 'Failed' } as const;

const columns = [
  columnHelper.accessor('displayName', {
    id: 'name',
    header: 'Backend',
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col">
        <Link
          to="/keys/backends/$backendId"
          params={{ backendId: row.original.id }}
          className="block max-w-64 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.displayName}
        </Link>
        <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">{row.original.id}</span>
      </div>
    ),
  }),
  columnHelper.accessor((row) => keyProviderLabels[row.provider], { id: 'provider', header: 'Provider' }),
  columnHelper.accessor('lifecycle', {
    header: 'Lifecycle',
    cell: ({ getValue }) => <LifecycleBadge lifecycle={getValue()} />,
  }),
  columnHelper.accessor('admission', {
    header: 'Admission',
    cell: ({ getValue }) => <AdmissionBadge admission={getValue()} />,
  }),
  columnHelper.accessor((row) => row.connection.kind, {
    id: 'connection',
    header: 'Connection',
    cell: ({ row }) => <ConnectionBadge connection={row.original.connection} />,
  }),
  columnHelper.accessor((row) => row.keys.length, {
    id: 'keys',
    header: 'Keys bound',
    cell: ({ getValue }) => <span className="font-mono text-xs tabular-nums">{getValue()}</span>,
  }),
  columnHelper.accessor((row) => (row.connection.kind === 'unknown' ? 0 : row.connection.observedAt.getTime()), {
    id: 'observed',
    header: 'Last observed',
    cell: ({ row }) =>
      row.original.connection.kind === 'unknown' ? (
        <span className="text-muted-foreground text-xs">Never</span>
      ) : (
        <RelativeTime date={row.original.connection.observedAt} className="text-muted-foreground text-xs" />
      ),
  }),
  columnHelper.display({ id: 'open', header: () => <ActionColumnHeader />, cell: () => <RowChevron /> }),
];

function buildFacets(backends: readonly BackendView[]): FilterFacet<BackendView>[] {
  const provider = facetFromRows({
    key: 'provider',
    label: 'Provider',
    icon: <ServerIcon aria-hidden="true" />,
    rows: backends,
    accessor: (row) => String(row.provider),
    describe: (value) => ({ label: keyProviderLabels[Number(value) as KeyProvider] }),
  });
  const admission = facetFromRows({
    key: 'admission',
    label: 'Admission',
    icon: <PauseCircleIcon aria-hidden="true" />,
    rows: backends,
    accessor: (row) => String(row.admission),
    describe: (value) => ({ label: admissionLabels[Number(value) as KeyBackendAdmission] }),
  });
  const connection = facetFromRows({
    key: 'connection',
    label: 'Connection',
    icon: <HeartPulseIcon aria-hidden="true" />,
    rows: backends,
    accessor: (row) => row.connection.kind,
    describe: (value) => ({ label: connectionFacetLabels[value as keyof typeof connectionFacetLabels] }),
  });
  return [provider, admission, connection].filter((facet) => facet.values.length > 0);
}

export function BackendsTableSkeleton() {
  return <TableSkeleton label="Loading backends" />;
}

export function BackendsTable({ backends }: { backends: readonly BackendView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(backends), [backends]);
  const visible = useMemo(() => applyFilters(backends, filters, facets, matchesText), [backends, filters, facets]);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter backends"
          placeholder="Filter by name, provider, admission or connection"
          rows={backends}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        backends.length === 0 ? (
          <TableEmptyState title="No backends registered" message="Register a backend to use keys you control." />
        ) : (
          <TableEmptyState title="No matches" message="No backend matches the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={() => setFilters(emptyFilterState)}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={backends.length} noun="backend" />}
      onOpen={(backend) => void navigate({ to: '/keys/backends/$backendId', params: { backendId: backend.id } })}
      muted={(backend) => backend.lifecycle === KeyBackendLifecycle.RETIRED}
    />
  );
}
