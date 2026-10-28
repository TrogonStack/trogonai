import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { ProviderAccessScope, ProviderConnectionLifecycle, ProviderProtocol } from '@trogonai/proto';
import { ActivityIcon, PlugIcon, PlusIcon, ServerIcon, ShieldIcon, WaypointsIcon, XIcon } from 'lucide-react';
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
import {
  Chip,
  ConnectionLifecycleBadge,
  EligibilityIndicator,
  ScopeChip,
  StatusDot,
  connectionTone,
} from '@/components/lifecycle-badge';
import { ProviderAvatar } from '@/components/provider-avatar';
import { ScrambleText } from '@/components/scramble-text';
import { Button, buttonVariants } from '@/components/ui/button';

import { accessScopeLabels, connectionLifecycleLabels, protocolLabels } from '../domain/labels';
import { describeAuthentication, describeEndpoint, type ConnectionView, type EndpointView } from '../domain/view';

const columnHelper = createColumnHelper<ConnectionView>();

const matchesText: TextMatcher<ConnectionView> = (connection, needle) =>
  [connection.id, connection.displayName, connection.providerId, describeEndpoint(connection.endpoint)]
    .join(' ')
    .toLowerCase()
    .includes(needle);

function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl).host;
  } catch {
    return baseUrl;
  }
}

function EndpointCell({ endpoint }: { endpoint: EndpointView }) {
  if (endpoint.kind === 'deployment-local') {
    return (
      <span
        title={describeEndpoint(endpoint)}
        className="text-muted-foreground inline-flex items-center gap-1.5 font-mono text-xs"
      >
        <ServerIcon className="size-3.5 shrink-0" aria-hidden="true" />
        local · {endpoint.endpointId}
      </span>
    );
  }
  return (
    <span title={endpoint.baseUrl} className="block max-w-40 truncate font-mono text-xs">
      {hostOf(endpoint.baseUrl)}
    </span>
  );
}

function PreferenceChip({ value }: { value: number }) {
  return (
    <span
      title="Routing preference"
      className="border-border bg-surface-deep/60 text-muted-foreground inline-flex h-5 items-center rounded-sm border px-1.5 font-mono text-[11px] tabular-nums"
    >
      {value > 0 ? `+${value}` : value}
    </span>
  );
}

const columns = [
  columnHelper.accessor((row) => row.displayName || row.id, {
    id: 'name',
    header: 'Connection',
    cell: ({ row }) => (
      <div className="flex min-w-0 items-center gap-2.5">
        <ProviderAvatar
          providerId={row.original.providerId}
          name={row.original.displayName}
          muted={row.original.lifecycle === ProviderConnectionLifecycle.REVOKED}
        />
        <div className="flex min-w-0 flex-col">
          <Link
            to="/providers/$connectionId"
            params={{ connectionId: row.original.id }}
            className="truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
          >
            {row.original.displayName || row.original.id}
          </Link>
          <ScrambleText text={row.original.id} className="text-muted-foreground truncate text-[11px] leading-4" />
        </div>
      </div>
    ),
  }),
  columnHelper.accessor('accessScope', {
    header: 'Scope',
    cell: ({ getValue }) => <ScopeChip scope={getValue()} />,
  }),
  columnHelper.accessor((row) => describeEndpoint(row.endpoint), {
    id: 'endpoint',
    header: 'Endpoint',
    cell: ({ row }) => <EndpointCell endpoint={row.original.endpoint} />,
  }),
  columnHelper.accessor('lifecycle', {
    header: 'Status',
    cell: ({ getValue }) => <ConnectionLifecycleBadge lifecycle={getValue()} />,
  }),
  columnHelper.accessor('preference', {
    id: 'routing',
    header: 'Routing',
    cell: ({ row }) => (
      <div className="flex items-center gap-2">
        <EligibilityIndicator eligibility={row.original.eligibility} />
        <PreferenceChip value={row.original.preference} />
      </div>
    ),
  }),
  columnHelper.accessor((row) => (row.activeBinding ? describeAuthentication(row.activeBinding.authentication) : ''), {
    id: 'authentication',
    header: 'Active binding',
    cell: ({ row, getValue }) =>
      row.original.activeBinding ? (
        <div className="flex max-w-36 min-w-0 flex-col">
          <span className="truncate text-[13px] leading-5">{getValue().replace(/\s*\(.*\)$/, '')}</span>
          <ScrambleText
            text={row.original.activeBinding.id}
            className="text-muted-foreground truncate text-[11px] leading-4"
          />
        </div>
      ) : (
        <span className="text-muted-foreground">None</span>
      ),
  }),
  columnHelper.accessor((row) => row.models.length, {
    id: 'models',
    header: 'Models',
    cell: ({ row }) => {
      const [first, ...rest] = row.original.models;
      if (!first) return <span className="text-muted-foreground">None</span>;
      return (
        <div
          className="flex items-center gap-1"
          title={row.original.models.map((model) => model.identifier).join(', ')}
        >
          <Chip className="max-w-24 truncate font-mono text-[11px]">{first.identifier}</Chip>
          {rest.length > 0 ? <span className="text-muted-foreground text-xs tabular-nums">+{rest.length}</span> : null}
        </div>
      );
    },
  }),
  columnHelper.display({
    id: 'open',
    header: () => <ActionColumnHeader />,
    cell: () => <RowChevron />,
  }),
];

export function ConnectionsTableSkeleton() {
  return <TableSkeleton label="Loading connections" leading />;
}

function buildFacets(connections: readonly ConnectionView[]): FilterFacet<ConnectionView>[] {
  const provider = facetFromRows({
    key: 'provider',
    label: 'Provider',
    icon: <PlugIcon aria-hidden="true" />,
    rows: connections,
    accessor: (row) => row.providerId,
    describe: (value) => ({
      label: value,
      icon: <ProviderAvatar providerId={value} size="sm" className="size-4 rounded-[3px] text-[9px]" />,
    }),
  });
  const status = facetFromRows({
    key: 'status',
    label: 'Status',
    icon: <ActivityIcon aria-hidden="true" />,
    rows: connections,
    accessor: (row) => String(row.lifecycle),
    describe: (value) => {
      const lifecycle = Number(value) as ProviderConnectionLifecycle;
      return { label: connectionLifecycleLabels[lifecycle], icon: <StatusDot tone={connectionTone(lifecycle)} /> };
    },
  });
  const scope = facetFromRows({
    key: 'scope',
    label: 'Access scope',
    icon: <ShieldIcon aria-hidden="true" />,
    rows: connections,
    accessor: (row) => String(row.accessScope),
    describe: (value) => ({ label: accessScopeLabels[Number(value) as ProviderAccessScope] }),
  });
  const endpoint = facetFromRows({
    key: 'endpoint',
    label: 'Endpoint',
    icon: <ServerIcon aria-hidden="true" />,
    rows: connections,
    accessor: (row) => row.endpoint.kind,
    describe: (value) => ({ label: value === 'network' ? 'Network' : 'Deployment local' }),
  });
  const protocol = facetFromRows({
    key: 'protocol',
    label: 'Protocol',
    icon: <WaypointsIcon aria-hidden="true" />,
    rows: connections,
    accessor: (row) => row.protocols.map(String),
    describe: (value) => ({ label: protocolLabels[Number(value) as ProviderProtocol] }),
  });
  return [provider, status, scope, endpoint, protocol].filter((facet) => facet.values.length > 0);
}

export function ConnectionsTable({ connections }: { connections: readonly ConnectionView[] }) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);
  const facets = useMemo(() => buildFacets(connections), [connections]);
  const visible = useMemo(
    () => applyFilters(connections, filters, facets, matchesText),
    [connections, filters, facets],
  );
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter connections"
          placeholder="Filter by name, id, provider or endpoint"
          rows={connections}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        connections.length > 0 ? (
          <TableEmptyState
            title="No matches"
            message="No provider connections match the current filters. The signal is out there, just not under this filter."
          >
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState
            title="No provider connections yet"
            message="The rail is quiet. Register a connection to start routing model requests through a provider."
          >
            <Link to="/providers/new" className={buttonVariants({ size: 'sm' })}>
              <PlusIcon data-icon="inline-start" />
              New connection
            </Link>
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={connections.length} noun="connection" />}
      muted={(connection) => connection.lifecycle === ProviderConnectionLifecycle.REVOKED}
      onOpen={(connection) =>
        void navigate({ to: '/providers/$connectionId', params: { connectionId: connection.id } })
      }
    />
  );
}
