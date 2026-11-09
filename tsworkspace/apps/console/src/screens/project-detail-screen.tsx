import { BackLink } from '@/components/back-link';
import { DetailHeader } from '@/components/detail-header';
import { FaultAlert } from '@/components/fault-alert';
import { NotFoundState } from '@/components/not-found-state';
import { Skeleton } from '@/components/ui/skeleton';
import { GenesisFacts } from '@/features/projects/components/genesis-facts';
import { ProjectLifecycleBadge } from '@/features/projects/components/lifecycle-badge';
import { OwnedResources } from '@/features/projects/components/owned-resources';
import { ProjectActions } from '@/features/projects/components/project-actions';
import { ProjectTimeline } from '@/features/projects/components/project-timeline';
import { useProjectHistoryQuery, useProjectQuery } from '@/features/projects/queries';

function DetailSkeleton() {
  return (
    <div role="status" aria-busy="true" className="flex flex-col gap-6">
      <span className="sr-only">Loading project</span>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-6 w-64" />
        <Skeleton className="h-3 w-40" />
      </div>
      <Skeleton className="h-32 w-full" />
    </div>
  );
}

export function ProjectDetailScreen({ projectId }: { projectId: string }) {
  const project = useProjectQuery(projectId);
  const history = useProjectHistoryQuery(projectId);

  const back = <BackLink to="/projects">All projects</BackLink>;

  if (project.isError) return <FaultAlert error={project.error} title="Could not load the project" />;
  if (project.data === undefined) return <DetailSkeleton />;
  if (project.data === null) {
    return (
      <NotFoundState back={back} title="Project not found">
        No project has the id <span className="font-mono">{projectId}</span>.
      </NotFoundState>
    );
  }
  const view = project.data;

  return (
    <div className="flex flex-col gap-6">
      <div>{back}</div>
      <DetailHeader
        title={view.displayName}
        status={<ProjectLifecycleBadge lifecycle={view.lifecycle} />}
        identifier={<span className="font-mono">{view.projectId}</span>}
        copyValue={view.projectId}
        copyLabel="project id"
        facts={[`Revision ${view.revision}`]}
        actions={<ProjectActions project={view} />}
      />
      <section aria-label="Goal" className="flex flex-col gap-1.5">
        <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
          <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
          Goal
        </h2>
        <p className="max-w-prose text-sm">{view.goal}</p>
      </section>
      <GenesisFacts project={view} />
      <OwnedResources projectId={view.projectId} />
      <section aria-label="Activity" className="flex flex-col gap-3">
        <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
          <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
          Activity
        </h2>
        {history.data ? <ProjectTimeline events={history.data} /> : <Skeleton className="h-24 w-full" />}
      </section>
    </div>
  );
}
