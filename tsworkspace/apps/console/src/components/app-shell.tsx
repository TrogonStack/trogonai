import { Link, useRouterState } from '@tanstack/react-router';
import {
  BirdIcon,
  FolderKanbanIcon,
  KeyRoundIcon,
  KeySquareIcon,
  LayersIcon,
  MonitorIcon,
  MoonIcon,
  NetworkIcon,
  PanelLeftIcon,
  PlugZapIcon,
  SunIcon,
  WebhookIcon,
  type LucideIcon,
} from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';

import { useTheme, type ThemePreference } from '@/components/theme-provider';
import { Button } from '@/components/ui/button';
import { Segmented, type SegmentedOption } from '@/components/ui/segmented';
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip';
import { useBackendQuery, useCustodyQuery } from '@/features/custody/queries';
import { useEnvironmentQuery, useEnvironmentsQuery } from '@/features/environments/queries';
import { useSourceQuery, useSourcesQuery } from '@/features/gateway-sources/queries';
import type { NodeId } from '@/features/hierarchy/domain/node-id';
import { useHierarchyView } from '@/features/hierarchy/queries';
import { ProjectSwitcher } from '@/features/projects/components/project-switcher';
import { useCurrentProject } from '@/features/projects/current-project';
import { useProjectQuery, useProjectsQuery } from '@/features/projects/queries';
import { useConnectionQuery, useConnectionsQuery } from '@/features/provider-connections/queries';
import { useCredentialQuery, useCredentialsQuery, useVaultQuery } from '@/features/secrets/queries';
import { cn } from '@/lib/utils';

const sidebarKey = 'trogonai-console-sidebar';

function readCollapsed(): boolean {
  try {
    return localStorage.getItem(sidebarKey) === 'collapsed';
  } catch {
    return false;
  }
}

const themeOptions: readonly SegmentedOption<ThemePreference>[] = [
  { value: 'light', label: 'Light', icon: <SunIcon /> },
  { value: 'system', label: 'System', icon: <MonitorIcon /> },
  { value: 'dark', label: 'Dark', icon: <MoonIcon /> },
];

const nextTheme: Record<ThemePreference, ThemePreference> = { light: 'dark', dark: 'system', system: 'light' };
const themeIcons: Record<ThemePreference, LucideIcon> = { light: SunIcon, dark: MoonIcon, system: MonitorIcon };

function ThemeToggle({ collapsed }: { collapsed: boolean }) {
  const { preference, setPreference } = useTheme();
  const CurrentIcon = themeIcons[preference];
  return (
    <>
      <div className={collapsed ? 'hidden' : 'hidden lg:block'}>
        <Segmented
          label="Theme"
          value={preference}
          options={themeOptions}
          onChange={setPreference}
          iconOnly
          size="sm"
        />
      </div>
      <div className={collapsed ? 'block' : 'lg:hidden'}>
        <Tooltip>
          <TooltipTrigger
            render={
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label={`Theme: ${preference}. Switch theme`}
                onClick={() => setPreference(nextTheme[preference])}
              />
            }
          >
            <CurrentIcon />
          </TooltipTrigger>
          <TooltipContent side="right">Theme: {preference}</TooltipContent>
        </Tooltip>
      </div>
    </>
  );
}

function NavCount({ collapsed, count }: { collapsed: boolean; count: number | undefined }) {
  if (count === undefined) return null;
  return (
    <span
      className={cn(
        'ml-auto rounded-sm border border-border bg-surface-deep/70 px-1.5 font-mono text-[10.5px] leading-4 text-muted-foreground tabular-nums',
        collapsed ? 'hidden' : 'hidden lg:inline',
      )}
    >
      {String(count).padStart(2, '0')}
    </span>
  );
}

function Sidebar({ collapsed }: { collapsed: boolean }) {
  const projects = useProjectsQuery();
  const hierarchy = useHierarchyView();
  const connections = useConnectionsQuery();
  const credentials = useCredentialsQuery();
  const environments = useEnvironmentsQuery();
  const custody = useCustodyQuery();
  const sources = useSourcesQuery();
  const label = collapsed ? 'hidden' : 'hidden lg:inline';
  const block = collapsed ? 'hidden' : 'hidden lg:flex';
  return (
    <aside
      className={cn(
        'sticky top-0 flex h-screen w-14 shrink-0 flex-col border-r border-sidebar-border bg-sidebar/95 text-sidebar-foreground backdrop-blur transition-[width] duration-200',
        !collapsed && 'lg:w-60',
      )}
    >
      <div className="border-sidebar-border flex h-14 items-center gap-2.5 border-b px-3">
        <span className="keycap-primary flex size-8 shrink-0 items-center justify-center rounded-md">
          <BirdIcon className="size-4" aria-hidden="true" />
        </span>
        <div className={cn('min-w-0 flex-col gap-0.5 leading-none', block)}>
          <span className="text-sm font-semibold tracking-tight">TrogonAi</span>
          <span className="micro text-muted-foreground text-[9.5px]">Console</span>
        </div>
      </div>
      <div className="border-sidebar-border border-b p-2">
        <ProjectSwitcher collapsed={collapsed} />
      </div>
      <nav aria-label="Primary" className="flex flex-1 flex-col gap-4 overflow-y-auto p-2">
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Workspace
          </p>
          <Link
            to="/workspaces"
            title="Workspaces"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <NetworkIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Workspaces</span>
            <NavCount collapsed={collapsed} count={hierarchy.view?.workspaces.length} />
          </Link>
          <Link
            to="/projects"
            title="Projects"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <FolderKanbanIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Projects</span>
            <NavCount collapsed={collapsed} count={projects.data?.length} />
          </Link>
        </div>
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Model access
          </p>
          <Link
            to="/providers"
            title="Providers"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <PlugZapIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Providers</span>
            <NavCount collapsed={collapsed} count={connections.data?.length} />
          </Link>
        </div>
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Secret management
          </p>
          <Link
            to="/secrets"
            title="Secrets"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <KeyRoundIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Secrets</span>
            <NavCount collapsed={collapsed} count={credentials.data?.length} />
          </Link>
        </div>
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Security
          </p>
          <Link
            to="/keys"
            title="Encryption keys"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <KeySquareIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Encryption keys</span>
            <NavCount collapsed={collapsed} count={custody.data?.stats.keys} />
          </Link>
        </div>
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Gateway
          </p>
          <Link
            to="/sources"
            title="Sources"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <WebhookIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Sources</span>
            <NavCount collapsed={collapsed} count={sources.data?.length} />
          </Link>
        </div>
        <div className="flex flex-col gap-1">
          <p className={cn('micro flex items-center gap-2 px-2.5 pt-2 pb-1 text-[10px] text-muted-foreground', label)}>
            <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
            Project
          </p>
          <Link
            to="/environments"
            title="Environments"
            className="group/nav text-sidebar-foreground hover:bg-foreground/5 focus-visible:ring-sidebar-ring/70 data-[status=active]:bg-sidebar-accent data-[status=active]:text-sidebar-accent-foreground relative flex h-9 items-center gap-2.5 rounded-md px-2.5 text-[13px] font-medium transition-colors outline-none focus-visible:ring-2"
          >
            <span
              aria-hidden="true"
              className="bg-primary absolute top-1.5 bottom-1.5 -left-2 w-[3px] rounded-r-full opacity-0 shadow-[0_0_10px_1px_var(--primary)] transition-opacity group-data-[status=active]/nav:opacity-100"
            />
            <LayersIcon className="size-4 shrink-0" aria-hidden="true" />
            <span className={label}>Environments</span>
            <NavCount collapsed={collapsed} count={environments.data?.length} />
          </Link>
        </div>
      </nav>
      <div className="border-sidebar-border flex flex-col gap-3 border-t p-2">
        <div
          className={cn(
            'well flex items-center gap-2 rounded-md border border-border px-2 py-1.5',
            collapsed ? 'justify-center' : 'justify-center lg:justify-start',
          )}
          role="status"
        >
          <span aria-hidden="true" className="led size-1.5 shrink-0 rounded-full [--led:var(--success)]" />
          <span className={cn('micro text-[9.5px] text-muted-foreground', label)}>System nominal</span>
          <span className="sr-only">System nominal</span>
        </div>
        <div className={cn('flex items-center', collapsed ? 'justify-center' : 'justify-center lg:justify-between')}>
          <ThemeToggle collapsed={collapsed} />
        </div>
        <div className="flex items-center gap-2.5 px-1 py-1">
          <span
            aria-hidden="true"
            className="avatar-tint flex size-7 shrink-0 items-center justify-center rounded-md font-mono text-[10.5px] font-bold [--avatar-hue:125]"
          >
            CO
          </span>
          <div className={cn('min-w-0 flex-col leading-tight', block)}>
            <span className="truncate text-[13px] font-medium">console-operator</span>
            <span className="micro text-muted-foreground text-[9.5px]">Operator</span>
          </div>
        </div>
      </div>
    </aside>
  );
}

interface Crumb {
  readonly label: string;
  readonly to?: string;
}

function useProjectCrumb(): Crumb[] {
  const { project } = useCurrentProject();
  return project ? [{ label: project.displayName, to: `/projects/${encodeURIComponent(project.projectId)}` }] : [];
}

function ProjectCrumbs({ projectId }: { projectId: string }) {
  const query = useProjectQuery(projectId);
  const name = query.data ? query.data.displayName : projectId;
  return <Breadcrumbs crumbs={[{ label: 'Projects', to: '/projects' }, { label: name }]} />;
}

function WorkspaceCrumbs({ nodeId, creating }: { nodeId?: string; creating?: boolean }) {
  const { view } = useHierarchyView();
  const organization = view?.organization.revealed && view.organization.name ? [{ label: view.organization.name }] : [];
  const node = nodeId ? view?.byId.get(nodeId as NodeId) : undefined;
  const crumbs: Crumb[] = creating
    ? [{ label: 'Workspaces', to: '/workspaces' }, { label: 'New workspace' }]
    : nodeId
      ? [{ label: 'Workspaces', to: '/workspaces' }, { label: node?.name ?? nodeId }]
      : [{ label: 'Workspaces' }];
  return <Breadcrumbs crumbs={[...organization, ...crumbs]} />;
}

function ScopedCrumbs({ crumbs }: { crumbs: readonly Crumb[] }) {
  const project = useProjectCrumb();
  return <Breadcrumbs crumbs={[...project, ...crumbs]} />;
}

function ConnectionCrumbs({ connectionId }: { connectionId: string }) {
  const query = useConnectionQuery(connectionId);
  const name = query.data ? query.data.displayName || query.data.id : connectionId;
  return <Breadcrumbs crumbs={[{ label: 'Providers', to: '/providers' }, { label: name }]} />;
}

function VaultCrumbs({ vaultId }: { vaultId: string }) {
  const query = useVaultQuery(vaultId);
  const name = query.data ? query.data.displayName : vaultId;
  return <ScopedCrumbs crumbs={[{ label: 'Secrets', to: '/secrets' }, { label: name }]} />;
}

function EnvironmentCrumbs({ environmentId }: { environmentId: string }) {
  const query = useEnvironmentQuery(environmentId);
  const name = query.data ? query.data.displayName : environmentId;
  return <ScopedCrumbs crumbs={[{ label: 'Environments', to: '/environments' }, { label: name }]} />;
}

function CredentialCrumbs({ secretRef }: { secretRef: string }) {
  const query = useCredentialQuery(secretRef);
  const name = query.data ? query.data.displayName : secretRef;
  return <ScopedCrumbs crumbs={[{ label: 'Secrets', to: '/secrets' }, { label: name }]} />;
}

function BackendCrumb({ backendId }: { backendId: string }) {
  const query = useBackendQuery(backendId);
  const name = query.data ? query.data.displayName : backendId;
  return (
    <Breadcrumbs
      crumbs={[{ label: 'Encryption keys', to: '/keys' }, { label: 'Backends', to: '/keys/backends' }, { label: name }]}
    />
  );
}

function KeyCrumb({ keyRef }: { keyRef: string }) {
  return (
    <Breadcrumbs
      crumbs={[{ label: 'Encryption keys', to: '/keys' }, { label: 'Keys', to: '/keys/keys' }, { label: keyRef }]}
    />
  );
}

function KeysCrumbs({ segments }: { segments: readonly string[] }) {
  if (segments.length === 1) return <Breadcrumbs crumbs={[{ label: 'Encryption keys' }]} />;
  if (segments[1] === 'backends') {
    if (segments.length === 2) {
      return <Breadcrumbs crumbs={[{ label: 'Encryption keys', to: '/keys' }, { label: 'Backends' }]} />;
    }
    if (segments[2] === 'new') {
      return (
        <Breadcrumbs
          crumbs={[
            { label: 'Encryption keys', to: '/keys' },
            { label: 'Backends', to: '/keys/backends' },
            { label: 'Register a backend' },
          ]}
        />
      );
    }
    return <BackendCrumb backendId={decodeURIComponent(segments[2]!)} />;
  }
  if (segments[1] === 'keys') {
    if (segments.length === 2) {
      return <Breadcrumbs crumbs={[{ label: 'Encryption keys', to: '/keys' }, { label: 'Keys' }]} />;
    }
    if (segments[2] === 'new') {
      return (
        <Breadcrumbs
          crumbs={[
            { label: 'Encryption keys', to: '/keys' },
            { label: 'Keys', to: '/keys/keys' },
            { label: 'Bind a key' },
          ]}
        />
      );
    }
    return <KeyCrumb keyRef={decodeURIComponent(segments[2]!)} />;
  }
  if (segments[1] === 'migrations') {
    if (segments.length === 2) {
      return <Breadcrumbs crumbs={[{ label: 'Encryption keys', to: '/keys' }, { label: 'Migrations' }]} />;
    }
    return (
      <Breadcrumbs
        crumbs={[
          { label: 'Encryption keys', to: '/keys' },
          { label: 'Migrations', to: '/keys/migrations' },
          { label: decodeURIComponent(segments[2]!) },
        ]}
      />
    );
  }
  return <Breadcrumbs crumbs={[{ label: 'Encryption keys' }]} />;
}

function SourceCrumbs({ integrationId }: { integrationId: string }) {
  const query = useSourceQuery(integrationId);
  const name = query.data ? query.data.displayName : integrationId;
  return <ScopedCrumbs crumbs={[{ label: 'Sources', to: '/sources' }, { label: name }]} />;
}

function Breadcrumbs({ crumbs }: { crumbs: readonly Crumb[] }) {
  return (
    <nav aria-label="Breadcrumb" className="min-w-0">
      <ol className="flex min-w-0 items-center gap-1.5 text-[13px]">
        {crumbs.map((crumb, index) => {
          const last = index === crumbs.length - 1;
          return (
            <li key={`${index}:${crumb.label}`} className="flex min-w-0 items-center gap-1.5">
              {index > 0 ? (
                <span aria-hidden="true" className="text-muted-foreground/60 font-mono">
                  /
                </span>
              ) : null}
              {crumb.to && !last ? (
                <Link to={crumb.to} className="text-muted-foreground hover:text-foreground transition-colors">
                  {crumb.label}
                </Link>
              ) : (
                <span aria-current={last ? 'page' : undefined} className="truncate font-medium">
                  {crumb.label}
                </span>
              )}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

function RouteBreadcrumbs() {
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const segments = pathname.split('/').filter(Boolean);
  if (segments[0] === 'projects') {
    if (segments.length === 1) return <Breadcrumbs crumbs={[{ label: 'Projects' }]} />;
    if (segments[1] === 'new') {
      return <Breadcrumbs crumbs={[{ label: 'Projects', to: '/projects' }, { label: 'New project' }]} />;
    }
    return <ProjectCrumbs projectId={decodeURIComponent(segments[1]!)} />;
  }
  if (segments[0] === 'workspaces') {
    if (segments[1] === 'new') return <WorkspaceCrumbs creating />;
    if (segments[1]) return <WorkspaceCrumbs nodeId={decodeURIComponent(segments[1])} />;
    return <WorkspaceCrumbs />;
  }
  if (segments[0] === 'secrets') {
    if (segments.length === 1) return <ScopedCrumbs crumbs={[{ label: 'Secrets' }]} />;
    if (segments[1] === 'new') {
      return <ScopedCrumbs crumbs={[{ label: 'Secrets', to: '/secrets' }, { label: 'Add credential' }]} />;
    }
    if (segments[1] === 'vaults' && segments[2]) return <VaultCrumbs vaultId={decodeURIComponent(segments[2])} />;
    if (segments[1] === 'credentials' && segments[2]) {
      return <CredentialCrumbs secretRef={decodeURIComponent(segments[2])} />;
    }
    return <ScopedCrumbs crumbs={[{ label: 'Secrets' }]} />;
  }
  if (segments[0] === 'sources') {
    if (segments[1]) return <SourceCrumbs integrationId={decodeURIComponent(segments[1])} />;
    return <ScopedCrumbs crumbs={[{ label: 'Sources' }]} />;
  }
  if (segments[0] === 'keys') return <KeysCrumbs segments={segments} />;
  if (segments[0] === 'environments') {
    if (segments[1]) return <EnvironmentCrumbs environmentId={decodeURIComponent(segments[1])} />;
    return <ScopedCrumbs crumbs={[{ label: 'Environments' }]} />;
  }
  if (segments[0] !== 'providers') return <Breadcrumbs crumbs={[{ label: 'Console' }]} />;
  if (segments.length === 1) return <Breadcrumbs crumbs={[{ label: 'Providers' }]} />;
  if (segments[1] === 'new') {
    return <Breadcrumbs crumbs={[{ label: 'Providers', to: '/providers' }, { label: 'New connection' }]} />;
  }
  return <ConnectionCrumbs connectionId={decodeURIComponent(segments[1]!)} />;
}

export function AppShell({ children }: { children: ReactNode }) {
  const [collapsed, setCollapsed] = useState(readCollapsed);

  useEffect(() => {
    try {
      localStorage.setItem(sidebarKey, collapsed ? 'collapsed' : 'expanded');
    } catch {
      return;
    }
  }, [collapsed]);

  return (
    <div className="bg-background text-foreground flex min-h-screen">
      <a
        href="#main"
        className="bg-card sr-only z-50 rounded-md px-3 py-1.5 text-[13px] shadow-md focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <Sidebar collapsed={collapsed} />
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="border-border bg-background/80 sticky top-0 z-30 flex h-12 shrink-0 items-center gap-3 border-b px-4 backdrop-blur-md">
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="text-muted-foreground hidden lg:inline-flex"
            aria-label={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
            aria-pressed={collapsed}
            onClick={() => setCollapsed((current) => !current)}
          >
            <PanelLeftIcon />
          </Button>
          <RouteBreadcrumbs />
          <div className="ml-auto flex items-center gap-2">
            <span className="micro border-border text-muted-foreground inline-flex h-6 items-center gap-2 rounded-sm border px-2 text-[10px]">
              <span className="led led-blink size-1.5 rounded-full [--led:var(--warning)]" aria-hidden="true" />
              In-memory fake
            </span>
          </div>
        </header>
        <main id="main" className="mx-auto w-full max-w-6xl min-w-0 flex-1 px-6 py-6">
          {children}
        </main>
      </div>
    </div>
  );
}
