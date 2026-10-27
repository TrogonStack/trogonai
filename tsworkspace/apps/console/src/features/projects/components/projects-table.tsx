import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { FolderTreeIcon, ShieldIcon, ShapesIcon, XIcon } from 'lucide-react';
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
import type { NodeId } from '@/features/hierarchy/domain/node-id';

import { accessPresetLabels, projectLifecycleLabels } from '../domain/labels';
import type { ProjectView } from '../domain/view';
import { AccessPresetChip } from './access-preset-chip';
import { ProjectLifecycleBadge } from './lifecycle-badge';
import { NodeLabel, useNodeLabel } from './node-label';
import { ResourceCounts } from './resource-counts';

const columnHelper = createColumnHelper<ProjectView>();

export function ProjectsTableSkeleton() {
  return <TableSkeleton label="Loading projects" />;
}

function buildFacets(projects: readonly ProjectView[], nodeLabel: (nodeId: NodeId) => string) {
  const lifecycle = facetFromRows({
    key: 'lifecycle',
    label: 'Lifecycle',
    icon: <ShapesIcon aria-hidden="true" />,
    rows: projects,
    accessor: (row) => String(row.lifecycle),
    describe: (value) => ({ label: projectLifecycleLabels[Number(value) as ProjectView['lifecycle']] }),
  });
  const access = facetFromRows({
    key: 'access',
    label: 'Access preset',
    icon: <ShieldIcon aria-hidden="true" />,
    rows: projects,
    accessor: (row) => String(row.initialAccessPreset),
    describe: (value) => ({ label: accessPresetLabels[Number(value) as ProjectView['initialAccessPreset']] }),
  });
  const parent = facetFromRows({
    key: 'parent',
    label: 'Parent',
    icon: <FolderTreeIcon aria-hidden="true" />,
    rows: projects,
    accessor: (row) => row.parent,
    describe: (value) => ({ label: nodeLabel(value as NodeId) }),
  });
  return [lifecycle, access, parent].filter((facet) => facet.values.length > 0) satisfies FilterFacet<ProjectView>[];
}

const columns = [
  columnHelper.accessor('displayName', {
    id: 'name',
    header: 'Project',
    cell: ({ row }) => (
      <div className="flex min-w-0 flex-col">
        <Link
          to="/projects/$projectId"
          params={{ projectId: row.original.projectId }}
          className="block max-w-56 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.displayName}
        </Link>
        <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">{row.original.projectId}</span>
      </div>
    ),
  }),
  columnHelper.accessor('goal', {
    header: 'Goal',
    enableSorting: false,
    cell: ({ getValue }) => (
      <span className="block max-w-64 truncate text-[13px]" title={getValue()}>
        {getValue()}
      </span>
    ),
  }),
  columnHelper.accessor('lifecycle', {
    header: 'Lifecycle',
    cell: ({ row }) => <ProjectLifecycleBadge lifecycle={row.original.lifecycle} />,
  }),
  columnHelper.accessor('initialAccessPreset', {
    header: 'Access preset',
    cell: ({ row }) => <AccessPresetChip preset={row.original.initialAccessPreset} />,
  }),
  columnHelper.accessor('parent', {
    header: 'Parent node',
    cell: ({ getValue }) => <NodeLabel nodeId={getValue()} />,
  }),
  columnHelper.display({
    id: 'resources',
    header: 'Resources',
    cell: ({ row }) => <ResourceCounts projectId={row.original.projectId} />,
  }),
  columnHelper.display({
    id: 'open',
    header: () => <ActionColumnHeader />,
    cell: () => <RowChevron />,
  }),
];

export function ProjectsTable({ projects, emptyAction }: { projects: readonly ProjectView[]; emptyAction: ReactNode }) {
  const navigate = useNavigate();
  const nodeLabel = useNodeLabel();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);

  const matchesText: TextMatcher<ProjectView> = useMemo(
    () => (project, needle) =>
      [project.displayName, project.projectId, project.goal, nodeLabel(project.parent)]
        .join(' ')
        .toLowerCase()
        .includes(needle),
    [nodeLabel],
  );

  const facets = useMemo(() => buildFacets(projects, nodeLabel), [projects, nodeLabel]);
  const visible = useMemo(
    () => applyFilters(projects, filters, facets, matchesText),
    [projects, filters, facets, matchesText],
  );
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      muted={(project) => project.archived}
      toolbar={
        <FilterBar
          label="Filter projects"
          placeholder="Filter by name, id, goal or parent"
          rows={projects}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        projects.length > 0 ? (
          <TableEmptyState title="No matches" message="No projects match the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState
            title="No projects yet"
            message="A project owns the vaults and environments of one piece of work. Create the first one."
          >
            {emptyAction}
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={projects.length} noun="project" />}
      onOpen={(project) => void navigate({ to: '/projects/$projectId', params: { projectId: project.projectId } })}
    />
  );
}
