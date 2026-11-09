import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { cn } from '@/lib/utils';

import { custodyTierDetails, custodyTiers, type CustodyTier } from '../domain/custody';
import { CustodyTierBadge } from './custody-tier-badge';

export function CustodyExplainer({ current }: { current?: CustodyTier }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Key custody tiers</CardTitle>
        <CardDescription>
          The data services wrap is encrypted under a key. The tier decides who operates that key and who can take it
          away.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ul className="grid gap-3 md:grid-cols-3">
          {custodyTiers.map((tier) => (
            <li
              key={tier}
              aria-current={tier === current ? 'true' : undefined}
              className={cn(
                'border-border bg-surface-deep/40 flex flex-col items-start gap-2 rounded-md border p-3',
                tier === current && 'border-primary/50',
              )}
            >
              <CustodyTierBadge tier={tier} />
              <p className="text-muted-foreground text-xs">{custodyTierDetails[tier]}</p>
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}
