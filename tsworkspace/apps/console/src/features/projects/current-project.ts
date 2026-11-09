import { useSearch } from '@tanstack/react-router';
import { useMemo } from 'react';

import type { ProjectId } from './domain/project-id';
import { resolveCurrentProject, type ProjectView } from './domain/view';
import { useProjectsQuery } from './queries';

export interface CurrentProject {
  readonly project: ProjectView | undefined;
  readonly projects: readonly ProjectView[] | undefined;
  readonly requested: string | undefined;
  readonly error: unknown;
}

export function useCurrentProject(): CurrentProject {
  const requested = useSearch({ strict: false, select: (search) => search.project });
  const query = useProjectsQuery();
  const project = useMemo(
    () => (query.data ? resolveCurrentProject(query.data, requested) : undefined),
    [query.data, requested],
  );
  return { project, projects: query.data, requested, error: query.error };
}

export function useCurrentProjectId(): ProjectId | undefined {
  return useCurrentProject().project?.projectId;
}

export function unresolvedClient<T extends object>(): T {
  return new Proxy({} as T, {
    get: () => () => Promise.reject(new Error('No project is selected yet.')),
  });
}
