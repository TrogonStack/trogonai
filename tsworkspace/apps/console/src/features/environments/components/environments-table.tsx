import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { ShapesIcon, VaultIcon, XIcon } from 'lucide-react';
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
import { cn } from '@/lib/utils';

import type { EnvironmentView } from '../domain/view';
import { CreateEnvironmentDialog } from './create-environment-dialog';
import { EnvironmentKindBadge } from './kind-badge';

const columnHelper = createColumnHelper<EnvironmentView>();

const matchesText: TextMatcher<EnvironmentView> = (environment, needle) =>
  [environment.displayName, environment.environmentId, environment.description]
    .join(' ')
    .toLowerCase()
    .includes(needle);

function Count({ value, attention }: { value: number; attention?: boolean }) {
  return (
    <span
      className={cn(
        'font-mono text-xs tabular-nums',
        value === 0 && 'text-muted-foreground',
        attention && value > 0 && 'text-danger-ink',
      )}
    >
      {value}
    </span>
  );
}

const columns = [
  columnHelper.accessor('displayName', {
    id: 'name',
    header: 'Environment',
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col">
        <Link
          to="/environments/$environmentId"
          params={{ environmentId: row.original.environmentId }}
          className="block max-w-56 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.displayName}
        </Link>
        <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">
          {row.original.environmentId}
        </span>
      </div>
    ),
  }),
  columnHelper.accessor((row) => (row.builtIn ? 'Built-in' : 'Custom'), {
    id: 'kind',
    header: 'Kind',
    cell: ({ row }) => <EnvironmentKindBadge builtIn={row.original.builtIn} />,
  }),
  columnHelper.accessor((row) => row.usage.vaults, {
    id: 'vaults',
    header: 'Vaults',
    cell: ({ getValue }) => <Count value={getValue()} />,
  }),
  columnHelper.accessor((row) => row.usage.credentials, {
    id: 'credentials',
    header: 'Credentials',
    cell: ({ getValue }) => <Count value={getValue()} />,
  }),
  columnHelper.accessor((row) => row.usage.needsAttention, {
    id: 'attention',
    header: 'Needs attention',
    cell: ({ getValue }) => <Count value={getValue()} attention />,
  }),
  columnHelper.display({
    id: 'open',
    header: () => <ActionColumnHeader />,
    cell: () => <RowChevron />,
  }),
];

export function EnvironmentsTableSkeleton() {
  return <TableSkeleton label="Loading environments" />;
}

function buildFacets(environments: readonly EnvironmentView[]): FilterFacet<EnvironmentView>[] {
  const kind = facetFromRows({
    key: 'kind',
    label: 'Kind',
    icon: <ShapesIcon aria-hidden="true" />,
    rows: environments,
    accessor: (row) => (row.builtIn ? 'built-in' : 'custom'),
    describe: (value) => ({ label: value === 'built-in' ? 'Built-in' : 'Custom' }),
  });
  const usage = facetFromRows({
    key: 'usage',
    label: 'Usage',
    icon: <VaultIcon aria-hidden="true" />,
    rows: environments,
    accessor: (row) => (row.usage.vaults > 0 ? 'in-use' : 'unused'),
    describe: (value) => ({ label: value === 'in-use' ? 'In use' : 'Unused' }),
  });
  return [kind, usage].filter((facet) => facet.values.length > 0);
}

export function EnvironmentsTable({ environments }: { environments: readonly EnvironmentView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(environments), [environments]);
  const visible = useMemo(
    () => applyFilters(environments, filters, facets, matchesText),
    [environments, filters, facets],
  );
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter environments"
          placeholder="Filter by name, id or kind"
          rows={environments}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        environments.length > 0 ? (
          <TableEmptyState title="No matches" message="No environments match the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState
            title="No environments yet"
            message="A blank map. Add an environment so vaults have somewhere to live."
          >
            <CreateEnvironmentDialog />
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={environments.length} noun="environment" />}
      onOpen={(environment) =>
        void navigate({ to: '/environments/$environmentId', params: { environmentId: environment.environmentId } })
      }
    />
  );
}
