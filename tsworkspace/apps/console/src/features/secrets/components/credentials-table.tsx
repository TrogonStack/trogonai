import { Link, useNavigate } from '@tanstack/react-router';
import { createColumnHelper } from '@tanstack/react-table';
import { CredentialLifecycle, SecretPurpose } from '@trogonai/proto';
import {
  ActivityIcon,
  FingerprintIcon,
  LayersIcon,
  PlusIcon,
  ShapesIcon,
  TagIcon,
  VaultIcon,
  XIcon,
} from 'lucide-react';
import { useMemo, useState } from 'react';

import { CopyButton, truncateMiddle } from '@/components/copy-button';
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
import { Button, buttonVariants } from '@/components/ui/button';
import { useEnvironmentNames } from '@/features/environments/queries';

import {
  credentialLifecycleLabels,
  credentialLifecycleTones,
  kindLabels,
  originLabels,
  purposeLabels,
} from '../domain/labels';
import type { CredentialView, VaultView } from '../domain/view';
import { CredentialLifecycleBadge } from './badges';
import { RelativeTime } from './relative-time';

const columnHelper = createColumnHelper<CredentialView>();

const matchesText: TextMatcher<CredentialView> = (credential, needle) =>
  [
    credential.displayName,
    credential.secret,
    credential.environment,
    purposeLabels[credential.purpose],
    kindLabels[credential.kind],
  ]
    .join(' ')
    .toLowerCase()
    .includes(needle);

function buildColumns(vaultNames: ReadonlyMap<string, string>, showVault: boolean) {
  const columns = [
    columnHelper.accessor('displayName', {
      id: 'name',
      header: 'Credential',
      cell: ({ row }) => (
        <Link
          to="/secrets/credentials/$secretRef"
          params={{ secretRef: row.original.secret }}
          className="block max-w-56 truncate text-[13px] leading-5 font-medium outline-none hover:underline focus-visible:underline"
        >
          {row.original.displayName}
        </Link>
      ),
    }),
    columnHelper.accessor('secret', {
      id: 'ref',
      header: 'Secret reference',
      enableSorting: false,
      cell: ({ getValue }) => (
        <span className="text-muted-foreground inline-flex items-center gap-1 font-mono text-[11px]">
          <span title={getValue()}>{truncateMiddle(getValue(), 8, 6)}</span>
          <CopyButton value={getValue()} label="secret reference" />
        </span>
      ),
    }),
    columnHelper.accessor((row) => purposeLabels[row.purpose], {
      id: 'purpose',
      header: 'Purpose',
      cell: ({ row, getValue }) => (
        <span className="flex min-w-0 flex-col">
          <span className="text-[13px] leading-5">{getValue()}</span>
          <span className="text-muted-foreground text-xs leading-4">{kindLabels[row.original.kind]}</span>
        </span>
      ),
    }),
  ];
  const tail = [
    columnHelper.accessor('lifecycle', {
      header: 'Status',
      cell: ({ getValue }) => <CredentialLifecycleBadge lifecycle={getValue()} />,
    }),
    columnHelper.accessor((row) => row.activeVersion ?? 0, {
      id: 'version',
      header: 'Version',
      cell: ({ row }) =>
        row.original.activeVersion === undefined ? (
          <span className="text-muted-foreground">None</span>
        ) : (
          <span className="font-mono text-xs tabular-nums">v{row.original.activeVersion}</span>
        ),
    }),
    columnHelper.accessor((row) => row.lastRotatedAt?.getTime() ?? 0, {
      id: 'rotated',
      header: 'Last rotated',
      cell: ({ row }) =>
        row.original.lastRotatedAt ? (
          <RelativeTime date={row.original.lastRotatedAt} className="text-muted-foreground text-xs" />
        ) : (
          <span className="text-muted-foreground">Never</span>
        ),
    }),
    columnHelper.display({
      id: 'open',
      header: () => <ActionColumnHeader />,
      cell: () => <RowChevron />,
    }),
  ];
  const vault = showVault
    ? [
        columnHelper.accessor((row) => vaultNames.get(row.vaultId) ?? row.vaultId, {
          id: 'vault',
          header: 'Vault',
          cell: ({ row, getValue }) => (
            <Link
              to="/secrets/vaults/$vaultId"
              params={{ vaultId: row.original.vaultId }}
              className="text-muted-foreground text-[13px] outline-none hover:underline focus-visible:underline"
            >
              {getValue()}
            </Link>
          ),
        }),
      ]
    : [];
  return [...columns, ...vault, ...tail];
}

export function CredentialsTableSkeleton() {
  return <TableSkeleton label="Loading credentials" />;
}

interface CredentialsTableProps {
  credentials: readonly CredentialView[];
  vaults: readonly VaultView[];
  vaultId?: string;
  canAdd?: boolean;
  showEnvironmentFacet?: boolean;
}

function buildFacets(
  credentials: readonly CredentialView[],
  vaultNames: ReadonlyMap<string, string>,
  environmentNames: ReadonlyMap<string, string>,
  showVault: boolean,
  showEnvironment: boolean,
): FilterFacet<CredentialView>[] {
  const environment = facetFromRows({
    key: 'environment',
    label: 'Environment',
    icon: <LayersIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => row.environment,
    describe: (value) => ({ label: environmentNames.get(value) ?? value }),
  });
  const vault = facetFromRows({
    key: 'vault',
    label: 'Vault',
    icon: <VaultIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => row.vaultId,
    describe: (value) => ({ label: vaultNames.get(value) ?? value }),
  });
  const purpose = facetFromRows({
    key: 'purpose',
    label: 'Purpose',
    icon: <TagIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => String(row.purpose),
    describe: (value) => ({ label: purposeLabels[Number(value) as SecretPurpose] }),
  });
  const kind = facetFromRows({
    key: 'kind',
    label: 'Kind',
    icon: <ShapesIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => String(row.kind),
    describe: (value) => ({ label: kindLabels[Number(value) as keyof typeof kindLabels] }),
  });
  const status = facetFromRows({
    key: 'status',
    label: 'Status',
    icon: <ActivityIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => String(row.lifecycle),
    describe: (value) => {
      const lifecycle = Number(value) as CredentialLifecycle;
      return {
        label: credentialLifecycleLabels[lifecycle],
        icon: <StatusDot tone={credentialLifecycleTones[lifecycle]} />,
      };
    },
  });
  const origin = facetFromRows({
    key: 'origin',
    label: 'Origin',
    icon: <FingerprintIcon aria-hidden="true" />,
    rows: credentials,
    accessor: (row) => String(row.origin ?? 0),
    describe: (value) => ({ label: originLabels[Number(value) as keyof typeof originLabels] }),
  });
  return [
    ...(showVault ? [vault] : []),
    ...(showEnvironment ? [environment] : []),
    purpose,
    kind,
    status,
    origin,
  ].filter((facet) => facet.values.length > 0);
}

export function CredentialsTable({
  credentials,
  vaults,
  vaultId,
  canAdd = true,
  showEnvironmentFacet = true,
}: CredentialsTableProps) {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<FilterState>(emptyFilterState);

  const vaultNames = useMemo(() => new Map(vaults.map((vault) => [vault.vaultId, vault.displayName])), [vaults]);
  const environmentNames = useEnvironmentNames();
  const columns = useMemo(() => buildColumns(vaultNames, vaultId === undefined), [vaultNames, vaultId]);
  const facets = useMemo(
    () => buildFacets(credentials, vaultNames, environmentNames, vaultId === undefined, showEnvironmentFacet),
    [credentials, vaultNames, environmentNames, vaultId, showEnvironmentFacet],
  );
  const visible = useMemo(
    () => applyFilters(credentials, filters, facets, matchesText),
    [credentials, filters, facets],
  );
  const clear = () => setFilters(emptyFilterState);

  return (
    <DataTable
      data={visible}
      columns={columns}
      toolbar={
        <FilterBar
          label="Filter credentials"
          placeholder="Filter by name, reference, purpose, environment or status"
          rows={credentials}
          facets={facets}
          state={filters}
          onChange={setFilters}
          matchesText={matchesText}
        />
      }
      empty={
        credentials.length > 0 ? (
          <TableEmptyState title="No matches" message="No credentials match the current filters.">
            <Button type="button" variant="outline" size="sm" onClick={clear}>
              <XIcon data-icon="inline-start" />
              Clear filters
            </Button>
          </TableEmptyState>
        ) : (
          <TableEmptyState
            title="No credentials yet"
            message="Nothing is stored here. Add a credential to hold a secret behind an opaque reference."
          >
            {canAdd ? (
              <Link to="/secrets/new" search={{ vault: vaultId }} className={buttonVariants({ size: 'sm' })}>
                <PlusIcon data-icon="inline-start" />
                Add credential
              </Link>
            ) : null}
          </TableEmptyState>
        )
      }
      footer={<TableFooter shown={visible.length} total={credentials.length} noun="credential" />}
      muted={(credential) =>
        credential.lifecycle === CredentialLifecycle.DESTROYED || credential.lifecycle === CredentialLifecycle.REVOKED
      }
      onOpen={(credential) =>
        void navigate({ to: '/secrets/credentials/$secretRef', params: { secretRef: credential.secret } })
      }
    />
  );
}
