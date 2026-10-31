import { Link } from '@tanstack/react-router';

import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import type { CustodyTier } from '@/features/custody/domain/custody';

export function VaultCustodyBadge({ tier, linked }: { tier: CustodyTier; linked?: boolean }) {
  const badge = <CustodyTierBadge tier={tier} />;
  if (!linked) return badge;
  return (
    <Link to="/keys" aria-label="Encryption keys" className="outline-none focus-visible:underline">
      {badge}
    </Link>
  );
}
