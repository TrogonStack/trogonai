import { Link } from '@tanstack/react-router';
import { PlusIcon } from 'lucide-react';

import { FaultAlert } from '@/components/fault-alert';
import { PageHeader } from '@/components/page-header';
import { buttonVariants } from '@/components/ui/button';
import { ProjectsTable, ProjectsTableSkeleton } from '@/features/projects/components/projects-table';
import { useProjectsQuery } from '@/features/projects/queries';

function NewProjectLink() {
  return (
    <Link to="/projects/new" className={buttonVariants({ size: 'lg' })}>
      <PlusIcon data-icon="inline-start" />
      New project
    </Link>
  );
}

export function ProjectsScreen() {
  const projects = useProjectsQuery();

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        eyebrow="Workspace"
        title="Projects"
        description="A project owns the vaults, credentials and environments of one piece of work. Pick one in the sidebar to see only its resources."
        actions={<NewProjectLink />}
      />
      {projects.isError ? <FaultAlert error={projects.error} title="Could not load projects" /> : null}
      {projects.data ? (
        <ProjectsTable projects={projects.data} emptyAction={<NewProjectLink />} />
      ) : projects.isError ? null : (
        <ProjectsTableSkeleton />
      )}
    </div>
  );
}
