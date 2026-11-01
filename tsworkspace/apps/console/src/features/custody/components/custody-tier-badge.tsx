import { KeyRoundIcon, LockKeyholeIcon, ShieldIcon, type LucideIcon } from 'lucide-react';

import { cn } from '@/lib/utils';

import { custodyTierLabels, custodyTierSummaries, type CustodyTier } from '../domain/custody';

const icons: Record<CustodyTier, LucideIcon> = {
  managed: ShieldIcon,
  dedicated: LockKeyholeIcon,
  customer_managed: KeyRoundIcon,
};

const tones: Record<CustodyTier, string> = {
  managed: 'border-border bg-muted text-muted-foreground',
  dedicated: 'border-info/35 bg-info-soft text-info-ink',
  customer_managed: 'border-primary/35 bg-primary/10 text-primary-ink',
};

export function CustodyTierBadge({ tier, className }: { tier: CustodyTier; className?: string }) {
  const Icon = icons[tier];
  return (
    <span
      title={custodyTierSummaries[tier]}
      className={cn(
        'micro inline-flex h-5 w-fit shrink-0 items-center gap-1.5 rounded-sm border px-1.5 text-[10px] whitespace-nowrap',
        tones[tier],
        className,
      )}
    >
      <Icon className="size-3" aria-hidden="true" />
      {custodyTierLabels[tier]}
    </span>
  );
}
