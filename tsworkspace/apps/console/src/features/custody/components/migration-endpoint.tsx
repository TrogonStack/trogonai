import type { MigrationEnd } from '../domain/view';
import { CustodyTierBadge } from './custody-tier-badge';
import { KeyStateBadge, PlatformManagedBadge } from './key-badges';
import { KeyRefLink } from './key-ref-link';

export function MigrationEndpoint({ end }: { end: MigrationEnd }) {
  return (
    <span className="flex flex-wrap items-center gap-2">
      <CustodyTierBadge tier={end.tier} />
      {end.key ? <KeyRefLink keyRef={end.key.keyRef} /> : null}
      {end.key ? <KeyStateBadge state={end.key.state} /> : null}
      {end.tier !== 'customer_managed' ? <PlatformManagedBadge /> : null}
    </span>
  );
}
