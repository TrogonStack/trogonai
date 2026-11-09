import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { FolderTreeIcon, XIcon } from 'lucide-react';
import { useMemo, useState, type ReactNode } from 'react';

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

import type { NodeView } from '../domain/view';

const columnHelper = createColumnHelper<NodeView>();

const matchesText: TextMatcher<NodeView> = (node, needle) =>
  `${node.name} ${node.nodeId}`.toLowerCase().includes(needle);

function Count({ value }: { value: number }) {
  return <span className={cn('font-mono text-xs tabular-nums', value === 0 && 'text-muted-foreground')}>{value}</span>;
}

const columns = [
  columnHelper.accessor('name', {
    id: 'name',
    header: 'Workspace',
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col">
        <Link
          to="/workspaces/$nodeId"
          params={{ nodeId: row.original.nodeId }}
          className="block max-w-56 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.name}
        </Link>
        <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">{row.original.nodeId}</span>
      </div>
    ),
  }),
  columnHelper.accessor('descendants', {
    id: 'folders',
    header: 'Folders',
    cell: ({ getValue }) => <Count value={getValue()} />,
  }),
  columnHelper.accessor('attachedBelow', {
    id: 'projects',
    header: 'Projects',
    cell: ({ getValue }) => <Count value={getValue()} />,
  }),
  columnHelper.display({
    id: 'open',
    header: () => <ActionColumnHeader />,
    cell: () => <RowChevron />,
  }),
];

export function WorkspacesTableSkeleton() {
  return <TableSkeleton label="Loading workspaces" />;
}

function buildFacets(workspaces: readonly NodeView[]): FilterFacet<NodeView>[] {
  const contents = facetFromRows({
    key: 'contents',
    label: 'Contents',
    icon: <FolderTreeIcon aria-hidden="true" />,
    rows: workspaces,
    accessor: (row) => (row.descendants > 0 || row.attachedBelow > 0 ? 'in-use' : 'empty'),
    describe: (value) => ({ label: value === 'in-use' ? 'Has folders or projects' : 'Empty' }),
  });
  return [contents].filter((facet) => facet.values.length > 0);
}

export function WorkspacesTable({
  workspaces,
  emptyAction,
}: {
  workspaces: readonly NodeView[];
  emptyAction: ReactNode;
}) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(workspaces), [workspaces]);
  const visible = useMemo(() => applyFilters(workspaces, filters, facets, matchesText), [workspaces, filters, facets]);
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter workspaces"
          placeholder="Filter by name or id"
          rows={workspaces}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        workspaces.length > 0 ? (
          <TableEmptyState title="No matches" message="No workspaces match the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState title="No workspaces yet" message="Add a workspace so projects have somewhere to live.">
            {emptyAction}
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={workspaces.length} noun="workspace" />}
      onOpen={(workspace) => void navigate({ to: '/workspaces/$nodeId', params: { nodeId: workspace.nodeId } })}
    />
  );
}
