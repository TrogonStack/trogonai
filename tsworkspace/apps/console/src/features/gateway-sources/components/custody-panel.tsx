import { Link } from '@tanstack/react-router';
import type { ReactNode } from 'react';

import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { AdmissionBadge, KeyStateBadge, PlatformManagedBadge } from '@/features/custody/components/key-badges';
import { custodyTierDetails } from '@/features/custody/domain/custody';

import { failClosedReason, type SourceCustody } from '../domain/source-custody';

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1">
      <dt className="micro text-muted-foreground text-[10px]">{label}</dt>
      <dd className="text-[13px]">{children}</dd>
    </div>
  );
}

export function CustodyPanel({ custody }: { custody: SourceCustody }) {
  const reason = failClosedReason(custody);
  return (
    <Card>
      <CardHeader>
        <CardTitle>Key custody</CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <CustodyTierBadge tier={custody.tier} />
        <p className="text-muted-foreground text-[13px]">{custodyTierDetails[custody.tier]}</p>
        <dl className="grid gap-4">
          {custody.tier === 'customer_managed' ? (
            <>
              <Fact label="Key status">
                <KeyStateBadge state={custody.keyState} />
              </Fact>
              <Fact label="Backend admission">
                <AdmissionBadge admission={custody.admission} />
              </Fact>
            </>
          ) : (
            <Fact label="Operated by">
              <PlatformManagedBadge />
            </Fact>
          )}
        </dl>
        {reason ? (
          <p
            role="status"
            className="border-danger/30 bg-danger-soft text-danger-ink rounded-lg border px-3 py-2.5 text-[13px]"
          >
            The platform fails closed. {reason} It cannot decrypt any credential for this integration until the key is
            usable again, so every delivery is rejected.
          </p>
        ) : null}
        <Link to="/keys" className="text-primary-ink w-fit text-[13px] underline-offset-2 hover:underline">
          Manage encryption keys
        </Link>
      </CardContent>
    </Card>
  );
}
