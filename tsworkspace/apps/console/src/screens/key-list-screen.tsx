import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { KeysSubnav } from '@/features/custody/components/keys-subnav';
import { KeysTable, KeysTableSkeleton } from '@/features/custody/components/keys-table';
import { useCustodyQuery } from '@/features/custody/queries';

export function KeyListScreen() {
  const query = useCustodyQuery();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Security"
        title="Bound keys"
        description="Opaque key references and what they point to. Managed keys have no key identity, so they are not listed."
        actions={
          <Link to="/keys/keys/new" className={buttonVariants({ size: 'lg' })}>
            <PlusIcon data-icon="inline-start" />
            Bind a key
          </Link>
        }
      />
      <KeysSubnav />
      {query.error ? <FaultAlert error={query.error} title="Could not load keys" /> : null}
      {query.data ? <KeysTable keys={query.data.keys} /> : query.error ? null : <KeysTableSkeleton />}
    </div>
  );
}
