import { Link } from '@tanstack/react-router';
import { ChevronRightIcon } from 'lucide-react';
import { Fragment } from 'react';

import type { HierarchyView, NodeView } from '../domain/view';

export function Ancestry({ node, view }: { node: NodeView; view: HierarchyView }) {
  const ancestors = node.path
    .slice(0, -1)
    .filter((nodeId) => nodeId !== view.rootId)
    .map((nodeId) => view.byId.get(nodeId))
    .filter((ancestor) => ancestor !== undefined);
  const organization = view.organization.revealed ? view.organization.name : undefined;
  if (!organization && ancestors.length === 0) return null;

  return (
    <nav aria-label="Ancestry" className="text-muted-foreground min-w-0 text-xs">
      <ol className="flex min-w-0 flex-wrap items-center gap-1.5">
        {organization ? (
          <li className="flex items-center gap-1.5">
            <span>{organization}</span>
            <ChevronRightIcon className="size-3" aria-hidden="true" />
          </li>
        ) : null}
        {ancestors.map((ancestor) => (
          <Fragment key={ancestor.nodeId}>
            <li className="flex items-center gap-1.5">
              <Link
                to="/workspaces/$nodeId"
                params={{ nodeId: ancestor.nodeId }}
                className="hover:text-foreground transition-colors"
              >
                {ancestor.name}
              </Link>
              <ChevronRightIcon className="size-3" aria-hidden="true" />
            </li>
          </Fragment>
        ))}
        <li aria-current="page" className="text-foreground font-medium">
          {node.name}
        </li>
      </ol>
    </nav>
  );
}
