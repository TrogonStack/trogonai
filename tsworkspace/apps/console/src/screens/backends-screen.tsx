import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { BackendsTable, BackendsTableSkeleton } from '@/features/custody/components/backends-table';
import { KeysSubnav } from '@/features/custody/components/keys-subnav';
import { useCustodyQuery } from '@/features/custody/queries';

export function BackendsScreen() {
  const query = useCustodyQuery();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Security"
        title="Key backends"
        description="The trust boundaries the platform is allowed to reach to wrap and unwrap keys. A backend holds no key until one is bound."
        actions={
          <Link to="/keys/backends/new" className={buttonVariants({ size: 'lg' })}>
            <PlusIcon data-icon="inline-start" />
            Register a backend
          </Link>
        }
      />
      <KeysSubnav />
      {query.error ? <FaultAlert error={query.error} title="Could not load backends" /> : null}
      {query.data ? <BackendsTable backends={query.data.backends} /> : query.error ? null : <BackendsTableSkeleton />}
    </div>
  );
}
