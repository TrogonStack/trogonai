import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import type { KeyState } from '@trogonai/proto';
import { ActivityIcon, ShieldIcon, TargetIcon, XIcon } from 'lucide-react';
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
import { StatusPill } from '@/components/lifecycle-badge';
import { Button } from '@/components/ui/button';
import { RelativeTime } from '@/features/secrets/components/relative-time';

import { keyCapabilityLabels } from '../domain/capability';
import { custodyTierLabels, type CustodyTier } from '../domain/custody';
import { keyStateLabels } from '../domain/key-state';
import type { KeyView } from '../domain/view';
import { CustodyTierBadge } from './custody-tier-badge';
import { KeyStateBadge, PlatformManagedBadge } from './key-badges';

const columnHelper = createColumnHelper<KeyView>();

const backendName = (key: KeyView): string => key.customer?.backend.displayName ?? 'Managed by the platform';

const matchesText: TextMatcher<KeyView> = (key, needle) =>
  [key.keyRef, backendName(key), key.purposeLabel].join(' ').toLowerCase().includes(needle);

const columns = [
  columnHelper.accessor('keyRef', {
    header: 'Key reference',
    cell: ({ row }) => (
      <div className="flex items-center gap-2">
        <Link
          to="/keys/keys/$keyRef"
          params={{ keyRef: row.original.keyRef }}
          className="font-mono text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.keyRef}
        </Link>
        {row.original.isDefault ? <StatusPill tone="info" label="Default" /> : null}
      </div>
    ),
  }),
  columnHelper.accessor(backendName, {
    id: 'backend',
    header: 'Backend',
    cell: ({ row }) => (row.original.customer ? row.original.customer.backend.displayName : <PlatformManagedBadge />),
  }),
  columnHelper.accessor('purposeLabel', { id: 'purpose', header: 'Purpose' }),
  columnHelper.accessor((row) => row.capabilities.map((capability) => keyCapabilityLabels[capability]).join(', '), {
    id: 'capabilities',
    header: 'Capabilities',
    enableSorting: false,
    cell: ({ getValue }) => <span className="text-muted-foreground text-xs">{getValue()}</span>,
  }),
  columnHelper.accessor('tier', { header: 'Custody', cell: ({ getValue }) => <CustodyTierBadge tier={getValue()} /> }),
  columnHelper.accessor('state', { header: 'State', cell: ({ getValue }) => <KeyStateBadge state={getValue()} /> }),
  columnHelper.accessor((row) => row.observedAt.getTime(), {
    id: 'observed',
    header: 'Last observed',
    cell: ({ row }) => <RelativeTime date={row.original.observedAt} className="text-muted-foreground text-xs" />,
  }),
  columnHelper.display({ id: 'open', header: () => <ActionColumnHeader />, cell: () => <RowChevron /> }),
];

function buildFacets(keys: readonly KeyView[]): FilterFacet<KeyView>[] {
  const labelOfPurpose = new Map(keys.map((key) => [key.purpose as string, key.purposeLabel] as const));
  const purpose = facetFromRows({
    key: 'purpose',
    label: 'Purpose',
    icon: <TargetIcon aria-hidden="true" />,
    rows: keys,
    accessor: (row) => row.purpose,
    describe: (value) => ({ label: labelOfPurpose.get(value) ?? value }),
  });
  const tier = facetFromRows({
    key: 'tier',
    label: 'Custody',
    icon: <ShieldIcon aria-hidden="true" />,
    rows: keys,
    accessor: (row) => row.tier,
    describe: (value) => ({ label: custodyTierLabels[value as CustodyTier] }),
  });
  const state = facetFromRows({
    key: 'state',
    label: 'State',
    icon: <ActivityIcon aria-hidden="true" />,
    rows: keys,
    accessor: (row) => String(row.state),
    describe: (value) => ({ label: keyStateLabels[Number(value) as KeyState] }),
  });
  return [purpose, tier, state].filter((facet) => facet.values.length > 0);
}

export function KeysTableSkeleton() {
  return <TableSkeleton label="Loading keys" />;
}

export function KeysTable({ keys }: { keys: readonly KeyView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(keys), [keys]);
  const visible = useMemo(() => applyFilters(keys, filters, facets, matchesText), [keys, filters, facets]);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter keys"
          placeholder="Filter by reference, backend, purpose or state"
          rows={keys}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        keys.length > 0 ? (
          <TableEmptyState title="No matches" message="No key matches the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={() => setFilters(emptyFilterState)}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState
            title="No keys bound"
            message="Bind a key from a registered backend to use it for a purpose."
          />
        )
      }
      footer={<TableFooter shown={visible.length} total={keys.length} noun="key" />}
      onOpen={(key) => void navigate({ to: '/keys/keys/$keyRef', params: { keyRef: key.keyRef } })}
    />
  );
}
