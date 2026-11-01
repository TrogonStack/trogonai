import { Link, useNavigate, useRouterState } from '@tanstack/react-router';
import { CheckIcon, ChevronsUpDownIcon, FolderKanbanIcon, PlusIcon } from 'lucide-react';
import { useState } from 'react';

import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { cn } from '@/lib/utils';

import { useCurrentProject } from '../current-project';
import type { ProjectId } from '../domain/project-id';
import { activeFirst } from '../domain/view';

const scopedSections = { secrets: '/secrets', environments: '/environments', sources: '/sources' } as const;

export function ProjectSwitcher({ collapsed }: { collapsed: boolean }) {
  const navigate = useNavigate();
  const section = useRouterState({ select: (state) => state.location.pathname.split('/').filter(Boolean)[0] });
  const { project, projects } = useCurrentProject();
  const [open, setOpen] = useState(false);

  const name = project?.displayName ?? 'Loading project';
  const label = collapsed ? 'hidden' : 'hidden lg:flex';
  const to = section === 'secrets' || section === 'environments' || section === 'sources' ? scopedSections[section] : '.';

  const choose = (projectId: ProjectId) => {
    setOpen(false);
    void navigate({ to, search: (previous) => ({ ...previous, project: projectId }) });
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        aria-label={`Project: ${name}. Switch project`}
        title={name}
        className="border-sidebar-border bg-surface-deep/60 hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 flex h-10 w-full items-center gap-2.5 rounded-md border px-2.5 text-left transition-colors outline-none focus-visible:ring-2"
      >
        <FolderKanbanIcon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
        <span className={cn('min-w-0 flex-1 flex-col leading-tight', label)}>
          <span className="micro text-muted-foreground text-[9.5px]">Project</span>
          <span className="truncate text-[13px] font-medium">{name}</span>
        </span>
        <ChevronsUpDownIcon
          className={cn('text-muted-foreground size-3.5 shrink-0', collapsed ? 'hidden' : 'hidden lg:block')}
          aria-hidden="true"
        />
      </PopoverTrigger>
      <PopoverContent align="start" side="bottom" className="w-64">
        <div className="flex flex-col gap-1 p-1.5">
          <p className="micro text-muted-foreground px-2 py-1 text-[10px]">Switch project</p>
          <ul aria-label="Projects" className="flex max-h-64 flex-col gap-0.5 overflow-y-auto">
            {activeFirst(projects ?? []).map((candidate) => (
              <li key={candidate.projectId}>
                <button
                  type="button"
                  aria-current={candidate.projectId === project?.projectId ? 'true' : undefined}
                  className={cn(
                    'hover:bg-muted focus-visible:ring-ring/70 flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-[13px] outline-none focus-visible:ring-2',
                    candidate.archived && 'text-muted-foreground',
                  )}
                  onClick={() => choose(candidate.projectId)}
                >
                  <span className="min-w-0 flex-1 truncate">{candidate.displayName}</span>
                  {candidate.archived ? (
                    <span className="micro border-border rounded-sm border px-1.5 text-[9.5px]">Archived</span>
                  ) : null}
                  {candidate.projectId === project?.projectId ? (
                    <CheckIcon className="size-3.5 shrink-0" aria-hidden="true" />
                  ) : null}
                </button>
              </li>
            ))}
          </ul>
          <div className="border-border mt-1 flex flex-col gap-0.5 border-t pt-1.5">
            <Link
              to="/projects"
              onClick={() => setOpen(false)}
              className="hover:bg-muted focus-visible:ring-ring/70 rounded-sm px-2 py-1.5 text-[13px] outline-none focus-visible:ring-2"
            >
              All projects
            </Link>
            <Link
              to="/projects/new"
              onClick={() => setOpen(false)}
              className="hover:bg-muted focus-visible:ring-ring/70 flex items-center gap-2 rounded-sm px-2 py-1.5 text-[13px] outline-none focus-visible:ring-2"
            >
              <PlusIcon className="size-3.5" aria-hidden="true" />
              New project
            </Link>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
