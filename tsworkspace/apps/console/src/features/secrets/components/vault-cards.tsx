import { Link } from '@tanstack/react-router';
import { VaultIcon } from 'lucide-react';

import { StatusDot, toneVariable } from '@/components/lifecycle-badge';
import { RegistrationMarks, SpotlightSurface } from '@/components/spotlight-card';
import { Skeleton } from '@/components/ui/skeleton';
import { cn } from '@/lib/utils';

import { credentialLifecycleLabels, credentialLifecycleTones } from '../domain/labels';
import type { VaultView } from '../domain/view';
import { EnvironmentChip, VaultLifecycleBadge } from './badges';
import { VaultCustodyBadge } from './vault-custody-badge';

function VaultCard({ vault }: { vault: VaultView }) {
  const counts = [...vault.byLifecycle.entries()].sort((left, right) => right[1] - left[1]);
  return (
    <SpotlightSurface className="flex flex-col gap-4 p-4">
      <RegistrationMarks />
      <div className="flex items-start justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2.5">
          <span className="keycap flex size-8 shrink-0 items-center justify-center rounded-md">
            <VaultIcon className="size-4" aria-hidden="true" />
          </span>
          <div className="flex min-w-0 flex-col">
            <Link
              to="/secrets/vaults/$vaultId"
              params={{ vaultId: vault.vaultId }}
              className="truncate text-sm leading-5 font-medium outline-none after:absolute after:inset-0 hover:underline focus-visible:underline"
            >
              {vault.displayName}
            </Link>
            <span className="text-muted-foreground truncate font-mono text-[11px] leading-4">{vault.vaultId}</span>
          </div>
        </div>
        <VaultLifecycleBadge lifecycle={vault.lifecycle} />
      </div>
      <div className="flex items-end justify-between gap-2">
        <div className="flex flex-col gap-0.5">
          <span className="font-mono text-2xl leading-8 font-semibold tabular-nums">{vault.credentialCount}</span>
          <span className="micro text-muted-foreground text-[10px]">
            {vault.credentialCount === 1 ? 'Credential' : 'Credentials'}
          </span>
        </div>
        <div className="relative z-10 flex flex-col items-end gap-1.5">
          <EnvironmentChip environment={vault.environment} linked />
          <VaultCustodyBadge tier={vault.custody} linked />
        </div>
      </div>
      <ul aria-label={`${vault.displayName} credentials by lifecycle`} className="flex flex-wrap gap-x-3 gap-y-1">
        {counts.length === 0 ? (
          <li className="micro text-muted-foreground text-[10px]">Empty</li>
        ) : (
          counts.map(([lifecycle, count]) => (
            <li
              key={lifecycle}
              className={cn(
                'micro text-muted-foreground flex items-center gap-1.5 text-[10px]',
                toneVariable(credentialLifecycleTones[lifecycle]),
              )}
            >
              <StatusDot tone={credentialLifecycleTones[lifecycle]} />
              <span className="tabular-nums">{count}</span>
              {credentialLifecycleLabels[lifecycle]}
            </li>
          ))
        )}
      </ul>
    </SpotlightSurface>
  );
}

export function VaultCards({ vaults }: { vaults: readonly VaultView[] }) {
  return (
    <section aria-label="Vaults" className="flex flex-col gap-3">
      <h2 className="micro text-muted-foreground flex items-center gap-2 text-[10px]">
        <span aria-hidden="true" className="bg-muted-foreground/50 h-px w-2" />
        Vaults
      </h2>
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
        {vaults.map((vault) => (
          <VaultCard key={vault.vaultId} vault={vault} />
        ))}
      </div>
    </section>
  );
}

export function VaultCardsSkeleton() {
  return (
    <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
      {Array.from({ length: 3 }, (_, index) => (
        <div key={index} className="border-border bg-card flex flex-col gap-4 rounded-lg border p-4">
          <div className="flex items-center gap-2.5">
            <Skeleton className="size-8" />
            <div className="flex flex-col gap-1.5">
              <Skeleton className="h-3.5 w-28" />
              <Skeleton className="h-3 w-20" />
            </div>
          </div>
          <Skeleton className="h-8 w-12" />
          <Skeleton className="h-3 w-40" />
        </div>
      ))}
    </div>
  );
}
