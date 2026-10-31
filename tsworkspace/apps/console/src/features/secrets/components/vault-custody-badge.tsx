import { Link } from '@tanstack/react-router';

import { CustodyTierBadge } from '@/features/custody/components/custody-tier-badge';
import { custodyTierLabels, type CustodyTier } from '@/features/custody/domain/custody';

export function VaultCustodyBadge({ tier, linked }: { tier: CustodyTier; linked?: boolean }) {
  const badge = <CustodyTierBadge tier={tier} />;
  if (!linked) return badge;
  return (
    <Link to="/keys" aria-label={`${custodyTierLabels[tier]}: Encryption keys`} className="outline-none focus-visible:underline">
      {badge}
    </Link>
  );
}
