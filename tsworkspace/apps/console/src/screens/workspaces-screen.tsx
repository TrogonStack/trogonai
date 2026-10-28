import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { WorkspacesTable, WorkspacesTableSkeleton } from '@/features/hierarchy/components/workspaces-table';
import { useHierarchyView } from '@/features/hierarchy/queries';

function NewWorkspaceLink() {
  return (
    <Link to="/workspaces/new" className={buttonVariants({ size: 'lg' })}>
      <PlusIcon data-icon="inline-start" />
      New workspace
    </Link>
  );
}

export function WorkspacesScreen() {
  const { view, error } = useHierarchyView();
  const organization = view?.organization.revealed ? view.organization.name : undefined;

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow={organization ? `Organization / ${organization}` : 'Workspace'}
        title="Workspaces"
        description="Workspaces group folders and projects. Folders nest below them, and the words follow the depth in the tree."
        actions={<NewWorkspaceLink />}
      />
      {error ? <FaultAlert error={error} title="Could not load workspaces" /> : null}
      {view ? (
        <WorkspacesTable workspaces={view.workspaces} emptyAction={<NewWorkspaceLink />} />
      ) : error ? null : (
        <WorkspacesTableSkeleton />
      )}
    </div>
  );
}
