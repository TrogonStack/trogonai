import { Link } from '@tanstack/react-router';
import { CredentialLifecycle, VaultLifecycle } from '@trogonai/proto';
import type { ReactNode } from 'react';

import { StatusPill } from '@/components/lifecycle-badge';
import { useEnvironmentNames } from '@/features/environments/queries';
import { cn } from '@/lib/utils';

import {
  credentialLifecycleLabels,
  credentialLifecycleTones,
  vaultLifecycleLabels,
  vaultLifecycleTones,
} from '../domain/labels';

export function CredentialLifecycleBadge({
  lifecycle,
  className,
}: {
  lifecycle: CredentialLifecycle;
  className?: string;
}) {
  return (
    <StatusPill
      tone={credentialLifecycleTones[lifecycle]}
      blink={lifecycle === CredentialLifecycle.PROVISIONING || lifecycle === CredentialLifecycle.RECONNECT_REQUIRED}
      sweep={lifecycle === CredentialLifecycle.PROVISIONING}
      struck={lifecycle === CredentialLifecycle.REVOKED || lifecycle === CredentialLifecycle.DESTROYED}
      label={credentialLifecycleLabels[lifecycle]}
      className={className}
    />
  );
}

export function VaultLifecycleBadge({ lifecycle }: { lifecycle: VaultLifecycle }) {
  return (
    <StatusPill
      tone={vaultLifecycleTones[lifecycle]}
      blink={lifecycle === VaultLifecycle.DESTROY_PENDING}
      struck={lifecycle === VaultLifecycle.DESTROYED}
      label={vaultLifecycleLabels[lifecycle]}
    />
  );
}

export function MetaChip({
  children,
  title,
  mono,
  className,
}: {
  children: ReactNode;
  title?: string;
  mono?: boolean;
  className?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        'micro border-border text-muted-foreground inline-flex h-5 w-fit items-center rounded-sm border border-dashed px-1.5 text-[10px] whitespace-nowrap',
        mono && 'font-mono normal-case',
        className,
      )}
    >
      {children}
    </span>
  );
}

export function EnvironmentChip({
  environment,
  linked = false,
  className,
}: {
  environment: string;
  linked?: boolean;
  className?: string;
}) {
  const names = useEnvironmentNames();
  const label = names.get(environment) ?? environment;
  if (!linked) {
    return (
      <MetaChip title="Environment" className={className}>
        {label}
      </MetaChip>
    );
  }
  return (
    <Link
      to="/environments/$environmentId"
      params={{ environmentId: environment }}
      title={`Environment: ${label}`}
      className={cn(
        'micro border-border text-muted-foreground hover:text-foreground focus-visible:ring-ring/70 inline-flex h-5 w-fit items-center rounded-sm border border-dashed px-1.5 text-[10px] whitespace-nowrap outline-none focus-visible:ring-2',
        className,
      )}
    >
      {label}
    </Link>
  );
}
