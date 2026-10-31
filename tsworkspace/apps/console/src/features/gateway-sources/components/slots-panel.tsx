import { Link } from '@tanstack/react-router';

import { CopyButton, truncateMiddle } from '@/components/copy-button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { MetaChip } from '@/features/secrets/components/badges';
import type { SecretRefId } from '@/features/secrets/domain/secret-ref';

import { resolutionDescriptions, resolutionLabels } from '../domain/labels';
import { formatVersion } from '../domain/secret-version';
import type { SlotView } from '../domain/view';
import { SlotStateBadge } from './status-badges';

function SecretReference({ secret, linked }: { secret: SecretRefId; linked: boolean }) {
  return (
    <span className="text-muted-foreground inline-flex items-center gap-1 font-mono text-[11px]">
      {linked ? (
        <Link
          to="/secrets/credentials/$secretRef"
          params={{ secretRef: secret }}
          title={secret}
          className="hover:text-foreground underline-offset-2 hover:underline"
        >
          {truncateMiddle(secret, 8, 6)}
        </Link>
      ) : (
        <span title={secret}>{truncateMiddle(secret, 8, 6)}</span>
      )}
      <CopyButton value={secret} label="secret reference" />
    </span>
  );
}

function SlotRow({ slot, knownSecrets }: { slot: SlotView; knownSecrets: ReadonlySet<string> }) {
  return (
    <li className="border-border flex flex-wrap items-center justify-between gap-3 border-b py-3 first:pt-0 last:border-b-0 last:pb-0">
      <div className="flex min-w-0 flex-col gap-1">
        <span className="text-[13px] leading-5 font-medium">{slot.label}</span>
        {slot.secret ? (
          <SecretReference secret={slot.secret} linked={knownSecrets.has(slot.secret)} />
        ) : (
          <span className="text-muted-foreground font-mono text-[11px]">{slot.envVar}</span>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <MetaChip title={resolutionDescriptions[slot.resolution]}>{resolutionLabels[slot.resolution]}</MetaChip>
        {slot.version !== undefined ? (
          <span className="font-mono text-xs tabular-nums">{formatVersion(slot.version)}</span>
        ) : null}
        {slot.state ? <SlotStateBadge state={slot.state} /> : <MetaChip>From environment</MetaChip>}
      </div>
    </li>
  );
}

export function SlotsPanel({ slots, knownSecrets }: { slots: readonly SlotView[]; knownSecrets: ReadonlySet<string> }) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>Credential slots</CardTitle>
        <CardDescription>
          Each slot holds one secret the gateway needs. Runtime slots follow the credential lifecycle. Static slots are
          read from the gateway environment.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ul aria-label="Credential slots">
          {slots.map((slot) => (
            <SlotRow key={slot.name} slot={slot} knownSecrets={knownSecrets} />
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}
