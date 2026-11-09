import {
  CredentialBindingLifecycle,
  ProviderAccessScope,
  ProviderConnectionLifecycle,
  ProviderRoutingEligibility,
} from '@trogonai/proto';
import type { ReactNode } from 'react';

import {
  accessScopeLabels,
  bindingLifecycleLabels,
  connectionLifecycleLabels,
  eligibilityLabels,
} from '@/features/provider-connections/domain/labels';
import { cn } from '@/lib/utils';

export type Tone = 'success' | 'warning' | 'info' | 'danger' | 'neutral';

const ledColor: Record<Tone, string> = {
  success: '[--led:var(--success)]',
  warning: '[--led:var(--warning)]',
  info: '[--led:var(--info)]',
  danger: '[--led:var(--danger)]',
  neutral: '[--led:var(--muted-foreground)]',
};

const panelTones: Record<Tone, string> = {
  success: 'border-success/35 bg-success-soft text-success-ink',
  warning: 'border-warning/35 bg-warning-soft text-warning-ink',
  info: 'border-info/35 bg-info-soft text-info-ink',
  danger: 'border-danger/30 bg-danger-soft text-danger-ink',
  neutral: 'border-border bg-muted text-muted-foreground',
};

export function toneVariable(tone: Tone): string {
  return ledColor[tone];
}

export function StatusDot({
  tone,
  blink,
  dim,
  className,
}: {
  tone: Tone;
  blink?: boolean;
  dim?: boolean;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'size-1.5 shrink-0 rounded-full',
        ledColor[tone],
        dim ? 'led-dim' : 'led',
        blink && 'led-blink',
        className,
      )}
    />
  );
}

export function StatusPill({
  tone,
  blink,
  sweep,
  struck,
  label,
  className,
}: {
  tone: Tone;
  blink?: boolean;
  sweep?: boolean;
  struck?: boolean;
  label: string;
  className?: string;
}) {
  return (
    <span
      className={cn(
        'micro inline-flex h-5 w-fit shrink-0 items-center gap-1.5 rounded-sm border px-1.5 text-[10px] whitespace-nowrap',
        panelTones[tone],
        ledColor[tone],
        struck && 'opacity-80',
        className,
      )}
    >
      <StatusDot tone={tone} blink={blink} dim={struck} />
      <span className={cn(struck && 'line-through decoration-current/50')}>{label}</span>
      {sweep ? (
        <span aria-hidden="true" className="ml-0.5 h-[3px] w-4 overflow-hidden rounded-full bg-current/15">
          <span className="led-sweep block size-full" />
        </span>
      ) : null}
    </span>
  );
}

const connectionTones: Record<ProviderConnectionLifecycle, Tone> = {
  [ProviderConnectionLifecycle.UNSPECIFIED]: 'neutral',
  [ProviderConnectionLifecycle.PENDING]: 'warning',
  [ProviderConnectionLifecycle.ACTIVE]: 'success',
  [ProviderConnectionLifecycle.REVOKED]: 'danger',
};

const bindingTones: Record<CredentialBindingLifecycle, Tone> = {
  [CredentialBindingLifecycle.UNSPECIFIED]: 'neutral',
  [CredentialBindingLifecycle.PENDING]: 'warning',
  [CredentialBindingLifecycle.ACTIVE]: 'success',
  [CredentialBindingLifecycle.DRAINING]: 'info',
  [CredentialBindingLifecycle.REVOKED]: 'danger',
};

export function connectionTone(lifecycle: ProviderConnectionLifecycle): Tone {
  return connectionTones[lifecycle];
}

export function bindingTone(lifecycle: CredentialBindingLifecycle): Tone {
  return bindingTones[lifecycle];
}

export function ConnectionLifecycleBadge({ lifecycle }: { lifecycle: ProviderConnectionLifecycle }) {
  return (
    <StatusPill
      tone={connectionTones[lifecycle]}
      blink={lifecycle === ProviderConnectionLifecycle.PENDING}
      struck={lifecycle === ProviderConnectionLifecycle.REVOKED}
      label={connectionLifecycleLabels[lifecycle]}
    />
  );
}

export function BindingLifecycleBadge({ lifecycle }: { lifecycle: CredentialBindingLifecycle }) {
  return (
    <StatusPill
      tone={bindingTones[lifecycle]}
      blink={lifecycle === CredentialBindingLifecycle.PENDING}
      sweep={lifecycle === CredentialBindingLifecycle.DRAINING}
      struck={lifecycle === CredentialBindingLifecycle.REVOKED}
      label={bindingLifecycleLabels[lifecycle]}
    />
  );
}

export function EligibilityIndicator({ eligibility }: { eligibility: ProviderRoutingEligibility }) {
  const enabled = eligibility === ProviderRoutingEligibility.ENABLED;
  return (
    <span className={cn('micro inline-flex items-center gap-1.5 text-[10px]', !enabled && 'text-muted-foreground')}>
      <StatusDot tone={enabled ? 'success' : 'neutral'} />
      {eligibilityLabels[eligibility]}
    </span>
  );
}

export function ScopeChip({ scope, className }: { scope: ProviderAccessScope; className?: string }) {
  return (
    <span
      className={cn(
        'micro inline-flex h-5 w-fit items-center rounded-sm border border-dashed border-border px-1.5 text-[10px] whitespace-nowrap text-muted-foreground',
        className,
      )}
    >
      {accessScopeLabels[scope]}
    </span>
  );
}

export function Chip({ children, className, title }: { children: ReactNode; className?: string; title?: string }) {
  return (
    <span
      title={title}
      className={cn(
        'inline-flex h-5 w-fit max-w-full items-center rounded-sm border border-border bg-surface-deep/60 px-1.5 text-xs font-medium whitespace-nowrap text-foreground/80',
        className,
      )}
    >
      {children}
    </span>
  );
}
