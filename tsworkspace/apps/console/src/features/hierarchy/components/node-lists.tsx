import { Link } from '@tanstack/react-router';
import { FolderIcon, FolderKanbanIcon } from 'lucide-react';
import type { ReactNode } from 'react';

import type { ProjectView } from '@/features/projects/domain/view';

import type { HierarchyView, NodeView } from '../domain/view';

function SectionHeading({ children }: { children: ReactNode }) {
  return (
    <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
      <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
      {children}
    </h2>
  );
}

const rowClass =
  'border-border hover:bg-foreground/[0.03] flex min-w-0 items-center gap-3 rounded-lg border px-3 py-2 text-[13px] outline-none focus-visible:underline';

export function ChildFolders({ node, view, action }: { node: NodeView; view: HierarchyView; action: ReactNode }) {
  const children = node.children.map((nodeId) => view.byId.get(nodeId)).filter((child) => child !== undefined);
  return (
    <section aria-label="Folders" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <SectionHeading>Folders</SectionHeading>
        {action}
      </div>
      {children.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {children.map((child) => (
            <li key={child.nodeId}>
              <Link to="/workspaces/$nodeId" params={{ nodeId: child.nodeId }} className={rowClass}>
                <FolderIcon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
                <span className="truncate font-medium">{child.name}</span>
                <span className="text-muted-foreground ml-auto font-mono text-[11px]">{child.nodeId}</span>
              </Link>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground border-border rounded-lg border border-dashed p-6 text-center text-sm">
          No folders here yet.
        </p>
      )}
    </section>
  );
}

export function AttachedProjects({ projects }: { projects: readonly ProjectView[] }) {
  return (
    <section aria-label="Projects" className="flex flex-col gap-3">
      <SectionHeading>Projects</SectionHeading>
      {projects.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {projects.map((project) => (
            <li key={project.projectId}>
              <Link to="/projects/$projectId" params={{ projectId: project.projectId }} className={rowClass}>
                <FolderKanbanIcon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
                <span className="truncate font-medium">{project.displayName}</span>
                <span className="text-muted-foreground ml-auto font-mono text-[11px]">{project.projectId}</span>
              </Link>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground border-border rounded-lg border border-dashed p-6 text-center text-sm">
          No project is attached here.
        </p>
      )}
    </section>
  );
}
